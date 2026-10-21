use wasm_bindgen_test::*;

use super::test_bfa::utils::*;
use super::test_bfa::*;
use crate::wallet::{AssetFilter, Online, Transfer, Wallet};
use crate::{
    AssetSchema, Assignment, BitcoinNetwork, Error, Fascia, FileContent, RgbOutExt, RgbTransfer,
    TS_BURN, TransferKind, TransferStatus, wallet::rust_only::validate_consignment_offchain,
};
use amplify::confinement::{Confined, U32};
use bdk_wallet::bitcoin::psbt::Psbt;
use std::str::FromStr;
use strict_encoding::StrictDeserialize;

fn reason() -> String {
    "ab".repeat(32)
}

/// Mine and refresh three times, so that the miner gets the broadcast transaction.
async fn mine_and_refresh(wallet: &mut Wallet, online: &Online) {
    for _ in 0..3 {
        mine_blocks(1).await;
        wait_for_esplora_sync().await;
        wallet.sync(online.clone()).await.unwrap();
        wallet
            .refresh(online.clone(), None, vec![], false)
            .await
            .unwrap();
    }
}

fn balance(wallet: &Wallet, asset_id: &str) -> (u64, u64, u64) {
    let b = wallet.get_asset_balance(asset_id.to_string()).unwrap();
    (b.settled, b.future, b.spendable)
}

fn transfer(wallet: &Wallet, asset_id: &str, batch_transfer_idx: i32) -> Transfer {
    wallet
        .list_transfers(AssetFilter::Id(asset_id.to_string()), None)
        .unwrap()
        .into_iter()
        .find(|t| t.batch_transfer_idx == batch_transfer_idx)
        .unwrap()
}

fn burns(wallet: &Wallet, asset_id: &str) -> Vec<(TransferKind, Option<String>)> {
    wallet
        .list_transfers(AssetFilter::Id(asset_id.to_string()), None)
        .unwrap()
        .into_iter()
        .filter(|t| t.kind == TransferKind::Burn)
        .map(|t| (t.kind, t.burn_reason))
        .collect()
}

async fn ifa(wallet_utxos: u8) -> (Wallet, Online, String, crate::wallet::WalletData) {
    let wd = wallet_data(vec![AssetSchema::Nia, AssetSchema::Ifa]);
    let (wallet, online) = funded_wallet(wd.clone(), wallet_utxos).await;
    let asset = wallet
        .issue_asset_ifa(
            s("IFA"),
            s("Inflatable"),
            0,
            vec![600, 400],
            vec![100],
            None,
        )
        .unwrap();
    (wallet, online, asset.asset_id, wd)
}

