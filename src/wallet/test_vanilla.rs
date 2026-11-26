use wasm_bindgen_test::*;

use super::test_bfa::utils::*;
use super::test_bfa::{funded_wallet, wallet_data};
use crate::database::DbWalletTransaction;
use crate::database::memory_db::{ActiveValue, DbBackupInfoActMod};
use crate::wallet::offline::test_wallet::assert_no_partial_change;
use crate::wallet::online::assert_abort_restores;
use crate::wallet::vss::VssBackupConfig;
use crate::wallet::{
    Online, Outpoint, PendingVanillaTx, SyncKeychain, SyncOptions, SyncStrategy, TransactionType,
    Wallet,
};
use crate::{AssetSchema, Error, TransferStatus, WalletTransactionType};
use bdk_wallet::KeychainKind;
use bdk_wallet::bitcoin::OutPoint;
use bdk_wallet::bitcoin::psbt::Psbt;
use bdk_wallet::bitcoin::secp256k1::SecretKey;
use std::str::FromStr;

const AMOUNT: u64 = 1000;

/// A wallet with `utxos` confirmed vanilla UTXOs.
async fn vanilla_wallet(utxos: u8) -> (Wallet, Online) {
    let (mut w, online) = funded_wallet(wallet_data(vec![AssetSchema::Nia]), 1).await;
    for _ in 1..utxos {
        fund_and_sync(&mut w, &online, "1.0").await;
    }
    if utxos > 1 {
        mine_blocks(1).await;
        wait_for_esplora_sync().await;
        w.full_sync(online.clone()).await.unwrap();
    }
    (w, online)
}

fn rcv_address() -> String {
    let (wd, keys) = wallet_data(vec![AssetSchema::Nia]);
    Wallet::new(wd, keys).unwrap().get_address().unwrap()
}

fn txid_of(psbt: &str) -> String {
    Psbt::from_str(psbt)
        .unwrap()
        .unsigned_tx
        .compute_txid()
        .to_string()
}

fn inputs_of(psbt: &str) -> Vec<OutPoint> {
    Psbt::from_str(psbt)
        .unwrap()
        .unsigned_tx
        .input
        .iter()
        .map(|i| i.previous_output)
        .collect()
}

fn row(w: &Wallet, txid: &str) -> DbWalletTransaction {
    let mut rows: Vec<_> = w
        .database
        .iter_wallet_transactions()
        .unwrap()
        .into_iter()
        .filter(|r| r.txid == txid)
        .collect();
    assert_eq!(rows.len(), 1);
    rows.remove(0)
}

fn no_row(w: &Wallet, txid: &str) -> bool {
    w.database
        .get_wallet_transaction_by_txid(txid)
        .unwrap()
        .is_none()
}

fn pending(w: &Wallet) -> Vec<PendingVanillaTx> {
    w.list_pending_vanilla_txs().unwrap()
}

