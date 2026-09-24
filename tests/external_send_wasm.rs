//! Executable browser/regtest proof of the generic externally constructed RGB
//! Layer-1 send lifecycle:
//!   prepare_external_rgb_send  (no consume, no broadcast, no sign)
//!   finalize_external_rgb_send (exact tx observed, records, consumes)
//!
//! The collaborative transaction is assembled OUTSIDE the wallet's normal
//! send_end flow: sender coloured UTXO + a receiver witness output + sender
//! change, coloured with the public `color_psbt` API, signed with the wallet's
//! `sign_psbt` and broadcast through Esplora.
//!
//! Opt-in via the integration harness (`REGTEST_HELPER_URL`/`ESPLORA_URL`).
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

mod utils;
use utils::*;

const TRADE_RGB: u64 = 100;
const CHANGE_RGB: u64 = 900;
const RGB_LEG_SATS: u64 = 2_000;
const MINER_FEE: u64 = 1_000;
const STATIC_BLINDING: u64 = 777;
const B_RECV_VOUT: u32 = 1;
const A_CHANGE_VOUT: u32 = 2;

fn transport_endpoint() -> String {
    format!("rpc://{}", PROXY_URL.trim_start_matches("http://"))
}

fn wallet_data(prefix: &str) -> WalletData {
    let keys = generate_keys(BitcoinNetwork::Regtest);
    WalletData {
        data_dir: format!("/tmp/rgb_ext_{prefix}"),
        bitcoin_network: BitcoinNetwork::Regtest,
        database_type: DatabaseType::Sqlite,
        max_allocations_per_utxo: 5,
        account_xpub_vanilla: keys.account_xpub_vanilla,
        account_xpub_colored: keys.account_xpub_colored,
        mnemonic: Some(keys.mnemonic),
        master_fingerprint: keys.master_fingerprint,
        vanilla_keychain: None,
        supported_schemas: vec![AssetSchema::Nia],
        reuse_addresses: false,
    }
}

async fn funded_sender(wd: WalletData) -> (Wallet, rgb_lib_wasm::wallet::Online, String) {
    let mut wallet = Wallet::new(wd).unwrap();
    let online = wallet
        .go_online(false, ESPLORA_URL.to_string())
        .await
        .unwrap();
    let addr = wallet.get_address().unwrap();
    fund_address(&addr, "1.0").await;
    mine_blocks(6).await;
    wait_for_esplora_sync().await;
    for _ in 0..60 {
        wallet.sync(online.clone()).await.unwrap();
        let unspents = wallet
            .list_unspents(Some(online.clone()), false, true)
            .unwrap();
        if unspents
            .iter()
            .any(|u| u.utxo.exists && u.utxo.btc_amount > 0)
        {
            break;
        }
        sleep_ms(1000).await;
    }
    let unsigned = wallet
        .create_utxos_begin(online.clone(), true, Some(5), Some(100_000), 1, true)
        .await
        .unwrap();
    let signed = wallet.sign_psbt(unsigned, None).unwrap();
    wallet
        .create_utxos_end(online.clone(), signed, true)
        .await
        .unwrap();
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();

    let asset = wallet
        .issue_asset_nia(
            "EXTS".to_string(),
            "External Send".to_string(),
            0,
            vec![1000],
        )
        .unwrap();
    mine_blocks(6).await;
    wait_for_esplora_sync().await;
    for _ in 0..60 {
        wallet.sync(online.clone()).await.unwrap();
        if wallet
            .get_asset_balance(asset.asset_id.clone())
            .map(|b| b.settled == 1000)
            .unwrap_or(false)
        {
            break;
        }
        sleep_ms(1000).await;
    }
    assert_eq!(
        wallet
            .get_asset_balance(asset.asset_id.clone())
            .unwrap()
            .settled,
        1000,
        "sender must hold 1000 settled units"
    );
    (wallet, online, asset.asset_id)
}

/// Receiver witness receive intent + its decoded witness script.
fn receiver_witness() -> (String, ScriptBuf) {
    let wd = wallet_data("recv");
    let mut wallet = Wallet::new(wd).unwrap();
    // Generic (asset-agnostic) witness receive: creates a witness vout at a
    // fresh receiver address; the asset is bound by the sender's recipient map.
    let data = wallet
        .witness_receive(
            None,
            Assignment::Fungible(TRADE_RGB),
            None,
            vec![transport_endpoint()],
            1,
        )
        .unwrap();
    let script =
        match rgbinvoice::XChainNet::<rgbinvoice::Beneficiary>::from_str(&data.recipient_id)
            .expect("recipient id")
        {
            rgbinvoice::XChainNet::BitcoinRegtest(rgbinvoice::Beneficiary::WitnessVout(p, _)) => {
                (*p).to_script()
            }
            other => panic!("expected regtest witness recipient, got {other:?}"),
        };
    (data.recipient_id, script)
}

