use bdk_wallet::KeychainKind;
use bdk_wallet::bitcoin::Psbt;
use bdk_wallet::bitcoin::ScriptBuf;
use std::collections::HashMap;
use std::str::FromStr;
use wasm_bindgen_test::*;

use super::test_bfa::utils::*;
use super::test_bfa::{funded_wallet, s, send, send_recipients, settle_both, wallet_data};
use super::test_send::issue;
use crate::utils::CHECK_AT;
use crate::wallet::{AssetFilter, Outpoint, TransferKind, TransferScope, Wallet};
use crate::{AssetSchema, Assignment, Error, TransferStatus};

const SENT: u64 = 66;

fn first_spk(wallet: &Wallet, keychain: KeychainKind) -> ScriptBuf {
    wallet.bdk_wallet.peek_address(keychain, 0).script_pubkey()
}

fn unspent_spks(wallet: &Wallet, keychain: KeychainKind) -> Vec<ScriptBuf> {
    wallet
        .bdk_wallet
        .list_unspent()
        .filter(|u| u.keychain == keychain)
        .map(|u| u.txout.script_pubkey)
        .collect()
}

// The output is on the first colored script, is available for allocations, and does not pay the
// receive pending there.
fn assert_own_change(wallet: &Wallet, txid: &str, vout: u32) {
    let first = first_spk(wallet, KeychainKind::External);
    let tx = wallet.bdk_wallet.get_tx(txid.parse().unwrap()).unwrap();
    assert_eq!(tx.tx_node.output[vout as usize].script_pubkey, first);
    let txid = txid.to_string();
    let outpoint = Outpoint { txid, vout };
    let txo = wallet.database.get_txo(&outpoint).unwrap().unwrap();
    assert!(txo.exists && !txo.pending_witness);
    let txos = wallet.database.get_unspent_txos(vec![]).unwrap();
    let unspents = wallet
        .database
        .get_rgb_allocations(txos, None, None, None, None);
    let available = wallet.get_available_allocations(unspents.unwrap(), &[], None);
    assert!(
        available
            .unwrap()
            .iter()
            .any(|u| u.utxo.outpoint() == outpoint)
    );
    let scripts = wallet.database.iter_pending_witness_scripts().unwrap();
    assert!(scripts.iter().any(|s| s.script == first.to_hex_string()));
}

// The output of the signed PSBT is in the DB when the broadcast starts.
fn expect_registered_at_broadcast(wallet: &Wallet, psbt: &str, vout: u32) {
    let txid = Psbt::from_str(psbt).unwrap().unsigned_tx.compute_txid();
    let outpoint = Outpoint {
        txid: txid.to_string(),
        vout,
    };
    let database = wallet.database.clone();
    let check = move || assert!(database.get_txo(&outpoint).unwrap().is_some());
    CHECK_AT.with_borrow_mut(|c| *c = Some(("broadcast", Box::new(check))));
}

fn assert_checked_at_broadcast() {
    assert!(CHECK_AT.with_borrow(Option::is_none));
}

#[wasm_bindgen_test]
async fn colored_address_reused() {
    let (mut wd, keys) = wallet_data(vec![AssetSchema::Nia]);
    wd.address_reuse_colored_auto = true;
    let (mut wallet, online) = funded_wallet((wd, keys), 3).await;
    let first = first_spk(&wallet, KeychainKind::External);
    assert_ne!(wallet.get_address().unwrap(), wallet.get_address().unwrap());
    let colored = unspent_spks(&wallet, KeychainKind::External);
    assert_eq!(colored, vec![first.clone(); 3]);
    let txos = wallet.database.iter_txos().unwrap();
    assert!(txos.iter().all(|t| !t.pending_witness));

    let asset_id = issue(&wallet, vec![1000]);
    let (mut rcv, rcv_online) = funded_wallet(wallet_data(vec![AssetSchema::Nia]), 1).await;
    let amount = Assignment::Fungible(SENT);
    let txid = send(
        &mut wallet,
        &online,
        &mut rcv,
        &rcv_online,
        &asset_id,
        amount,
    )
    .await;
    settle_both(&mut wallet, &online, &mut rcv, &rcv_online).await;
    let sent = wallet
        .list_transfers(AssetFilter::AnyOrNone, TransferScope::All)
        .unwrap()
        .into_iter()
        .find(|t| t.kind == TransferKind::Send && t.txid.as_deref() == Some(&txid))
        .unwrap();
    let tx = wallet.bdk_wallet.get_tx(txid.parse().unwrap()).unwrap();
    assert!(!sent.change_utxos.is_empty());
    for change in sent.change_utxos {
        assert_eq!(tx.tx_node.output[change.vout as usize].script_pubkey, first);
    }
    let index = wallet.bdk_wallet.spk_index();
    assert_eq!(index.last_revealed_index(KeychainKind::External), Some(0));
}

