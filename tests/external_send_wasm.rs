//! External-send PREPARE lifecycle (browser/regtest).

#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::str::FromStr;

use rgb_lib_wasm::bitcoin::absolute::LockTime;
use rgb_lib_wasm::bitcoin::psbt::Psbt;
use rgb_lib_wasm::bitcoin::transaction::Version;
use rgb_lib_wasm::bitcoin::{
    Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness,
};
use rgb_lib_wasm::wallet::rust_only::{AssetColoringInfo, ColoringInfo};
use rgb_lib_wasm::wallet::{DatabaseType, Recipient, Wallet, WalletData, WitnessData};
use rgb_lib_wasm::{AssetSchema, Assignment, BitcoinNetwork, generate_keys};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

mod ext_common;
mod utils;
use ext_common::*;
use utils::*;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[wasm_bindgen_test]
async fn external_prepare_happy_path() {
    let (mut wallet, online, asset_id) = funded_sender(wallet_data("t1")).await;
    let (rid, b_script) = receiver_witness();
    let change = ScriptBuf::from_hex(
        &rgb_lib_wasm::bitcoin::Address::from_str(&wallet.get_address().unwrap())
            .unwrap()
            .assume_checked()
            .script_pubkey()
            .to_hex_string(),
    )
    .unwrap();
    let mut psbt = build_external_psbt(&mut wallet, &online, &asset_id, b_script, change).await;
    let fascia = color_external(&wallet, &mut psbt, contract_id(&asset_id));
    let prepared = wallet
        .prepare_external_rgb_send(recipients(&asset_id, &rid), &psbt, &fascia, 1)
        .await
        .expect("prepare");
    assert_eq!(prepared.txid, psbt.unsigned_tx.compute_txid().to_string());
}

#[wasm_bindgen_test]
async fn external_prepare_does_not_consume_fascia() {
    let (mut wallet, online, asset_id) = funded_sender(wallet_data("t2")).await;
    let (rid, b_script) = receiver_witness();
    let change = ScriptBuf::from_hex(
        &rgb_lib_wasm::bitcoin::Address::from_str(&wallet.get_address().unwrap())
            .unwrap()
            .assume_checked()
            .script_pubkey()
            .to_hex_string(),
    )
    .unwrap();
    let mut psbt = build_external_psbt(&mut wallet, &online, &asset_id, b_script, change).await;
    let fascia = color_external(&wallet, &mut psbt, contract_id(&asset_id));
    wallet
        .prepare_external_rgb_send(recipients(&asset_id, &rid), &psbt, &fascia, 1)
        .await
        .expect("prepare");
    assert_eq!(
        wallet.get_asset_balance(asset_id.clone()).unwrap().settled,
        1000,
        "prepare must not consume the sender allocation"
    );
}

#[wasm_bindgen_test]
async fn external_prepare_identical_retry_is_idempotent() {
    let (mut wallet, online, asset_id) = funded_sender(wallet_data("t3")).await;
    let (rid, b_script) = receiver_witness();
    let change = ScriptBuf::from_hex(
        &rgb_lib_wasm::bitcoin::Address::from_str(&wallet.get_address().unwrap())
            .unwrap()
            .assume_checked()
            .script_pubkey()
            .to_hex_string(),
    )
    .unwrap();
    let mut psbt = build_external_psbt(&mut wallet, &online, &asset_id, b_script, change).await;
    let fascia = color_external(&wallet, &mut psbt, contract_id(&asset_id));
    let first = wallet
        .prepare_external_rgb_send(recipients(&asset_id, &rid), &psbt, &fascia, 1)
        .await
        .expect("prepare");
    let again = wallet
        .prepare_external_rgb_send(recipients(&asset_id, &rid), &psbt, &fascia, 1)
        .await
        .expect("idempotent retry");
    assert_eq!(first.batch_transfer_idx, again.batch_transfer_idx);
}

