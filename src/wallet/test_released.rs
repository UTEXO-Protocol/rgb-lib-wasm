use std::collections::HashMap;
use wasm_bindgen_test::*;

use super::backup::RELEASED_FIXTURES;
use super::test_bfa::utils::*;
use super::test_bfa::{settle_both, wallet_data};
use super::test_send::{begin_map, issue, nia_wallet};
use crate::api::proxy_v1::ProxyClient;
use crate::utils::now;
use crate::wallet::{Recipient, Wallet, WitnessData};
use crate::{AssetSchema, Assignment, TransferStatus};

/// Replace the chain of a fresh stack with the one the fixtures were written on.
async fn replay_fixture_chain() {
    let chain: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/pending/chain.json")).unwrap();
    let tip_height = chain["tip_height"].as_u64().unwrap();
    assert!(get_block_height().await < tip_height, "needs a fresh stack");
    let blocks: Vec<&str> = include_str!("../../tests/fixtures/pending/chain.hex")
        .lines()
        .collect();
    assert_eq!(submit_blocks(&blocks).await, tip_height);
    let client = reqwest::Client::new();
    let url = format!("{ESPLORA_URL}/blocks/tip/hash");
    for poll in 0.. {
        assert!(poll < 300, "esplora did not follow the replayed chain");
        let response = client.get(&url).fetch_cache_no_store().send().await;
        if response.unwrap().text().await.unwrap() == chain["tip_hash"].as_str().unwrap() {
            break;
        }
        sleep_ms(1000).await;
    }
    // the replay dropped the coins of the miner, so mine new ones
    mine_blocks(110).await;
}

/// Reserve the pending receive `rid` again, with the invoice ID of its release.
/// Return its recipient.
async fn pending_recipient(
    wallet: &Wallet,
    rid: &str,
    invoice_id: &str,
    witness_data: Option<WitnessData>,
    amount: u64,
) -> Recipient {
    let db_data = wallet.database.get_db_data(false).unwrap();
    let transfer = db_data
        .transfers
        .iter()
        .find(|t| t.incoming && t.recipient_id.as_deref() == Some(rid))
        .unwrap();
    let asset_transfer = db_data
        .asset_transfers
        .iter()
        .find(|a| a.idx == transfer.asset_transfer_idx)
        .unwrap();
    let batch = db_data
        .batch_transfers
        .iter()
        .find(|b| b.idx == asset_transfer.batch_transfer_idx)
        .unwrap();
    let public_key = hex::encode(transfer.recipient_pubkey.as_ref().unwrap());
    // the proxy keeps a reservation for 30 days at most
    let reserved_until = (now().unix_timestamp() + 86_400).to_string();
    let reserved = ProxyClient::new(PROXY_URL)
        .unwrap()
        .prepare_invoice(invoice_id, &reserved_until, &public_key, 1, None)
        .await
        .unwrap();
    assert_eq!(reserved.result, Some(true), "{:?}", reserved.error);
    Recipient {
        recipient_id: rid.to_string(),
        witness_data,
        assignment: Assignment::Fungible(amount),
        transport_endpoints: vec![transport_endpoint(PROXY_URL)],
        public_key: Some(public_key),
        encryption: transfer.recipient_encryption,
        expiration_timestamp: batch.expiration.map(|e| e as u64),
    }
}

/// The receives pending in the state of each release complete. Their invoices expire on 2035-11-25:
/// write new fixtures before that.
#[wasm_bindgen_test]
#[ignore = "it replays the fixture chain, so it needs a fresh stack"]
async fn released_pending_receives_complete() {
    replay_fixture_chain().await;
    // a UTXO for the change of each send
    let (mut s, s_on) = nia_wallet(10).await;
    let asset_id = issue(&s, vec![1000]);

    for fixture in RELEASED_FIXTURES {
        let meta: serde_json::Value = serde_json::from_str(fixture.meta).unwrap();
        for name in ["snapshot", "backup"] {
            web_sys::console::log_1(&format!("{} {name}", fixture.tag).into());
            let wallet_data = wallet_data(vec![AssetSchema::Nia]).0;
            let mut r = Wallet::new(wallet_data, fixture.singlesig_keys(name)).unwrap();
            if name == "snapshot" {
                let snapshot = serde_json::from_str(fixture.snapshot).unwrap();
                r.restore_from_snapshot(snapshot).unwrap();
            } else {
                r.restore_backup(fixture.backup, "password").unwrap();
            }
            let r_on = r.go_online(go_online_options(false)).await.unwrap();
            full_sync(&mut r, &r_on).await;

            let rids = ["blind_rid", "witness_rid"].map(|k| meta[name][k].as_str().unwrap());
            let ids = rids.map(|rid| meta[name]["invoice_ids"][rid].as_str().unwrap());
            let witness_data = Some(WitnessData {
                amount_sat: 1000,
                blinding: None,
            });
            let recipients = vec![
                pending_recipient(&r, rids[0], ids[0], None, 50).await,
                pending_recipient(&r, rids[1], ids[1], witness_data, 30).await,
            ];
            let map = HashMap::from([(asset_id.clone(), recipients)]);
            let psbt = begin_map(&mut s, &s_on, map, false).await.unwrap();
            let signed = s.sign_psbt(psbt, None).unwrap();
            s.send_end(s_on.clone(), signed).await.unwrap();
            settle_both(&mut s, &s_on, &mut r, &r_on).await;

            assert_eq!(r.get_asset_balance(asset_id.clone()).unwrap().settled, 80);
            for rid in rids {
                let received = transfer_of(&r, rid);
                assert_eq!(received.status, TransferStatus::Settled);
                let consignment = r.get_consignment(received.idx).unwrap();
                assert!(consignment.is_some());
                let sent = transfer_of(&s, rid);
                assert_eq!(s.get_consignment(sent.idx).unwrap(), consignment);
            }
        }
    }
}
