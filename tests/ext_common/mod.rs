//! Shared harness for the external-send browser/regtest suites.

#![allow(clippy::result_large_err)]
#![allow(dead_code)]

use std::collections::HashMap;
use std::str::FromStr;

use crate::utils::*;
use rgb_lib_wasm::bitcoin::absolute::LockTime;
use rgb_lib_wasm::bitcoin::psbt::Psbt;
use rgb_lib_wasm::bitcoin::transaction::Version;
use rgb_lib_wasm::bitcoin::{
    Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness,
};
use rgb_lib_wasm::wallet::rust_only::{AssetColoringInfo, ColoringInfo};
use rgb_lib_wasm::wallet::{DatabaseType, Recipient, Wallet, WalletData, WitnessData};
use rgb_lib_wasm::{AssetSchema, Assignment, BitcoinNetwork, generate_keys};

pub const TRADE_RGB: u64 = 100;
pub const CHANGE_RGB: u64 = 900;
pub const RGB_LEG_SATS: u64 = 2_000;
pub const MINER_FEE: u64 = 1_000;
pub const STATIC_BLINDING: u64 = 777;
pub const B_RECV_VOUT: u32 = 1;
pub const A_CHANGE_VOUT: u32 = 2;

pub fn transport_endpoint() -> String {
    format!("rpc://{}", PROXY_URL.trim_start_matches("http://"))
}

pub fn wallet_data(prefix: &str) -> WalletData {
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

pub async fn funded_sender(wd: WalletData) -> (Wallet, rgb_lib_wasm::wallet::Online, String) {
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
pub fn receiver_witness() -> (String, ScriptBuf) {
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

pub async fn fetch_tx_hex(txid: &str) -> String {
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

pub async fn fetch_txout(txid: &str, vout: u32) -> (ScriptBuf, u64) {
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

pub async fn broadcast(tx: &Transaction) -> String {
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

pub async fn wait_for_tx_observed(txid: &str) {
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
pub async fn sender_colored_utxo(
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
pub async fn build_external_psbt(
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
pub fn color_external(
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

pub fn recipients(asset_id: &str, b_rid: &str) -> HashMap<String, Vec<Recipient>> {
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

pub fn sign_extract(wallet: &Wallet, psbt: &Psbt) -> Transaction {
    let signed = wallet.sign_psbt(psbt.to_string(), None).expect("sign_psbt");
    let signed_psbt = Psbt::from_str(&signed).expect("signed psbt");
    signed_psbt.extract_tx().expect("extract_tx")
}

pub fn contract_id(asset_id: &str) -> rgb_lib_wasm::ContractId {
    rgb_lib_wasm::ContractId::from_str(asset_id).expect("contract id")
}

/// Persist, drop and reopen the wallet through the production restore API, then
/// go back online. Mirrors a browser page/runtime reconstruction.
pub async fn reopen(wd: WalletData, wallet: Wallet) -> (Wallet, rgb_lib_wasm::wallet::Online) {
    wallet.flush().await.unwrap();
    drop(wallet);
    let mut wallet = Wallet::restore(wd).await.unwrap();
    let online = wallet
        .go_online(false, ESPLORA_URL.to_string())
        .await
        .unwrap();
    (wallet, online)
}