#[derive(serde::Deserialize)]
struct EsploraVout {
    scriptpubkey: String,
    value: u64,
}
#[derive(serde::Deserialize)]
struct EsploraTx {
    vout: Vec<EsploraVout>,
}

async fn fetch_tx_hex(txid: &str) -> String {
    let client = reqwest::Client::new();
    client
        .get(format!("{ESPLORA_URL}/tx/{txid}/hex"))
        .send()
        .await
        .expect("tx hex request")
        .text()
        .await
        .expect("tx hex body")
}

async fn fetch_txout(txid: &str, vout: u32) -> (ScriptBuf, u64) {
    let client = reqwest::Client::new();
    let tx: EsploraTx = client
        .get(format!("{ESPLORA_URL}/tx/{txid}"))
        .send()
        .await
        .expect("tx request")
        .json()
        .await
        .expect("tx json");
    let out = &tx.vout[vout as usize];
    (
        ScriptBuf::from_hex(&out.scriptpubkey).expect("script hex"),
        out.value,
    )
}

async fn broadcast(tx: &Transaction) -> String {
    use rgb_lib_wasm::bitcoin::consensus::encode::serialize_hex;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{ESPLORA_URL}/tx"))
        .body(serialize_hex(tx))
        .send()
        .await
        .expect("broadcast request");
    assert!(
        resp.status().is_success(),
        "broadcast failed: {}",
        resp.text().await.unwrap_or_default()
    );
    resp.text()
        .await
        .expect("broadcast txid")
        .trim()
        .to_string()
}

async fn wait_for_tx_observed(txid: &str) {
    let client = reqwest::Client::new();
    for _ in 0..120 {
        if let Ok(resp) = client
            .get(format!("{ESPLORA_URL}/tx/{txid}/status"))
            .send()
            .await
        {
            if resp.status().is_success() {
                return;
            }
        }
        sleep_ms(500).await;
    }
    panic!("esplora did not observe {txid}");
}

/// Sender's first coloured UTXO carrying the full 1000 units.
async fn sender_colored_utxo(
    wallet: &mut Wallet,
    _online: &rgb_lib_wasm::wallet::Online,
    asset_id: &str,
) -> (OutPoint, ScriptBuf, u64) {
    let unspents = wallet.list_unspents(None, false, true).unwrap();
    let u = unspents
        .iter()
        .find(|u| {
            u.utxo.exists
                && u.rgb_allocations.iter().any(|a| {
                    a.asset_id.as_deref() == Some(asset_id)
                        && matches!(a.assignment, Assignment::Fungible(n) if n >= TRADE_RGB + CHANGE_RGB)
                })
        })
        .expect("a coloured UTXO with at least 1000 units");
    let (script, _esplora_value) = fetch_txout(&u.utxo.outpoint.txid, u.utxo.outpoint.vout).await;
    let value = u.utxo.btc_amount;
    assert!(
        value >= RGB_LEG_SATS + MINER_FEE,
        "coloured UTXO must carry enough sats (got {value})"
    );
    (
        OutPoint {
            txid: Txid::from_str(&u.utxo.outpoint.txid).unwrap(),
            vout: u.utxo.outpoint.vout,
        },
        script,
        value,
    )
}