#[wasm_bindgen_test]
async fn ifa_burn_with_and_without_reason_settles() {
    let (mut wallet, online, asset_id, wd) = ifa(5).await;

    // one-step burn with a reason
    let res = wallet
        .burn(online.clone(), asset_id.clone(), 150, Some(reason()), 1, 1)
        .await
        .unwrap();
    mine_and_refresh(&mut wallet, &online).await;
    assert_eq!(balance(&wallet, &asset_id), (850, 850, 850));
    let burned = transfer(&wallet, &asset_id, res.batch_transfer_idx);
    assert_eq!(burned.kind, TransferKind::Burn);
    assert_eq!(burned.status, TransferStatus::Settled);
    assert_eq!(burned.burn_reason, Some(reason()));
    assert_eq!(burned.requested_assignment, Some(Assignment::Fungible(150)));
    assert_eq!(burned.assignments, vec![Assignment::Fungible(250)]);
    assert_eq!(burned.recipient_id, None);
    assert!(burned.change_utxo.is_some());

    // the kept consignment proves the burn with the signed transaction
    let bytes = wallet.transfer_artifacts[&res.txid].consignment_bytes[&asset_id].clone();
    assert!(
        validate_consignment_offchain(&bytes, &res.txid, BitcoinNetwork::Regtest)
            .unwrap()
            .valid
    );
    let consignment = RgbTransfer::load(&bytes[..]).unwrap();
    let witness_bundle = consignment
        .bundles
        .iter()
        .find(|wb| wb.tx.compute_txid().to_string() == res.txid)
        .unwrap();
    assert!(
        witness_bundle
            .tx
            .input
            .iter()
            .all(|i| !i.witness.is_empty())
    );
    assert!(
        witness_bundle
            .bundle
            .known_transitions
            .iter()
            .any(|kt| kt.transition.transition_type == TS_BURN)
    );
    assert!(bytes.windows(32).any(|w| w == [0xab; 32]));
    let unspents = wallet.list_unspents(None, false, true).unwrap();
    assert!(
        unspents
            .iter()
            .flat_map(|u| &u.rgb_allocations)
            .all(|a| a.assignment != Assignment::Fungible(0))
    );

    // begin without dry run, then end
    let begin = wallet
        .burn_begin(online.clone(), asset_id.clone(), 700, None, 1, 1, false)
        .await
        .unwrap();
    let idx = begin.batch_transfer_idx.unwrap();
    // refresh skips the initiated burn and its inputs are not spendable
    wallet
        .refresh(online.clone(), None, vec![], false)
        .await
        .unwrap();
    assert_eq!(
        transfer(&wallet, &asset_id, idx).status,
        TransferStatus::Initiated
    );
    assert!(matches!(
        wallet
            .burn_begin(online.clone(), asset_id.clone(), 1, None, 1, 1, true)
            .await,
        Err(Error::InsufficientAssignments { .. })
    ));
    assert_eq!(begin.details.burn_reason, None);
    let psbt = Psbt::from_str(&begin.psbt).unwrap();
    let opret = psbt
        .unsigned_tx
        .output
        .iter()
        .position(|o| o.script_pubkey.is_op_return())
        .unwrap();
    assert_eq!(
        psbt.outputs[opret].mpc_entropy(),
        Some(begin.details.entropy)
    );
    let fascia = Fascia::from_strict_serialized::<U32>(
        Confined::try_from(hex::decode(&begin.details.fascia).unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        fascia.witness_id().to_string(),
        psbt.unsigned_tx.compute_txid().to_string()
    );
    let signed = wallet.sign_psbt(begin.psbt, None).unwrap();
    let res = wallet.burn_end(online.clone(), signed).await.unwrap();
    assert_eq!(res.batch_transfer_idx, idx);
    assert_eq!(res.entropy, begin.details.entropy);
    mine_and_refresh(&mut wallet, &online).await;
    let burned = transfer(&wallet, &asset_id, idx);
    assert_eq!(burned.status, TransferStatus::Settled);
    assert_eq!(burned.burn_reason, None);
    assert_eq!(balance(&wallet, &asset_id), (150, 150, 150));

    // a reload and a restore keep the burns
    wallet.flush().await.unwrap();
    drop(wallet);
    let wallet = Wallet::restore(wd.clone()).await.unwrap();
    let backup = wallet.backup("password").unwrap();
    let mut restored = Wallet::new(wd).unwrap();
    restored.restore_backup(&backup, "password").unwrap();
    let expected = vec![
        (TransferKind::Burn, Some(reason())),
        (TransferKind::Burn, None),
    ];
    assert_eq!(burns(&wallet, &asset_id), expected);
    assert_eq!(burns(&restored, &asset_id), expected);
    assert_eq!(balance(&restored, &asset_id), (150, 150, 150));
}

#[wasm_bindgen_test]
async fn bfa_burn_settles_and_keeps_send_receive() {
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 3).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let bridge = new_evm_address();
    let reject_list = new_evm_address();
    deploy_bridge(&bridge).await;
    deploy_reject_list(&reject_list, &[]).await;
    let (asset_id, opid) = bfa_with_units(&mut a, &a_online, &bridge, Some(reject_list)).await;

    let res = a
        .burn(a_online.clone(), asset_id.clone(), 30, Some(reason()), 1, 1)
        .await
        .unwrap();
    mine_and_refresh(&mut a, &a_online).await;
    assert_eq!(balance(&a, &asset_id), (70, 70, 70));
    let burned = transfer(&a, &asset_id, res.batch_transfer_idx);
    assert_eq!(burned.status, TransferStatus::Settled);
    assert_eq!(burned.burn_reason, Some(reason()));

    let begin = a
        .burn_begin(a_online.clone(), asset_id.clone(), 20, None, 1, 1, true)
        .await
        .unwrap();
    assert_eq!(begin.batch_transfer_idx, None);
    let signed = a.sign_psbt(begin.psbt, None).unwrap();
    let res = a.burn_end(a_online.clone(), signed).await.unwrap();
    mine_and_refresh(&mut a, &a_online).await;
    assert_eq!(balance(&a, &asset_id), (50, 50, 50));
    assert_eq!(
        transfer(&a, &asset_id, res.batch_transfer_idx).burn_reason,
        None
    );
    assert_eq!(mint_rights(&mut a, &asset_id), 1);

    emit_funds_in(&bridge, opid, 100).await;
    finalize_evm().await;
    send(&mut a, &a_online, &mut b, &asset_id, 10).await;
    let statuses = settle(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
    assert_eq!(balance(&b, &asset_id), (10, 10, 10));
    assert_eq!(balance(&a, &asset_id), (40, 40, 40));
}

#[wasm_bindgen_test]
async fn burn_end_refuses_failed_ended_and_other_operations() {
    let (mut wallet, online, asset_id, _) = ifa(5).await;
    let before = balance(&wallet, &asset_id);

    // a failed burn
    let begin = wallet
        .burn_begin(online.clone(), asset_id.clone(), 700, None, 1, 1, false)
        .await
        .unwrap();
    let idx = begin.batch_transfer_idx.unwrap();
    let txid = Psbt::from_str(&begin.psbt)
        .unwrap()
        .unsigned_tx
        .compute_txid()
        .to_string();
    assert!(
        wallet
            .fail_transfers(online.clone(), Some(idx), false, false)
            .await
            .unwrap()
    );
    assert_eq!(balance(&wallet, &asset_id), before);
    let signed = wallet.sign_psbt(begin.psbt, None).unwrap();
    assert!(matches!(
        wallet.burn_end(online.clone(), signed).await,
        Err(Error::CannotEndTransfer { .. })
    ));
    mine_and_refresh(&mut wallet, &online).await;
    assert_eq!(
        transfer(&wallet, &asset_id, idx).status,
        TransferStatus::Failed
    );
    assert_eq!(
        wallet.indexer().get_tx_confirmations(&txid).await.unwrap(),
        None
    );

    // a burn PSBT in the end method of another operation
    let begin = wallet
        .burn_begin(online.clone(), asset_id.clone(), 100, None, 1, 1, false)
        .await
        .unwrap();
    let idx = begin.batch_transfer_idx.unwrap();
    let txid = Psbt::from_str(&begin.psbt)
        .unwrap()
        .unsigned_tx
        .compute_txid()
        .to_string();

    // refresh skips the initiated burn and an RGB begin does not spend its allocations
    wallet
        .refresh(online.clone(), None, vec![], false)
        .await
        .unwrap();
    assert_eq!(
        transfer(&wallet, &asset_id, idx).status,
        TransferStatus::Initiated
    );
    let colored: Vec<String> = wallet
        .list_unspents(None, false, true)
        .unwrap()
        .into_iter()
        .filter(|u| !u.rgb_allocations.is_empty())
        .map(|u| u.utxo.outpoint.to_string())
        .collect();
    let colored_inputs = |psbt: &str| -> Vec<String> {
        Psbt::from_str(psbt)
            .unwrap()
            .unsigned_tx
            .input
            .iter()
            .map(|i| i.previous_output.to_string())
            .filter(|o| colored.contains(o))
            .collect()
    };
    let other = wallet
        .burn_begin(online.clone(), asset_id.clone(), 100, None, 1, 1, true)
        .await
        .unwrap();
    let (locked, free) = (colored_inputs(&begin.psbt), colored_inputs(&other.psbt));
    assert!(!locked.is_empty() && !free.is_empty());
    assert!(
        free.iter().all(|o| !locked.contains(o)),
        "{locked:?} {free:?}"
    );

    let signed = wallet.sign_psbt(begin.psbt.clone(), None).unwrap();
    let wrong = |r: &Result<(), Error>| matches!(r, Err(Error::WrongOperation { .. }));
    let results = [
        wallet
            .send_end(online.clone(), signed.clone(), false)
            .await
            .map(|_| ()),
        wallet
            .inflate_end(online.clone(), signed.clone())
            .await
            .map(|_| ()),
        wallet
            .create_utxos_end(online.clone(), signed.clone(), false)
            .await
            .map(|_| ()),
        wallet
            .drain_to_end(online.clone(), signed.clone())
            .await
            .map(|_| ()),
        wallet
            .send_btc_end(online.clone(), signed.clone(), false)
            .await
            .map(|_| ()),
    ];
    assert!(results.iter().all(wrong), "{results:?}");

    // a failed broadcast keeps the burn to end again
    assert!(wallet.burn_end(online.clone(), begin.psbt).await.is_err());
    assert_eq!(
        transfer(&wallet, &asset_id, idx).status,
        TransferStatus::Initiated
    );
    assert!(wallet.transfer_artifacts[&txid].fascia.is_some());
    wallet
        .burn_end(online.clone(), signed.clone())
        .await
        .unwrap();

    // an ended burn
    assert!(matches!(
        wallet.burn_end(online.clone(), signed.clone()).await,
        Err(Error::CannotEndTransfer { .. })
    ));
    assert!(wrong(
        &wallet
            .create_utxos_end(online.clone(), signed.clone(), false)
            .await
            .map(|_| ())
    ));

    // an inflate PSBT in burn_end
    let unsigned = wallet
        .inflate_begin(online.clone(), asset_id.clone(), vec![50], 1, 1)
        .await
        .unwrap();
    let inflate = wallet.sign_psbt(unsigned, None).unwrap();
    assert!(wrong(
        &wallet
            .burn_end(online.clone(), inflate.clone())
            .await
            .map(|_| ())
    ));
    wallet.inflate_end(online.clone(), inflate).await.unwrap();

    // a burn of another wallet
    let mut other = Wallet::new(wallet_data(vec![AssetSchema::Ifa])).unwrap();
    let other_online = other
        .go_online(false, ESPLORA_URL.to_string(), Default::default())
        .await
        .unwrap();
    assert!(matches!(
        other.burn_end(other_online, signed).await,
        Err(Error::UnknownTransfer { .. })
    ));
}