fn pending_types(w: &Wallet) -> Vec<WalletTransactionType> {
    pending(w).into_iter().map(|p| p.r#type).collect()
}

fn reserved(w: &Wallet) -> Vec<OutPoint> {
    w.get_reserved_vanilla_outpoints().unwrap()
}

async fn spendable(w: &mut Wallet) -> u64 {
    w.get_btc_balance(None, true)
        .await
        .unwrap()
        .vanilla
        .spendable
}

async fn send_begin(w: &mut Wallet, online: &Online) -> Result<String, Error> {
    let address = rcv_address();
    w.send_btc_begin(online.clone(), address, AMOUNT, 1, true, false)
        .await
}

async fn create_begin(w: &mut Wallet, online: &Online) -> Result<String, Error> {
    w.create_utxos_begin(online.clone(), false, false, Some(1), None, 1, true, false)
        .await
}

async fn drain_begin(w: &mut Wallet, online: &Online) -> Result<String, Error> {
    w.drain_to_begin(online.clone(), rcv_address(), 1, false)
        .await
}

async fn send_end(w: &mut Wallet, online: &Online, psbt: String) -> Result<String, Error> {
    let signed = w.sign_psbt(psbt, None).unwrap();
    w.send_btc_end(online.clone(), signed).await
}

async fn drain_end(w: &mut Wallet, online: &Online, psbt: String) -> Result<String, Error> {
    let signed = w.sign_psbt(psbt, None).unwrap();
    w.drain_to_end(online.clone(), signed).await
}

async fn mine_and_refresh(w: &mut Wallet, online: &Online) {
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    w.refresh(online.clone(), None, vec![], false)
        .await
        .unwrap();
}

#[wasm_bindgen_test]
async fn list_pending_vanilla_txs_success() {
    let (mut w, online) = vanilla_wallet(3).await;
    assert!(pending(&w).is_empty());

    let send = send_begin(&mut w, &online).await.unwrap();
    assert_eq!(pending_types(&w), vec![WalletTransactionType::SendBtc]);
    create_begin(&mut w, &online).await.unwrap();
    assert_eq!(
        pending_types(&w),
        vec![
            WalletTransactionType::SendBtc,
            WalletTransactionType::CreateUtxos
        ]
    );

    send_end(&mut w, &online, send).await.unwrap();
    assert_eq!(pending_types(&w), vec![WalletTransactionType::CreateUtxos]);
}

#[wasm_bindgen_test]
async fn abort_pending_vanilla_tx_success() {
    let (mut w, online) = vanilla_wallet(1).await;
    let psbt = send_begin(&mut w, &online).await.unwrap();
    let txid = txid_of(&psbt);
    assert_eq!(pending(&w).len(), 1);
    assert!(!reserved(&w).is_empty());

    w.backup("pw").unwrap();
    assert!(!w.backup_info().unwrap());
    w.abort_pending_vanilla_tx(txid.clone()).unwrap();
    assert!(w.backup_info().unwrap());

    assert!(pending(&w).is_empty());
    assert!(reserved(&w).is_empty());
    assert!(no_row(&w, &txid));
    let again = send_begin(&mut w, &online).await.unwrap();
    assert_eq!(inputs_of(&again), inputs_of(&psbt));
}

#[wasm_bindgen_test]
async fn abort_pending_vanilla_tx_fail() {
    let (mut w, online) = vanilla_wallet(1).await;
    let res = w.abort_pending_vanilla_tx("0".repeat(64));
    assert_eq!(res, Err(Error::CannotAbortPendingVanillaTx));

    let psbt = send_begin(&mut w, &online).await.unwrap();
    let txid = send_end(&mut w, &online, psbt).await.unwrap();
    let res = w.abort_pending_vanilla_tx(txid);
    assert_eq!(res, Err(Error::CannotAbortPendingVanillaTx));
}

#[wasm_bindgen_test]
async fn send_btc_begin_reservation_interactions() {
    let (mut w, online) = vanilla_wallet(5).await;
    let before = spendable(&mut w).await;
    let psbt = send_begin(&mut w, &online).await.unwrap();
    let txid = txid_of(&psbt);

    let r = row(&w, &txid);
    assert_eq!(r.r#type, WalletTransactionType::SendBtc);
    assert_eq!(r.status, TransferStatus::Initiated);
    let inputs: Vec<Outpoint> = inputs_of(&psbt).into_iter().map(Outpoint::from).collect();
    assert_eq!(r.inputs, inputs);
    let reserved_inputs: Vec<Outpoint> = reserved(&w).into_iter().map(Outpoint::from).collect();
    assert_eq!(reserved_inputs, inputs);
    let while_reserved = spendable(&mut w).await;
    assert!(while_reserved < before);

    send_end(&mut w, &online, psbt).await.unwrap();
    assert!(reserved(&w).is_empty());
    assert!(pending(&w).is_empty());
    assert!(spendable(&mut w).await > while_reserved);
    row(&w, &txid);

    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    w.full_sync(online.clone()).await.unwrap();
    let tx = w
        .list_transactions(None, true)
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.txid == txid)
        .unwrap();
    assert!(matches!(tx.transaction_type, TransactionType::SendBtc));
    assert!(!tx.pending);
}

#[wasm_bindgen_test]
async fn send_btc_two_concurrent_begins_pick_disjoint_inputs() {
    let (mut w, online) = vanilla_wallet(3).await;
    let first = inputs_of(&send_begin(&mut w, &online).await.unwrap());
    let second = inputs_of(&send_begin(&mut w, &online).await.unwrap());
    assert!(!first.is_empty() && !second.is_empty());
    assert!(first.iter().all(|i| !second.contains(i)));
    assert_eq!(pending(&w).len(), 2);
}

#[wasm_bindgen_test]
async fn create_utxos_begin_reservation_interactions() {
    let (mut w, online) = vanilla_wallet(2).await;
    let psbt = w
        .create_utxos_begin(online.clone(), true, false, None, None, 1, false, false)
        .await
        .unwrap();
    let txid = txid_of(&psbt);
    let inputs: Vec<Outpoint> = reserved(&w).into_iter().map(Outpoint::from).collect();
    assert_eq!(row(&w, &txid).inputs, inputs);
    assert_eq!(pending_types(&w), vec![WalletTransactionType::CreateUtxos]);
    w.abort_pending_vanilla_tx(txid).unwrap();

    let send_inputs = inputs_of(&send_begin(&mut w, &online).await.unwrap());
    assert_eq!(send_inputs.len(), 1);
    let before = reserved(&w);
    let dry = w
        .create_utxos_begin(online.clone(), false, false, Some(1), None, 1, true, true)
        .await
        .unwrap();
    let dry_inputs = inputs_of(&dry);
    assert_eq!(dry_inputs.len(), 1);
    assert!(!send_inputs.contains(&dry_inputs[0]));
    assert_eq!(reserved(&w), before);
    let create_inputs = inputs_of(&create_begin(&mut w, &online).await.unwrap());
    assert_eq!(create_inputs.len(), 1);
    assert!(!send_inputs.contains(&create_inputs[0]));
    let res = create_begin(&mut w, &online).await;
    assert!(matches!(
        res,
        Err(Error::InsufficientBitcoins { available: 0, .. })
    ));
}

#[wasm_bindgen_test]
async fn drain_to_reservation_interaction() {
    let (mut w, online) = vanilla_wallet(3).await;
    let drain = drain_begin(&mut w, &online).await.unwrap();
    let res = send_begin(&mut w, &online).await;
    assert!(matches!(res, Err(Error::InsufficientBitcoins { .. })));

    w.abort_pending_vanilla_tx(txid_of(&drain)).unwrap();
    let send_inputs = inputs_of(&send_begin(&mut w, &online).await.unwrap());
    assert!(!send_inputs.is_empty());
    let drain_inputs = inputs_of(&drain_begin(&mut w, &online).await.unwrap());
    assert!(send_inputs.iter().all(|i| drain_inputs.contains(i)));
}

#[wasm_bindgen_test]
async fn drain_to_repeated_begin_keeps_one_row() {
    // one UTXO: BDK shuffles the inputs of a drain
    let (w_wd, w_keys) = wallet_data(vec![AssetSchema::Nia]);
    let mut w = Wallet::new(w_wd, w_keys).unwrap();
    let online = w.go_online(go_online_options(false)).await.unwrap();
    fund_and_sync(&mut w, &online, "1.0").await;
    let address = rcv_address();
    let first = w
        .drain_to_begin(online.clone(), address.clone(), 1, false)
        .await
        .unwrap();
    let second = w
        .drain_to_begin(online.clone(), address, 1, false)
        .await
        .unwrap();
    let txid = txid_of(&first);
    assert_eq!(txid_of(&second), txid);
    row(&w, &txid);
    assert_eq!(pending(&w).len(), 1);

    drain_end(&mut w, &online, first).await.unwrap();
    assert!(pending(&w).is_empty());
    assert!(reserved(&w).is_empty());
    assert_eq!(row(&w, &txid).status, TransferStatus::WaitingConfirmations);
}

#[wasm_bindgen_test]
async fn begun_vanilla_listed_in_transactions() {
    let (mut w, online) = vanilla_wallet(3).await;
    let mut txids = vec![];
    for tx_type in ["SendBtc", "CreateUtxos", "Drain"] {
        let psbt = match tx_type {
            "SendBtc" => send_begin(&mut w, &online).await,
            "CreateUtxos" => create_begin(&mut w, &online).await,
            _ => drain_begin(&mut w, &online).await,
        }
        .unwrap();
        let txid = txid_of(&psbt);
        let tx = w
            .list_transactions(None, true)
            .await
            .unwrap()
            .into_iter()
            .find(|t| t.txid == txid)
            .unwrap();
        assert_eq!(format!("{:?}", tx.transaction_type), tx_type);
        assert!(tx.pending);
        assert!(tx.fee > 0 && tx.sent > 0);
        assert!(tx.confirmation_time.is_none());
        txids.push(txid);
    }

    for txid in txids {
        w.abort_pending_vanilla_tx(txid).unwrap();
    }
    let txs = w.list_transactions(None, true).await.unwrap();
    assert!(txs.iter().all(|t| !t.pending));
}

#[wasm_bindgen_test]
async fn pending_vanilla_survives_reload() {
    let (mut w, online) = vanilla_wallet(3).await;
    let send = send_begin(&mut w, &online).await.unwrap();
    let create = create_begin(&mut w, &online).await.unwrap();
    let drain = drain_begin(&mut w, &online).await.unwrap();
    let txids: Vec<String> = [&send, &create, &drain].map(|p| txid_of(p)).to_vec();

    let (mut restored, online) = saved_wallet(&w).await;
    for txid in &txids {
        assert_eq!(row(&restored, txid), row(&w, txid));
        assert_eq!(row(&restored, txid).status, TransferStatus::Initiated);
    }
    assert_eq!(pending(&restored), pending(&w));
    assert_eq!(pending(&restored).len(), 3);

    send_end(&mut restored, &online, send).await.unwrap();
    assert_eq!(pending(&restored).len(), 2);
}

#[wasm_bindgen_test]
async fn refresh_settles_ended() {
    let (mut w, online) = vanilla_wallet(3).await;
    let create = w
        .create_utxos_begin(online.clone(), false, false, Some(1), None, 1, false, false)
        .await
        .unwrap();
    let create_txid = txid_of(&create);
    let signed = w.sign_psbt(create, None).unwrap();
    w.create_utxos_end(online.clone(), signed).await.unwrap();
    let send = send_begin(&mut w, &online).await.unwrap();
    let send_txid = send_end(&mut w, &online, send).await.unwrap();

    w.refresh(online.clone(), None, vec![], false)
        .await
        .unwrap();
    for txid in [&create_txid, &send_txid] {
        assert_eq!(row(&w, txid).status, TransferStatus::WaitingConfirmations);
    }
    mine_and_refresh(&mut w, &online).await;
    for txid in [&create_txid, &send_txid] {
        assert_eq!(row(&w, txid).status, TransferStatus::Settled);
    }

    let drain = drain_begin(&mut w, &online).await.unwrap();
    let drain_txid = txid_of(&drain);
    assert_eq!(row(&w, &drain_txid).status, TransferStatus::Initiated);
    drain_end(&mut w, &online, drain).await.unwrap();
    assert_eq!(
        row(&w, &drain_txid).status,
        TransferStatus::WaitingConfirmations
    );
    mine_and_refresh(&mut w, &online).await;
    assert_eq!(row(&w, &drain_txid).status, TransferStatus::Settled);
}

/// Fund an address of the wallet with a confirmed output that the wallet does not know yet.
/// Return the TXID.
async fn unsynced_utxos(address: String) -> String {
    let txid = fund_address(&address, "0.001").await;
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    txid
}

fn has_txo(w: &Wallet, txid: &str) -> bool {
    w.database
        .iter_txos()
        .unwrap()
        .iter()
        .any(|t| t.txid == txid)
}

#[wasm_bindgen_test]
async fn vanilla_operations_are_atomic() {
    let (mut w, online) = vanilla_wallet(3).await;
    // a failed sync drops the UTXOs it found, and a new sync finds them
    let txid = unsynced_utxos(w.get_new_address().unwrap().to_string()).await;
    assert_no_partial_change(&mut w, "sync", 0, async |w| {
        w.full_sync(online.clone()).await
    })
    .await;
    assert!(!has_txo(&w, &txid));
    w.full_sync(online.clone()).await.unwrap();
    assert!(has_txo(&w, &txid));
    let txid = unsynced_utxos(w.get_new_address().unwrap().to_string()).await;
    let full_scan = SyncOptions {
        keychain: SyncKeychain::Colored,
        strategy: SyncStrategy::FullScan,
    };
    assert_no_partial_change(&mut w, "sync", 0, async |w| {
        w.sync(online.clone(), full_scan).await
    })
    .await;
    assert!(!has_txo(&w, &txid));
    w.sync(online.clone(), full_scan).await.unwrap();
    assert!(has_txo(&w, &txid));
    // the vanilla listing syncs the vanilla keychain only, so it uses a vanilla address
    let txid = unsynced_utxos(w.get_address().unwrap()).await;
    assert_no_partial_change(&mut w, "list_unspents_vanilla", 0, async |w| {
        w.list_unspents_vanilla(online.clone(), 0, false).await
    })
    .await;
    let unspents = w
        .list_unspents_vanilla(online.clone(), 0, false)
        .await
        .unwrap();
    assert!(unspents.iter().any(|u| u.outpoint.txid.to_string() == txid));

    assert_no_partial_change(&mut w, "create_utxos_begin", 0, async |w| {
        create_begin(w, &online).await
    })
    .await;
    assert_no_partial_change(&mut w, "send_btc_begin", 0, async |w| {
        send_begin(w, &online).await
    })
    .await;
    assert_no_partial_change(&mut w, "drain_to_begin", 0, async |w| {
        drain_begin(w, &online).await
    })
    .await;

    // an aborted begin restores the wallet and can run again
    let psbt = assert_abort_restores(&mut w, "create_utxos_begin", async |w| {
        create_begin(w, &online).await
    })
    .await;
    w.abort_pending_vanilla_tx(txid_of(&psbt)).unwrap();
    let psbt = assert_abort_restores(&mut w, "send_btc_begin", async |w| {
        send_begin(w, &online).await
    })
    .await;
    w.abort_pending_vanilla_tx(txid_of(&psbt)).unwrap();
    // an end that fails after its broadcast keeps the TX and can run again
    let psbt = assert_abort_restores(&mut w, "drain_to_begin", async |w| {
        drain_begin(w, &online).await
    })
    .await;
    let signed = w.sign_psbt(psbt.clone(), None).unwrap();
    let txids = assert_no_partial_change(&mut w, "drain_to_end", 1, async |w| {
        w.drain_to_end(online.clone(), signed.clone()).await
    })
    .await;
    assert_eq!(txids, vec![txid_of(&psbt)]);
    w.drain_to_end(online.clone(), signed).await.unwrap();
    assert_eq!(
        row(&w, &txid_of(&psbt)).status,
        TransferStatus::WaitingConfirmations
    );
}

#[wasm_bindgen_test]
async fn operation_after_a_later_backup_time_needs_backup() {
    let (w_wd, w_keys) = wallet_data(vec![AssetSchema::Nia]);
    let w = Wallet::new(w_wd, w_keys).unwrap();
    // a saved backup time one hour after the clock
    let ahead = (js_sys::Date::now() as i128 + 3_600_000) * 1_000_000;
    w.database
        .set_backup_info(DbBackupInfoActMod {
            last_backup_timestamp: ActiveValue::Set(ahead.to_string()),
            last_operation_timestamp: ActiveValue::Set("0".to_string()),
            ..Default::default()
        })
        .unwrap();
    assert!(!w.backup_info().unwrap());
    let op = w.update_backup_info(false).unwrap().unwrap();
    assert!(w.backup_info().unwrap());
    // a backup time also comes after the last operation time
    let info = w.update_backup_info(true).unwrap().unwrap();
    assert!(
        info.last_backup_timestamp.parse::<i128>().unwrap()
            > op.last_operation_timestamp.parse::<i128>().unwrap()
    );
    assert!(!w.backup_info().unwrap());
}

#[wasm_bindgen_test]
async fn ensure_initial_backup_uploads_without_record() {
    let (w_wd, w_keys) = wallet_data(vec![AssetSchema::Nia]);
    let mut w = Wallet::new(w_wd, w_keys).unwrap();
    w.configure_vss_backup(&VssBackupConfig::new(
        "http://127.0.0.1:9/vss".to_string(),
        "store".to_string(),
        SecretKey::from_slice(&[1; 32]).unwrap(),
    ));
    assert!(w.database.get_backup_info().unwrap().is_none());
    assert!(w.ensure_initial_backup().await.is_err());
    // the wallet tried an upload
    assert!(w.database.get_backup_info().unwrap().is_some());
}

#[wasm_bindgen_test]
async fn dry_begin_survives_reload() {
    let (mut w, online) = vanilla_wallet(2).await;
    for (r#type, keychain) in [
        (WalletTransactionType::CreateUtxos, KeychainKind::External),
        (WalletTransactionType::SendBtc, KeychainKind::Internal),
    ] {
        let psbt = match r#type {
            WalletTransactionType::CreateUtxos => {
                w.create_utxos_begin(online.clone(), false, false, Some(1), None, 1, true, true)
                    .await
            }
            _ => {
                w.send_btc_begin(online.clone(), rcv_address(), AMOUNT, 1, true, true)
                    .await
            }
        }
        .unwrap();
        let outputs: Vec<_> = Psbt::from_str(&psbt)
            .unwrap()
            .unsigned_tx
            .output
            .into_iter()
            .map(|o| o.script_pubkey)
            .collect();

        let (mut restored, _) = saved_wallet(&w).await;
        let next = restored._get_new_address(keychain).unwrap().script_pubkey();
        assert!(!outputs.contains(&next));
    }
}

