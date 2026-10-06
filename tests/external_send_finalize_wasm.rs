//! External-send FINALIZE lifecycle (browser/regtest).

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

#[wasm_bindgen_test]
async fn external_finalize_modified_transaction_rejected() {
    let wd = wallet_data("t8");
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
    let mut tx = sign_extract(&wallet, &psbt);
    // Substitute a modified transaction (same txid input, changed output value).
    tx.output[A_CHANGE_VOUT as usize].value = Amount::from_sat(1);
    let (mut wallet, _online) = reopen(wd, wallet).await;
    let err = wallet.finalize_external_rgb_send(prepared.txid, &tx).await;
    assert!(err.is_err(), "modified transaction must be rejected");
}

#[wasm_bindgen_test]
async fn external_finalize_happy_path() {
    let wd = wallet_data("t9");
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
    let tx = sign_extract(&wallet, &psbt);
    assert_eq!(broadcast(&tx).await, prepared.txid);
    wait_for_tx_observed(&prepared.txid).await;
    let (mut wallet, online) = reopen(wd, wallet).await;
    let result = wallet
        .finalize_external_rgb_send(prepared.txid.clone(), &tx)
        .await
        .expect("finalize");
    assert_eq!(result.txid, prepared.txid);
    assert!(
        result.batch_transfer_idx > 0,
        "finalised transfer must advance the batch"
    );
    // Confirm the external transaction so the transfer leaves WaitingConfirmations;
    // the full 900 economic reconciliation belongs to the later settlement gate.
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    let _ = wallet.sync(online.clone()).await;
}

#[wasm_bindgen_test]
async fn external_finalize_identical_retry_is_idempotent() {
    let wd = wallet_data("t10");
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
    let tx = sign_extract(&wallet, &psbt);
    broadcast(&tx).await;
    wait_for_tx_observed(&prepared.txid).await;
    let (mut wallet, _online) = reopen(wd, wallet).await;
    let first = wallet
        .finalize_external_rgb_send(prepared.txid.clone(), &tx)
        .await
        .expect("finalize");
    let second = wallet
        .finalize_external_rgb_send(prepared.txid.clone(), &tx)
        .await
        .expect("idempotent finalize");
    assert_eq!(first.batch_transfer_idx, second.batch_transfer_idx);
}

#[wasm_bindgen_test]
async fn external_finalize_unrelated_operation_rejected() {
    let (mut wallet, online, asset_id) = funded_sender(wallet_data("t11")).await;
    let (_rid, b_script) = receiver_witness();
    let change = ScriptBuf::from_hex(
        &rgb_lib_wasm::bitcoin::Address::from_str(&wallet.get_address().unwrap())
            .unwrap()
            .assume_checked()
            .script_pubkey()
            .to_hex_string(),
    )
    .unwrap();
    let mut psbt = build_external_psbt(&mut wallet, &online, &asset_id, b_script, change).await;
    let _fascia = color_external(&wallet, &mut psbt, contract_id(&asset_id));
    // Finalize an operation that was never prepared.
    let tx = sign_extract(&wallet, &psbt);
    let err = wallet
        .finalize_external_rgb_send(tx.compute_txid().to_string(), &tx)
        .await;
    assert!(err.is_err(), "unrelated operation must be rejected");
}

#[wasm_bindgen_test]
async fn external_finalize_recovers_after_interrupted_state() {
    let wd = wallet_data("t12");
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
    let tx = sign_extract(&wallet, &psbt);
    broadcast(&tx).await;
    wait_for_tx_observed(&prepared.txid).await;

    // Interruption: drop the runtime after broadcast but before finalisation.
    wallet.flush().await.unwrap();
    drop(wallet);
    let mut wallet = Wallet::restore(wd).await.unwrap();
    let _online = wallet
        .go_online(false, ESPLORA_URL.to_string())
        .await
        .unwrap();
    // Recovery converges to exactly one finalised transfer.
    wallet
        .finalize_external_rgb_send(prepared.txid.clone(), &tx)
        .await
        .expect("recovered finalize");
    let second = wallet
        .finalize_external_rgb_send(prepared.txid.clone(), &tx)
        .await
        .expect("idempotent after recovery");
    assert_eq!(second.txid, prepared.txid);
}