// Two receives on one address, paid one by one. Each receive keeps its own data.
async fn reuse_address_paid(colored_reuse: bool, reverse: bool) {
    let (mut sender, sender_online) = funded_wallet(wallet_data(vec![AssetSchema::Nia]), 5).await;
    let asset_id = issue(&sender, vec![1000]);
    let (mut wd, keys) = wallet_data(vec![AssetSchema::Nia]);
    wd.address_reuse_colored_auto = colored_reuse;
    let (mut rcv, rcv_online) = funded_wallet((wd, keys), 1).await;
    let first = rcv.bdk_wallet.peek_address(KeychainKind::External, 0);
    let first = first.to_string();
    let address = colored_reuse.then(|| first.clone());
    let d1 = witness_rcv_at(&mut rcv, &rcv_online, address)
        .await
        .unwrap();
    let d2 = witness_rcv_at(&mut rcv, &rcv_online, d1.address.clone())
        .await
        .unwrap();
    assert!(d1.address.is_some());
    assert_eq!(d1.address == Some(first), colored_reuse);
    assert_eq!(d2.address, d1.address);
    assert_eq!(d2.recipient_id, d1.recipient_id);
    assert_ne!(d2.public_key, d1.public_key);
    assert_ne!(d2.invoice, d1.invoice);

    // The setting does not change this check, so it runs once.
    if !colored_reuse {
        let both = vec![
            recipient_for(&d1, SENT, 1000),
            recipient_for(&d2, 2 * SENT, 1000),
        ];
        let res = sender
            .send_begin(
                sender_online.clone(),
                HashMap::from([(asset_id.clone(), both)]),
                false,
                1,
                1,
                default_send_expiration(),
                Some(transport_endpoint(PROXY_URL)),
                None,
                false,
            )
            .await;
        assert!(matches!(res, Err(Error::RecipientIDDuplicated)));
    }

    let mut payments = [(&d1, SENT), (&d2, 2 * SENT)];
    if reverse {
        payments.reverse();
    }
    for (d, amount) in payments {
        let recipients = vec![recipient_for(d, amount, 1000)];
        send_recipients(&mut sender, &sender_online, &asset_id, recipients).await;
        settle_both(&mut sender, &sender_online, &mut rcv, &rcv_online).await;
        let transfer = transfer_at(&rcv, d.batch_transfer_idx);
        assert_eq!(transfer.status, TransferStatus::Settled);
        assert_eq!(transfer.assignments, vec![Assignment::Fungible(amount)]);
    }
    let balance = rcv.get_asset_balance(asset_id).unwrap();
    assert_eq!((balance.settled, balance.spendable), (3 * SENT, 3 * SENT));
}

#[wasm_bindgen_test]
async fn reuse_address() {
    reuse_address_paid(false, true).await;
}

#[wasm_bindgen_test]
async fn reuse_address_colored_reuse_in_order() {
    reuse_address_paid(true, false).await;
}