#[wasm_bindgen_test]
async fn drain_begin_backup_info() {
    let (mut w, online) = vanilla_wallet(1).await;
    for dry_run in [true, false] {
        w.backup("pw").unwrap();
        sleep_ms(1100).await;
        let psbt = w
            .drain_to_begin(online.clone(), rcv_address(), 1, dry_run)
            .await
            .unwrap();
        assert_eq!(w.backup_info().unwrap(), !dry_run);
        assert_eq!(no_row(&w, &txid_of(&psbt)), dry_run);
        assert_eq!(reserved(&w).is_empty(), dry_run);
    }
}

#[wasm_bindgen_test]
async fn one_step_vanilla_calls() {
    let (mut w, online) = vanilla_wallet(3).await;
    let before: Vec<String> = w
        .database
        .iter_wallet_transactions()
        .unwrap()
        .into_iter()
        .map(|r| r.txid)
        .collect();
    let created = w
        .create_utxos(online.clone(), false, false, Some(2), None, 1, true)
        .await
        .unwrap();
    assert_eq!(created, 2);
    let new_rows: Vec<_> = w
        .database
        .iter_wallet_transactions()
        .unwrap()
        .into_iter()
        .filter(|r| !before.contains(&r.txid))
        .collect();
    assert_eq!(new_rows.len(), 1);
    let create_txid = new_rows[0].txid.clone();
    assert_eq!(new_rows[0].r#type, WalletTransactionType::CreateUtxos);
    assert_eq!(new_rows[0].status, TransferStatus::WaitingConfirmations);
    assert!(pending(&w).is_empty());
    assert!(reserved(&w).is_empty());
    assert_eq!(
        w.create_utxos(online.clone(), true, false, Some(1), None, 1, true)
            .await,
        Err(Error::AllocationsAlreadyAvailable)
    );

    let send_txid = w
        .send_btc(online.clone(), rcv_address(), AMOUNT, 1, true)
        .await
        .unwrap();
    let send_row = row(&w, &send_txid);
    assert_eq!(send_row.r#type, WalletTransactionType::SendBtc);
    assert_eq!(send_row.status, TransferStatus::WaitingConfirmations);
    assert!(pending(&w).is_empty());
    assert!(reserved(&w).is_empty());

    mine_and_refresh(&mut w, &online).await;
    for txid in [&create_txid, &send_txid] {
        assert_eq!(row(&w, txid).status, TransferStatus::Settled);
    }

    // a faulted create keeps colored outputs the DB does not know, so it runs last
    assert_no_partial_change(&mut w, "send_btc", 1, async |w| {
        w.send_btc(online.clone(), rcv_address(), AMOUNT, 1, true)
            .await
    })
    .await;
    assert_no_partial_change(&mut w, "create_utxos", 1, async |w| {
        w.create_utxos(online.clone(), false, false, Some(1), None, 1, true)
            .await
    })
    .await;

    let drain_txid = w.drain_to(online.clone(), rcv_address(), 1).await.unwrap();
    let drain_row = row(&w, &drain_txid);
    assert_eq!(drain_row.r#type, WalletTransactionType::Drain);
    assert_eq!(drain_row.status, TransferStatus::WaitingConfirmations);
    assert!(pending(&w).is_empty());
    mine_and_refresh(&mut w, &online).await;
    assert_eq!(spendable(&mut w).await, 0);
    assert_eq!(row(&w, &drain_txid).status, TransferStatus::Settled);
}

#[wasm_bindgen_test]
async fn drain_to_one_step_is_atomic() {
    // a faulted drain keeps a TX that spends everything
    let (mut w, online) = vanilla_wallet(1).await;
    assert_no_partial_change(&mut w, "drain_to", 1, async |w| {
        w.drain_to(online.clone(), rcv_address(), 1).await
    })
    .await;
}