/// Build the externally-assembled collaborative unsigned PSBT:
/// vout 0 = OP_RETURN, vout 1 = receiver witness, vout 2 = sender change.
async fn build_external_psbt(
    wallet: &mut Wallet,
    online: &rgb_lib_wasm::wallet::Online,
    asset_id: &str,
    b_script: ScriptBuf,
    a_change_script: ScriptBuf,
) -> Psbt {
    let (outpoint, prev_script, prev_value) = sender_colored_utxo(wallet, online, asset_id).await;
    let prev_tx_hex = fetch_tx_hex(&outpoint.txid.to_string()).await;
    let prev_tx: Transaction =
        rgb_lib_wasm::bitcoin::consensus::deserialize(&hex::decode(prev_tx_hex).unwrap()).unwrap();

    let change_value = prev_value - RGB_LEG_SATS - MINER_FEE;
    let tx = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: outpoint,
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        output: vec![
            TxOut {
                value: Amount::ZERO,
                script_pubkey: ScriptBuf::new_op_return([]),
            },
            TxOut {
                value: Amount::from_sat(RGB_LEG_SATS),
                script_pubkey: b_script,
            },
            TxOut {
                value: Amount::from_sat(change_value),
                script_pubkey: a_change_script,
            },
        ],
    };
    let mut psbt = Psbt::from_unsigned_tx(tx).expect("psbt");
    psbt.inputs[0].witness_utxo = Some(TxOut {
        value: Amount::from_sat(prev_value),
        script_pubkey: prev_script,
    });
    psbt.inputs[0].non_witness_utxo = Some(prev_tx);
    psbt
}

/// Colour the collaborative PSBT (public API), returning the fascia.
fn color_external(
    wallet: &Wallet,
    psbt: &mut Psbt,
    contract_id: rgb_lib_wasm::ContractId,
) -> rgb_lib_wasm::Fascia {
    let mut output_map = HashMap::new();
    // opreturn_first + P2TR present => color_psbt key = actual vout - 1.
    output_map.insert(B_RECV_VOUT - 1, TRADE_RGB);
    output_map.insert(A_CHANGE_VOUT - 1, CHANGE_RGB);
    let mut asset_info_map = HashMap::new();
    asset_info_map.insert(
        contract_id,
        AssetColoringInfo {
            output_map,
            static_blinding: Some(STATIC_BLINDING),
        },
    );
    let (fascia, _beneficiaries) = wallet
        .color_psbt(
            psbt,
            ColoringInfo {
                asset_info_map,
                static_blinding: Some(STATIC_BLINDING),
                nonce: None,
            },
        )
        .expect("color_psbt");
    fascia
}

fn recipients(asset_id: &str, b_rid: &str) -> HashMap<String, Vec<Recipient>> {
    let mut map = HashMap::new();
    map.insert(
        asset_id.to_string(),
        vec![Recipient {
            recipient_id: b_rid.to_string(),
            witness_data: Some(WitnessData {
                amount_sat: RGB_LEG_SATS,
                blinding: None,
            }),
            assignment: Assignment::Fungible(TRADE_RGB),
            transport_endpoints: vec![transport_endpoint()],
        }],
    );
    map
}

fn sign_extract(wallet: &Wallet, psbt: &Psbt) -> Transaction {
    let signed = wallet.sign_psbt(psbt.to_string(), None).expect("sign_psbt");
    let signed_psbt = Psbt::from_str(&signed).expect("signed psbt");
    signed_psbt.extract_tx().expect("extract_tx")
}

fn contract_id(asset_id: &str) -> rgb_lib_wasm::ContractId {
    rgb_lib_wasm::ContractId::from_str(asset_id).expect("contract id")
}

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

#[wasm_bindgen_test]
async fn external_finalize_modified_transaction_rejected() {
    let (mut wallet, online, asset_id) = funded_sender(wallet_data("t8")).await;
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
    let err = wallet.finalize_external_rgb_send(prepared.txid, &tx).await;
    assert!(err.is_err(), "modified transaction must be rejected");
}

#[wasm_bindgen_test]
async fn external_finalize_happy_path() {
    let (mut wallet, online, asset_id) = funded_sender(wallet_data("t9")).await;
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
    let result = wallet
        .finalize_external_rgb_send(prepared.txid.clone(), &tx)
        .await
        .expect("finalize");
    assert_eq!(result.txid, prepared.txid);
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    for _ in 0..60 {
        wallet.sync(online.clone()).await.unwrap();
        let _ = wallet
            .refresh(online.clone(), Some(asset_id.clone()), vec![], false)
            .await;
        if wallet.get_asset_balance(asset_id.clone()).unwrap().settled == CHANGE_RGB {
            return;
        }
        sleep_ms(1000).await;
    }
    assert_eq!(
        wallet.get_asset_balance(asset_id.clone()).unwrap().settled,
        CHANGE_RGB,
        "sender must settle to 900"
    );
}

#[wasm_bindgen_test]
async fn external_finalize_identical_retry_is_idempotent() {
    let (mut wallet, online, asset_id) = funded_sender(wallet_data("t10")).await;
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
