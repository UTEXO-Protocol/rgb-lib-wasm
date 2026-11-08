use wasm_bindgen::JsValue;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

use rgb_lib_wasm::wallet::{DatabaseType, ReceiveData, WalletData};
use rgb_lib_wasm::{AssetSchema, Assignment, BitcoinNetwork, generate_keys};
use rgb_lib_wasm_bindings::{WasmInvoice, WasmWallet};

fn wallet() -> WasmWallet {
    wallet_with_schemas(vec![AssetSchema::Nia])
}

fn wallet_with_schemas(supported_schemas: Vec<AssetSchema>) -> WasmWallet {
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
        supported_schemas,
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
    serde_wasm_bindgen::to_value(&vec!["rpc://127.0.0.1:3000/json-rpc"]).unwrap()
}

fn err(e: JsValue) -> String {
    e.as_string().unwrap()
}

async fn online(wallet: &WasmWallet) -> JsValue {
    let options = js_sys::JSON::parse(
        r#"{"indexerConfig":{"Esplora":{"url":"http://127.0.0.1:8094/regtest/api"}},
        "skipConsistencyCheck":true,"proxyAuthKeys":{}}"#,
    )
    .unwrap();
    wallet.go_online(options).await.unwrap()
}

#[wasm_bindgen_test]
async fn witness_receive_takes_number_bigint_and_string() {
    let wallet = wallet();
    let online = online(&wallet).await;
    let exp = now_secs() + 100_000;
    for v in [
        JsValue::from_f64(exp as f64),
        JsValue::from(exp),
        JsValue::from_str(&exp.to_string()),
    ] {
        let data = wallet
            .witness_receive(online.clone(), None, any(), v, endpoints(), 1, None)
            .await
            .unwrap();
        let data: ReceiveData = serde_wasm_bindgen::from_value(data).unwrap();
        assert_eq!(data.expiration_timestamp, exp);
        assert_eq!(data.public_key.len(), 64);
        let invoice = WasmInvoice::new(&format!("{}&foo=bar", data.invoice)).unwrap();
        let invoice_data = invoice.invoice_data().unwrap();
        let get = |k| js_sys::Reflect::get(&invoice_data, &JsValue::from_str(k)).unwrap();
        assert_eq!(get("expirationTimestamp"), JsValue::from(exp));
        let params = js_sys::JSON::stringify(&get("unknownQueryParams")).unwrap();
        assert_eq!(String::from(params), r#"{"foo":"bar"}"#);
    }
}

#[wasm_bindgen_test]
async fn receive_rejects_malformed_expiration() {
    let wallet = wallet();
    let e = err(wallet
        .witness_receive(
            JsValue::NULL,
            None,
            any(),
            JsValue::from_str("soon"),
            endpoints(),
            1,
            None,
        )
        .await
        .unwrap_err());
    assert!(e.starts_with("Invalid expiration timestamp:"), "{e}");
}

#[wasm_bindgen_test]
async fn receive_takes_an_optional_online_and_an_encryption() {
    let wallet = wallet();
    let exp = JsValue::from_f64((now_secs() + 100_000) as f64);
    // the invoice is reserved on the proxy, so the wallet needs to be online
    for online in [JsValue::NULL, JsValue::UNDEFINED] {
        let e = err(wallet
            .witness_receive(online, None, any(), exp.clone(), endpoints(), 1, None)
            .await
            .unwrap_err());
        assert_eq!(e, "The Online object is needed to perform this operation");
    }
    let online = online(&wallet).await;
    let e = err(wallet
        .witness_receive(
            online.clone(),
            None,
            any(),
            exp.clone(),
            endpoints(),
            1,
            Some("Sometimes".to_string()),
        )
        .await
        .unwrap_err());
    assert_eq!(e, "Invalid encryption: Sometimes");
    for encryption in ["Required", "Unsupported", "Optional"] {
        wallet
            .witness_receive(
                online.clone(),
                None,
                any(),
                exp.clone(),
                endpoints(),
                1,
                Some(encryption.to_string()),
            )
            .await
            .unwrap();
    }
}

#[wasm_bindgen_test]
async fn go_online_reads_options_object() {
    let wallet = wallet_with_schemas(vec![AssetSchema::Bfa]);
    let options = js_sys::JSON::parse(
        r#"{"indexerConfig":{"Esplora":{"url":"http://127.0.0.1:1"}},"skipConsistencyCheck":true,
        "proxyAuthKeys":{"rpc://127.0.0.1:3000/json-rpc":"not hex"}}"#,
    )
    .unwrap();
    let e = err(wallet.go_online(options).await.unwrap_err());
    assert!(e.starts_with("Invalid proxy authentication key"), "{e}");

    let options = |chain_id: &str| {
        js_sys::JSON::parse(&format!(
            r#"{{"indexerConfig":{{"Esplora":{{"url":"http://127.0.0.1:8094/regtest/api"}}}},
            "skipConsistencyCheck":true,"proxyAuthKeys":{{}},
            "evmChains":{{"{chain_id}":{{"rpcUrl":"http://127.0.0.1:8545","minMintLockBlocks":2,"maxMintLockBlocks":1000}}}}}}"#
        ))
        .unwrap()
    };
    wallet.go_online(options("31337")).await.unwrap();
    let e = err(wallet.go_online(options("1")).await.unwrap_err());
    assert!(e.starts_with("EVM chain ID mismatch"), "{e}");
}

#[wasm_bindgen_test]
fn get_received_mint_is_null_when_unknown() {
    assert!(wallet().get_received_mint("unknown").unwrap().is_null());
}

#[wasm_bindgen_test]
fn pending_vanilla_txs_list_and_abort() {
    let wallet = wallet();
    let pending = js_sys::Array::from(&wallet.list_pending_vanilla_txs().unwrap());
    assert_eq!(pending.length(), 0);
    let e = err(wallet
        .abort_pending_vanilla_tx(&"0".repeat(64))
        .unwrap_err());
    assert_eq!(e, "Pending vanilla TX cannot be aborted");
}
