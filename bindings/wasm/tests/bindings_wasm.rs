use wasm_bindgen::JsValue;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

use rgb_lib_wasm::wallet::{DatabaseType, ReceiveData, WalletData};
use rgb_lib_wasm::{AssetSchema, Assignment, BitcoinNetwork, generate_keys};
use rgb_lib_wasm_bindings::WasmWallet;

fn wallet() -> WasmWallet {
    let keys = generate_keys(BitcoinNetwork::Regtest);
    let wd = WalletData {
        data_dir: "/tmp/rgb_bindings_test".to_string(),
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
    };
    WasmWallet::new(&serde_json::to_string(&wd).unwrap()).unwrap()
}

fn now_secs() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64
}

fn any() -> JsValue {
    serde_wasm_bindgen::to_value(&Assignment::Any).unwrap()
}

fn endpoints() -> JsValue {
    serde_wasm_bindgen::to_value(&vec!["rpc://127.0.0.1"]).unwrap()
}

fn err(e: JsValue) -> String {
    e.as_string().unwrap()
}

#[wasm_bindgen_test]
fn witness_receive_takes_number_bigint_and_string() {
    let wallet = wallet();
    let exp = now_secs() + 100_000;
    for v in [
        JsValue::from_f64(exp as f64),
        JsValue::from(exp),
        JsValue::from_str(&exp.to_string()),
    ] {
        let data = wallet
            .witness_receive(None, any(), v, endpoints(), 1)
            .unwrap();
        let data: ReceiveData = serde_wasm_bindgen::from_value(data).unwrap();
        assert_eq!(data.expiration_timestamp, exp);
    }
}

#[wasm_bindgen_test]
fn receive_rejects_malformed_expiration() {
    let e = err(wallet()
        .witness_receive(None, any(), JsValue::from_str("soon"), endpoints(), 1)
        .unwrap_err());
    assert!(e.starts_with("Invalid expiration timestamp:"), "{e}");
}

#[wasm_bindgen_test]
async fn go_online_reads_options_object() {
    let wallet = wallet();
    let options = js_sys::JSON::parse(
        r#"{"indexerConfig":{"Esplora":{"url":"http://127.0.0.1:1"}},"skipConsistencyCheck":true,
        "proxyAuthKeys":{"rpc://127.0.0.1:3000/json-rpc":"not hex"}}"#,
    )
    .unwrap();
    let e = err(wallet.go_online(options).await.unwrap_err());
    assert!(e.starts_with("Invalid proxy authentication key"), "{e}");
}