// The wallet's own outputs on its first colored address are usable at once and pay no receive.
#[wasm_bindgen_test]
async fn own_outputs_usable_with_pending_deposit() {
    let (mut wd, keys) = wallet_data(vec![AssetSchema::Nia]);
    wd.address_reuse_colored_auto = true;
    let (mut wallet, online) = funded_wallet((wd, keys), 1).await;
    let first = wallet.get_first_address(KeychainKind::External).unwrap();
    let plain = witness_rcv_at(&mut wallet, &online, None).await.unwrap();
    assert_ne!(plain.address, Some(first.to_string()));
    let deposit = witness_rcv_at(&mut wallet, &online, Some(first.to_string()))
        .await
        .unwrap();

    let unsigned = wallet
        .create_utxos_begin(online.clone(), false, false, Some(2), None, 1, false, false)
        .await
        .unwrap();
    let signed = wallet.sign_psbt(unsigned, None).unwrap();
    let txid = Psbt::from_str(&signed).unwrap().unsigned_tx.compute_txid();
    let created = wallet.create_utxos_end(online.clone(), signed).await;
    assert_eq!(created, Ok(2));
    let spk = first.script_pubkey();
    let tx = wallet.bdk_wallet.get_tx(txid).unwrap();
    let txid = txid.to_string();
    let txos = wallet.database.iter_txos().unwrap();
    let own: Vec<_> = txos.iter().filter(|t| t.txid == txid).collect();
    assert_eq!(own.len(), 2);
    for txo in own {
        assert_eq!(tx.tx_node.output[txo.vout as usize].script_pubkey, spk);
        assert!(txo.exists && !txo.pending_witness);
    }
    // The begin syncs, and all three UTXOs stay available, unconfirmed.
    let res = wallet
        .create_utxos_begin(online.clone(), true, false, Some(3), None, 1, false, false)
        .await;
    assert!(matches!(res, Err(Error::AllocationsAlreadyAvailable)));
    let scripts = wallet.database.iter_pending_witness_scripts().unwrap();
    assert!(scripts.iter().any(|s| s.script == spk.to_hex_string()));
    let status = transfer_at(&wallet, deposit.batch_transfer_idx).status;
    assert_eq!(status, TransferStatus::WaitingCounterparty);

    // The bitcoin change of a send with no RGB change is also usable before a sync or block.
    // It follows the OP_RETURN output.
    let asset_id = issue(&wallet, vec![1000]);
    let (mut rcv, rcv_online) = funded_wallet(wallet_data(vec![AssetSchema::Nia]), 1).await;
    let exp = default_rcv_expiration();
    let endpoints = vec![transport_endpoint(PROXY_URL)];
    let blind = blind_rcv(&mut rcv, &rcv_online, None, Assignment::Any, exp, endpoints).await;
    let recipients = vec![recipient(
        &blind.unwrap().recipient_id,
        1000,
        None,
        PROXY_URL,
    )];
    // A donation broadcasts in the end.
    let begin = wallet
        .send_begin(
            online.clone(),
            HashMap::from([(asset_id, recipients)]),
            true,
            1,
            1,
            default_send_expiration(),
            Some(transport_endpoint(PROXY_URL)),
            None,
            false,
        )
        .await
        .unwrap();
    let signed = wallet.sign_psbt(begin.psbt, None).unwrap();
    let txid = wallet.send_end(online.clone(), signed).await.unwrap().txid;
    assert_own_change(&wallet, &txid, 1);
}

// After a dry-run begin, the end registers its change before it broadcasts it.
#[wasm_bindgen_test]
async fn own_outputs_registered_after_dry_run() {
    let (mut wd, keys) = wallet_data(vec![AssetSchema::Nia, AssetSchema::Ifa]);
    wd.address_reuse_colored_auto = true;
    let (mut wallet, online) = funded_wallet((wd, keys), 5).await;
    let (mut rcv, rcv_online) = funded_wallet(wallet_data(vec![AssetSchema::Nia]), 1).await;
    let first = wallet.get_first_address(KeychainKind::External).unwrap();
    witness_rcv_at(&mut wallet, &online, Some(first.to_string()))
        .await
        .unwrap();

    // The change of a donation send follows the OP_RETURN output and the recipient one.
    let asset_id = issue(&wallet, vec![1000]);
    let data = witness_rcv_at(&mut rcv, &rcv_online, None).await.unwrap();
    let recipients = vec![recipient_for(&data, SENT, 1000)];
    let begin = wallet
        .send_begin(
            online.clone(),
            HashMap::from([(asset_id, recipients)]),
            true,
            1,
            1,
            default_send_expiration(),
            Some(transport_endpoint(PROXY_URL)),
            None,
            true,
        )
        .await
        .unwrap();
    assert!(begin.batch_transfer_idx.is_none());
    let signed = wallet.sign_psbt(begin.psbt, None).unwrap();
    expect_registered_at_broadcast(&wallet, &signed, 2);
    let sent = wallet.send_end(online.clone(), signed).await.unwrap();
    assert_checked_at_broadcast();
    assert_own_change(&wallet, &sent.txid, 2);

    // The change of an inflation follows the OP_RETURN output and the two new allocations.
    let asset = wallet
        .issue_asset_ifa(
            s("IFA"),
            s("Inflatable"),
            0,
            vec![1000],
            vec![100],
            None,
            false,
            None,
            false,
        )
        .unwrap();
    let begin = wallet
        .inflate_begin(online.clone(), asset.asset_id, vec![1, 2], 1, 1, true)
        .await
        .unwrap();
    assert!(begin.batch_transfer_idx.is_none());
    let signed = wallet.sign_psbt(begin.psbt, None).unwrap();
    expect_registered_at_broadcast(&wallet, &signed, 3);
    let inflated = wallet.inflate_end(online, signed).await.unwrap();
    assert_checked_at_broadcast();
    assert_own_change(&wallet, &inflated.txid, 3);
}