#[wasm_bindgen_test]
async fn external_prepare_conflicting_same_txid_rejected() {
    let (mut wallet, online, asset_id) = funded_sender(wallet_data("t4")).await;
    let (rid, b_script) = receiver_witness();
    let change = ScriptBuf::from_hex(
        &rgb_lib_wasm::bitcoin::Address::from_str(&wallet.get_address().unwrap())
            .unwrap()
            .assume_checked()
            .script_pubkey()
            .to_hex_string(),
    )
    .unwrap();
    let mut psbt = build_external_psbt(&mut wallet, &online, &asset_id, b_script, change).await;
    let fascia = color_external(&wallet, &mut psbt, contract_id(&asset_id));
    wallet
        .prepare_external_rgb_send(recipients(&asset_id, &rid), &psbt, &fascia, 1)
        .await
        .expect("prepare");
    // Same txid, conflicting declared amount -> must be rejected.
    let mut conflicting = recipients(&asset_id, &rid);
    conflicting
        .get_mut(&asset_id)
        .unwrap()
        .get_mut(0)
        .unwrap()
        .assignment = Assignment::Fungible(TRADE_RGB + 1);
    let err = wallet
        .prepare_external_rgb_send(conflicting, &psbt, &fascia, 1)
        .await;
    assert!(
        err.is_err(),
        "conflicting same-txid prepare must be rejected"
    );
}

#[wasm_bindgen_test]
async fn external_prepare_mismatched_witness_rejected() {
    let (mut wallet, online, asset_id) = funded_sender(wallet_data("t5")).await;
    let (rid, b_script) = receiver_witness();
    let change = ScriptBuf::from_hex(
        &rgb_lib_wasm::bitcoin::Address::from_str(&wallet.get_address().unwrap())
            .unwrap()
            .assume_checked()
            .script_pubkey()
            .to_hex_string(),
    )
    .unwrap();
    let mut psbt = build_external_psbt(&mut wallet, &online, &asset_id, b_script, change).await;
    let fascia = color_external(&wallet, &mut psbt, contract_id(&asset_id));
    // Mutate the unsigned transaction so its txid no longer matches the fascia witness.
    psbt.unsigned_tx.output[A_CHANGE_VOUT as usize].value = Amount::from_sat(1);
    let err = wallet
        .prepare_external_rgb_send(recipients(&asset_id, &rid), &psbt, &fascia, 1)
        .await;
    assert!(err.is_err(), "mismatched fascia witness must be rejected");
}

#[wasm_bindgen_test]
async fn external_prepare_missing_commitment_rejected() {
    let (mut wallet, online, asset_id) = funded_sender(wallet_data("t6")).await;
    let (rid, b_script) = receiver_witness();
    let change = ScriptBuf::from_hex(
        &rgb_lib_wasm::bitcoin::Address::from_str(&wallet.get_address().unwrap())
            .unwrap()
            .assume_checked()
            .script_pubkey()
            .to_hex_string(),
    )
    .unwrap();
    let mut psbt = build_external_psbt(&mut wallet, &online, &asset_id, b_script, change).await;
    let fascia = color_external(&wallet, &mut psbt, contract_id(&asset_id));
    // Remove the RGB OP_RETURN commitment from the transaction.
    psbt.unsigned_tx.output.remove(0);
    let err = wallet
        .prepare_external_rgb_send(recipients(&asset_id, &rid), &psbt, &fascia, 1)
        .await;
    assert!(err.is_err(), "missing RGB commitment must be rejected");
}

#[wasm_bindgen_test]
async fn external_prepare_survives_reload() {
    let wd = wallet_data("t7");
    let (mut wallet, online, asset_id) = funded_sender(wd.clone()).await;
    let (rid, b_script) = receiver_witness();
    let change = ScriptBuf::from_hex(
        &rgb_lib_wasm::bitcoin::Address::from_str(&wallet.get_address().unwrap())
            .unwrap()
            .assume_checked()
            .script_pubkey()
            .to_hex_string(),
    )
    .unwrap();
    let mut psbt = build_external_psbt(&mut wallet, &online, &asset_id, b_script, change).await;
    let fascia = color_external(&wallet, &mut psbt, contract_id(&asset_id));
    let prepared = wallet
        .prepare_external_rgb_send(recipients(&asset_id, &rid), &psbt, &fascia, 1)
        .await
        .expect("prepare");
    drop(wallet);

    // Reopen the persisted wallet state via the production restore API.
    let mut wallet = Wallet::restore(wd).await.unwrap();
    let _online = wallet
        .go_online(false, ESPLORA_URL.to_string())
        .await
        .unwrap();
    let tx = sign_extract(&wallet, &psbt);
    assert_eq!(tx.compute_txid().to_string(), prepared.txid);
    let broadcast_txid = broadcast(&tx).await;
    assert_eq!(broadcast_txid, prepared.txid);
    wait_for_tx_observed(&prepared.txid).await;
    wallet
        .finalize_external_rgb_send(prepared.txid.clone(), &tx)
        .await
        .expect("finalize after reload must recover the prepared operation");
}
