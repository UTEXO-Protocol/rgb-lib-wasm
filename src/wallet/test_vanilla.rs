use wasm_bindgen_test::*;

use super::test_bfa::utils::*;
use super::test_bfa::{funded_wallet, wallet_data};
use crate::database::DbWalletTransaction;
use crate::wallet::{Online, Outpoint, PendingVanillaTx, TransactionType, Wallet};
use crate::{AssetSchema, Error, TransferStatus, WalletTransactionType};
use bdk_wallet::bitcoin::OutPoint;
use bdk_wallet::bitcoin::psbt::Psbt;
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
        w.sync(online.clone()).await.unwrap();
    }
    (w, online)
}

fn rcv_address() -> String {
    Wallet::new(wallet_data(vec![AssetSchema::Nia]))
        .unwrap()
        .get_address()
        .unwrap()
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

fn pending(w: &Wallet) -> Vec<PendingVanillaTx> {
    w.list_pending_vanilla_txs().unwrap()
}

fn pending_types(w: &Wallet) -> Vec<WalletTransactionType> {
    pending(w).into_iter().map(|p| p.r#type).collect()
}

fn reserved(w: &Wallet) -> Vec<OutPoint> {
    w.get_reserved_vanilla_outpoints().unwrap()
}

fn spendable(w: &mut Wallet) -> u64 {
    w.get_btc_balance(None, true).unwrap().vanilla.spendable
}

async fn send_begin(w: &mut Wallet, online: &Online) -> Result<String, Error> {
    let address = rcv_address();
    w.send_btc_begin(online.clone(), address, AMOUNT, 1, true)
        .await
}

async fn create_begin(w: &mut Wallet, online: &Online) -> Result<String, Error> {
    w.create_utxos_begin(online.clone(), false, Some(1), None, 1, true)
        .await
}

async fn drain_begin(w: &mut Wallet, online: &Online) -> Result<String, Error> {
    w.drain_to_begin(online.clone(), rcv_address(), false, 1)
        .await
}

async fn send_end(w: &mut Wallet, online: &Online, psbt: String) -> Result<String, Error> {
    let signed = w.sign_psbt(psbt, None).unwrap();
    w.send_btc_end(online.clone(), signed, true).await
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
    sleep_ms(1100).await;
    w.abort_pending_vanilla_tx(txid.clone()).unwrap();
    assert!(w.backup_info().unwrap());

    assert!(pending(&w).is_empty());
    assert!(reserved(&w).is_empty());
    assert_eq!(row(&w, &txid).status, TransferStatus::Failed);
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
    let before = spendable(&mut w);
    let psbt = send_begin(&mut w, &online).await.unwrap();
    let txid = txid_of(&psbt);

    let r = row(&w, &txid);
    assert_eq!(r.r#type, WalletTransactionType::SendBtc);
    assert_eq!(r.status, TransferStatus::Initiated);
    let inputs: Vec<Outpoint> = inputs_of(&psbt).into_iter().map(Outpoint::from).collect();
    assert_eq!(r.inputs, inputs);
    let reserved_inputs: Vec<Outpoint> = reserved(&w).into_iter().map(Outpoint::from).collect();
    assert_eq!(reserved_inputs, inputs);
    let while_reserved = spendable(&mut w);
    assert!(while_reserved < before);

    send_end(&mut w, &online, psbt).await.unwrap();
    assert!(reserved(&w).is_empty());
    assert!(pending(&w).is_empty());
    assert!(spendable(&mut w) > while_reserved);
    row(&w, &txid);

    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    w.sync(online.clone()).await.unwrap();
    let tx = w
        .list_transactions(None, true)
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
        .create_utxos_begin(online.clone(), true, None, None, 1, false)
        .await
        .unwrap();
    let txid = txid_of(&psbt);
    let inputs: Vec<Outpoint> = reserved(&w).into_iter().map(Outpoint::from).collect();
    assert_eq!(row(&w, &txid).inputs, inputs);
    assert_eq!(pending_types(&w), vec![WalletTransactionType::CreateUtxos]);
    w.abort_pending_vanilla_tx(txid).unwrap();

    let send_inputs = inputs_of(&send_begin(&mut w, &online).await.unwrap());
    assert_eq!(send_inputs.len(), 1);
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
    let (mut w, online) = vanilla_wallet(1).await;
    let address = rcv_address();
    let first = w
        .drain_to_begin(online.clone(), address.clone(), false, 1)
        .await
        .unwrap();
    let second = w
        .drain_to_begin(online.clone(), address, false, 1)
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
    let txs = w.list_transactions(None, true).unwrap();
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
        .create_utxos_begin(online.clone(), false, Some(1), None, 1, false)
        .await
        .unwrap();
    let create_txid = txid_of(&create);
    let signed = w.sign_psbt(create, None).unwrap();
    w.create_utxos_end(online.clone(), signed, false)
        .await
        .unwrap();
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
