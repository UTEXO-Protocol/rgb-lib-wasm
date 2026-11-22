use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

use rgb_lib_wasm::wallet::{
    AssetFilter, DatabaseType, ReceiveData, SinglesigKeys, WalletData, WalletDescriptors,
};
use rgb_lib_wasm::{AssetSchema, Assignment, BitcoinNetwork, WitnessVersion, generate_keys};
use rgb_lib_wasm_bindings::{
    WasmInvoice, WasmWallet, check_indexer_url, inspect_consignment, max_expiration_secs,
};

fn wallet() -> WasmWallet {
    wallet_with_schemas(vec![AssetSchema::Nia])
}

fn wallet_with_schemas(supported_schemas: Vec<AssetSchema>) -> WasmWallet {
    let (wd, keys) = wallet_json(supported_schemas);
    WasmWallet::new(&wd, &keys).unwrap()
}

fn wallet_json(supported_schemas: Vec<AssetSchema>) -> (String, String) {
    let keys = generate_keys(BitcoinNetwork::Regtest, WitnessVersion::Taproot);
    let wd = WalletData {
        data_dir: "/tmp/rgb_bindings_test".to_string(),
        bitcoin_network: BitcoinNetwork::Regtest,
        database_type: DatabaseType::Sqlite,
        max_allocations_per_utxo: 5,
        supported_schemas,
        reuse_addresses: false,
        allow_reckless_schemas: false,
    };
    (
        serde_json::to_string(&wd).unwrap(),
        serde_json::to_string(&SinglesigKeys::from_keys(&keys, None)).unwrap(),
    )
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
        "skipConsistencyCheck":true,"failedWitnessReceiveGraceSecs":604800,"proxyAuthKeys":{},
        "vanillaSyncLookback":20}"#,
    )
    .unwrap();
    wallet.go_online(options).await.unwrap()
}

async fn full_sync(wallet: &WasmWallet, online: &JsValue) {
    for options in [
        r#"{"keychain":"Colored","strategy":"FullSync"}"#,
        r#"{"keychain":{"Vanilla":{"lookback":20}},"strategy":"FullSync"}"#,
    ] {
        let options = js_sys::JSON::parse(options).unwrap();
        wallet.sync(online.clone(), options).await.unwrap();
    }
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
        "proxyAuthKeys":{}}"#,
    )
    .unwrap();
    let e = err(wallet.go_online(options).await.unwrap_err());
    assert!(e.contains("failedWitnessReceiveGraceSecs"), "{e}");

    let options = js_sys::JSON::parse(
        r#"{"indexerConfig":{"Esplora":{"url":"http://127.0.0.1:1"}},"skipConsistencyCheck":true,
        "failedWitnessReceiveGraceSecs":604800,"proxyAuthKeys":{}}"#,
    )
    .unwrap();
    let e = err(wallet.go_online(options).await.unwrap_err());
    assert!(e.contains("vanillaSyncLookback"), "{e}");

    let options = js_sys::JSON::parse(
        r#"{"indexerConfig":{"Esplora":{"url":"http://127.0.0.1:1"}},"skipConsistencyCheck":true,
        "failedWitnessReceiveGraceSecs":604800,"proxyAuthKeys":{"rpc://127.0.0.1:3000/json-rpc":"not hex"},
        "vanillaSyncLookback":20}"#,
    )
    .unwrap();
    let e = err(wallet.go_online(options).await.unwrap_err());
    assert!(e.starts_with("Invalid proxy authentication key"), "{e}");

    let options = |chain_id: &str| {
        js_sys::JSON::parse(&format!(
            r#"{{"indexerConfig":{{"Esplora":{{"url":"http://127.0.0.1:8094/regtest/api"}}}},
            "skipConsistencyCheck":true,"failedWitnessReceiveGraceSecs":604800,"proxyAuthKeys":{{}},
            "vanillaSyncLookback":20,"evmChains":{{"{chain_id}":{{"rpcUrl":"http://127.0.0.1:8545","minMintLockBlocks":2,"maxMintLockBlocks":1000}}}}}}"#
        ))
        .unwrap()
    };
    wallet.go_online(options("31337")).await.unwrap();
    let e = err(wallet.go_online(options("1")).await.unwrap_err());
    assert!(e.starts_with("EVM chain ID mismatch"), "{e}");
}

#[wasm_bindgen_test]
async fn save_new_asset_reads_arguments() {
    let wallet = wallet();
    let txid = "00".repeat(32);
    let e = err(wallet
        .save_new_asset(JsValue::NULL, &[0u8; 4], &txid)
        .await
        .unwrap_err());
    assert!(e.starts_with("Invalid Online object:"), "{e}");
}

#[wasm_bindgen_test]
fn get_received_mint_is_null_when_unknown() {
    assert!(wallet().get_received_mint("unknown").unwrap().is_null());
}

#[wasm_bindgen_test]
fn get_burn_consignment_fails_for_unknown_asset() {
    let e = err(wallet()
        .get_burn_consignment("rgb:unknown", &"00".repeat(32))
        .unwrap_err());
    assert!(e.starts_with("Asset with id rgb:unknown not found"), "{e}");
}

#[wasm_bindgen_test]
fn list_asset_media_fails_for_unknown_asset() {
    let e = err(wallet().list_asset_media("rgb:unknown").unwrap_err());
    assert!(e.starts_with("Asset with id rgb:unknown not found"), "{e}");
}

#[wasm_bindgen_test]
fn get_keys_hides_mnemonic() {
    let (wd, keys) = wallet_json(vec![AssetSchema::Nia]);
    let mut keys: SinglesigKeys = serde_json::from_str(&keys).unwrap();
    keys.vanilla_keychain = Some(1);
    let wallet = WasmWallet::new(&wd, &serde_json::to_string(&keys).unwrap()).unwrap();
    let data = wallet.get_keys().unwrap();
    let mnemonic = js_sys::Reflect::get(&data, &JsValue::from_str("mnemonic")).unwrap();
    assert!(mnemonic.is_null(), "getKeys returns the mnemonic");
    keys.mnemonic = None;
    assert_eq!(
        String::from(js_sys::JSON::stringify(&data).unwrap()),
        serde_json::to_string(&keys).unwrap()
    );
}

#[wasm_bindgen_test]
fn keys_take_witness_version() {
    let keys = rgb_lib_wasm_bindings::generate_keys("regtest", "SegWitV0").unwrap();
    assert_eq!(
        prop(&keys, "witnessVersion").as_string().unwrap(),
        "SegWitV0"
    );
    let mnemonic = prop(&keys, "mnemonic").as_string().unwrap();
    let colored = |wv| {
        let keys = rgb_lib_wasm_bindings::restore_keys("regtest", &mnemonic, wv).unwrap();
        prop(&keys, "accountXpubColored").as_string().unwrap()
    };
    let expected = prop(&keys, "accountXpubColored").as_string().unwrap();
    assert_eq!(colored("segwitv0"), expected);
    assert_ne!(colored("Taproot"), expected);

    let e = err(rgb_lib_wasm_bindings::generate_keys("regtest", "nonsense").unwrap_err());
    assert_eq!(e, "Invalid witness version: nonsense");
}

#[wasm_bindgen_test]
fn get_descriptors_match_rust() {
    let (wd, keys) = wallet_json(vec![AssetSchema::Nia]);
    let wallet = WasmWallet::new(&wd, &keys).unwrap();
    let descriptors: WalletDescriptors =
        serde_wasm_bindgen::from_value(wallet.get_descriptors().unwrap()).unwrap();
    let rust = rgb_lib_wasm::Wallet::new(
        serde_json::from_str(&wd).unwrap(),
        serde_json::from_str(&keys).unwrap(),
    )
    .unwrap();
    assert_eq!(descriptors, rust.get_descriptors());
}

#[wasm_bindgen_test]
async fn load_restores_flushed_wallet() {
    let (wd, keys) = wallet_json(vec![AssetSchema::Nia]);
    let mut data: WalletData = serde_json::from_str(&wd).unwrap();
    data.data_dir = format!("/tmp/rgb_bindings_load-{}", js_sys::Date::now());
    let wd = serde_json::to_string(&data).unwrap();
    let wallet = WasmWallet::create(&wd, &keys).await.unwrap();
    wallet.flush().await.unwrap();
    let fingerprint = serde_json::from_str::<SinglesigKeys>(&keys)
        .unwrap()
        .master_fingerprint;

    let loaded = WasmWallet::load(&data.data_dir, "regtest", &fingerprint, None)
        .await
        .unwrap();
    assert_eq!(
        String::from(js_sys::JSON::stringify(&loaded.get_wallet_data().unwrap()).unwrap()),
        wd
    );
}

#[wasm_bindgen_test]
fn allow_reckless_schemas_opens_mainnet_ifa() {
    let base: WalletData =
        serde_json::from_str(&wallet_json(vec![AssetSchema::Nia, AssetSchema::Ifa]).0).unwrap();
    let keys = generate_keys(BitcoinNetwork::Mainnet, WitnessVersion::Taproot);
    let keys = serde_json::to_string(&SinglesigKeys::from_keys(&keys, None)).unwrap();
    let wd = WalletData {
        bitcoin_network: BitcoinNetwork::Mainnet,
        ..base
    };
    let mut json = serde_json::to_value(&wd).unwrap();
    json.as_object_mut()
        .unwrap()
        .remove("allowRecklessSchemas")
        .unwrap();
    assert_eq!(
        err(WasmWallet::new(&json.to_string(), &keys).err().unwrap()),
        "Cannot use Ifa schema on mainnet"
    );
    json["allowRecklessSchemas"] = true.into();
    assert!(WasmWallet::new(&json.to_string(), &keys).is_ok());
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

// The runner runs tests in reverse name order.
// This name makes this test run before `wallet_builds_after_trap`.
#[wasm_bindgen_test]
fn wasm_init_runs_after_trap() {
    rgb_lib_wasm_bindings::init();
    let trap: js_sys::Function = Closure::once_into_js(|| -> () {
        panic!("forced trap");
    })
    .unchecked_into();
    assert!(trap.call0(&JsValue::NULL).is_err());

    let init: js_sys::Function =
        Closure::once_into_js(rgb_lib_wasm_bindings::init).unchecked_into();
    assert!(
        init.call0(&JsValue::NULL).is_ok(),
        "init traps after a trap"
    );
}

#[wasm_bindgen_test]
async fn wallet_builds_after_trap() {
    let trap: js_sys::Function = Closure::once_into_js(|| -> () {
        panic!("forced trap");
    })
    .unchecked_into();
    assert!(trap.call0(&JsValue::NULL).is_err());

    let (json, keys) = wallet_json(vec![AssetSchema::Nia]);
    let (json_new, keys_new) = (json.clone(), keys.clone());
    let build: js_sys::Function =
        Closure::once_into_js(move || WasmWallet::new(&json_new, &keys_new).is_ok())
            .unchecked_into();
    assert_eq!(
        build.call0(&JsValue::NULL).ok().and_then(|v| v.as_bool()),
        Some(true),
        "no wallet after a trap"
    );
    assert!(WasmWallet::create(&json, &keys).await.is_ok());
}

#[wasm_bindgen_test]
fn sign_psbt_reads_sign_options() {
    use rgb_lib_wasm::bitcoin::{
        Amount, OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid, absolute::LockTime,
        psbt::Psbt, transaction::Version,
    };
    use std::str::FromStr;

    let w = wallet();
    let psbt = Psbt::from_unsigned_tx(Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::from_str(&"11".repeat(32)).unwrap(), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(1000),
            script_pubkey: ScriptBuf::new(),
        }],
    })
    .unwrap()
    .to_string();

    // BDK defaults refuse an input without its previous TX
    let e = err(w.sign_psbt(&psbt, None).unwrap_err());
    assert!(e.contains("Missing non-witness UTXO"), "{e}");

    // the option reaches BDK
    let opts = js_sys::JSON::parse(r#"{"trustWitnessUtxo":true}"#).unwrap();
    assert!(w.sign_psbt(&psbt, Some(opts.unchecked_into())).is_ok());

    let opts = js_sys::JSON::parse(r#"{"trustWitnessUtxo":"yes"}"#).unwrap();
    let e = err(w.sign_psbt(&psbt, Some(opts.unchecked_into())).unwrap_err());
    assert!(e.starts_with("Invalid SignOptions object"), "{e}");
}

#[wasm_bindgen_test]
fn max_expiration_secs_is_exposed() {
    assert_eq!(
        max_expiration_secs(),
        rgb_lib_wasm::wallet::MAX_EXPIRATION_SECS
    );
}

#[wasm_bindgen_test]
fn inspect_consignment_js() {
    let bytes = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/inspection/send.rgbc"
    ));
    let get = |v: &JsValue, key: &str| js_sys::Reflect::get(v, &JsValue::from_str(key)).unwrap();
    let error = |filter: JsValue, bytes: &[u8]| {
        inspect_consignment(bytes, filter)
            .unwrap_err()
            .as_string()
            .unwrap()
    };

    let inspection = inspect_consignment(bytes, JsValue::from_str("Terminals")).unwrap();
    assert_eq!(get(&inspection, "isTransfer"), JsValue::TRUE);
    assert_eq!(get(&inspection, "assetSchema"), JsValue::from_str("Nia"));
    let witnesses: js_sys::Array = get(&inspection, "witnesses").dyn_into().unwrap();
    assert_eq!(witnesses.length(), 1);

    let filter = js_sys::Object::new();
    js_sys::Reflect::set(&filter, &"Witness".into(), &"invalid".into()).unwrap();
    assert!(error(filter.into(), bytes).contains("Invalid TXID"));
    assert!(error(JsValue::from_str("All"), &[0]).contains("Invalid consignment"));
    assert!(error(JsValue::from_str("Bogus"), bytes).starts_with("Invalid bundle filter"));
}

async fn await_js(code: &str) -> JsValue {
    let promise: js_sys::Promise = js_sys::eval(code).unwrap().dyn_into().unwrap();
    wasm_bindgen_futures::JsFuture::from(promise).await.unwrap()
}

async fn helper_post(path: &str, body: &str) {
    await_js(&format!(
        "fetch('http://127.0.0.1:8080/{path}', {{method: 'POST', \
         headers: {{'Content-Type': 'application/json'}}, body: '{body}'}}).then(r => r.json())"
    ))
    .await;
}

async fn esplora_synced() {
    await_js(
        "(async () => {
            const height = (await (await fetch('http://127.0.0.1:8080/height')).json()).height;
            for (let i = 0; i < 120; i++) {
                const r = await fetch('http://127.0.0.1:8094/regtest/api/blocks/tip/height',
                    {cache: 'no-store'});
                if (r.ok && parseInt(await r.text(), 10) >= height) return;
                await new Promise(res => setTimeout(res, 500));
            }
            throw new Error('esplora did not sync');
        })()",
    )
    .await;
}

fn prop(v: &JsValue, key: &str) -> JsValue {
    js_sys::Reflect::get(v, &key.into()).unwrap()
}

async fn funded_wallet(supported_schemas: Vec<AssetSchema>) -> (WasmWallet, JsValue) {
    let wallet = wallet_with_schemas(supported_schemas);
    let online = online(&wallet).await;
    let address = wallet.get_address().unwrap();
    helper_post(
        "fund",
        &format!(r#"{{"address":"{address}","amount":"1.0"}}"#),
    )
    .await;
    esplora_synced().await;
    full_sync(&wallet, &online).await;
    let unsigned = wallet
        .create_utxos_begin(online.clone(), true, Some(5), None, 1, true, false)
        .await
        .unwrap();
    let signed = wallet.sign_psbt(&unsigned, None).unwrap();
    wallet
        .create_utxos_end(online.clone(), &signed)
        .await
        .unwrap();
    helper_post("mine", r#"{"blocks":1}"#).await;
    esplora_synced().await;
    full_sync(&wallet, &online).await;
    (wallet, online)
}

#[wasm_bindgen_test]
async fn issue_bfa_reports_supply_and_locations() {
    let (wallet, _online) = funded_wallet(vec![AssetSchema::Nia, AssetSchema::Bfa]).await;
    let unspents: js_sys::Array = wallet
        .list_unspents(JsValue::NULL, false, true)
        .await
        .unwrap()
        .dyn_into()
        .unwrap();
    assert!(unspents.length() > 0);
    assert!(unspents.iter().all(|u| {
        prop(&prop(&u, "utxo"), "derivationIndex")
            .as_f64()
            .is_some()
    }));
    let reject_list = "0x00000000000000000000000000000000000000b2";
    let asset = wallet
        .issue_asset_bfa(
            "BFA",
            "Bridged",
            0,
            1,
            31337,
            "0x00000000000000000000000000000000000000b1",
            Some(reject_list.to_string()),
            false,
            None,
            false,
        )
        .unwrap();
    let supply = prop(&asset, "knownCirculatingSupply");
    assert!(supply.is_bigint());
    assert!(supply.loose_eq(&JsValue::from(0u64)));
    let evm = prop(&prop(&asset, "rejectList"), "Evm");
    let chain_id = prop(&evm, "chainId");
    assert!(chain_id.is_bigint());
    assert!(chain_id.loose_eq(&JsValue::from(31337u64)));
    assert_eq!(
        prop(&evm, "address").as_string().as_deref(),
        Some(reject_list)
    );
    let bridge = "0x00000000000000000000000000000000000000b1";
    let bridge_evm = prop(&prop(&asset, "bridgeLocation"), "Evm");
    assert_eq!(
        prop(&bridge_evm, "address").as_string().as_deref(),
        Some(bridge)
    );
    let asset_id = prop(&asset, "assetId").as_string().unwrap();
    let metadata = wallet.get_asset_metadata(&asset_id).unwrap();
    assert_eq!(
        prop(&prop(&prop(&metadata, "bridgeLocation"), "Evm"), "address")
            .as_string()
            .as_deref(),
        Some(bridge)
    );
}

#[wasm_bindgen_test]
async fn inflate_begin_returns_result_object() {
    let (wallet, online) = funded_wallet(vec![AssetSchema::Nia, AssetSchema::Ifa]).await;
    let asset = wallet
        .issue_asset_ifa(
            "IFA",
            "Inflatable",
            0,
            serde_wasm_bindgen::to_value(&vec![600u64, 400]).unwrap(),
            serde_wasm_bindgen::to_value(&vec![100u64]).unwrap(),
            None,
            false,
            None,
            false,
        )
        .unwrap();
    let asset_id = prop(&asset, "assetId").as_string().unwrap();
    let amounts = || serde_wasm_bindgen::to_value(&vec![50u64]).unwrap();

    let res = wallet
        .inflate_begin(online.clone(), &asset_id, amounts(), 1, 1, true)
        .await
        .unwrap();
    assert!(prop(&res, "psbt").is_string());
    assert!(prop(&res, "batchTransferIdx").loose_eq(&JsValue::NULL));
    let details = prop(&res, "details");
    assert_eq!(prop(&details, "minConfirmations").as_f64(), Some(1.0));
    let fascia: js_sys::Array = prop(&details, "fascia").dyn_into().unwrap();
    assert!(fascia.length() > 0);
    assert!(prop(&details, "entropy").is_bigint());

    let res = wallet
        .inflate_begin(online.clone(), &asset_id, amounts(), 1, 1, false)
        .await
        .unwrap();
    assert!(prop(&res, "batchTransferIdx").as_f64().is_some());

    // the batch transfer scope lists the inflation with its unsigned PSBT
    let scope = js_sys::Object::new();
    js_sys::Reflect::set(
        &scope,
        &"BatchTransfer".into(),
        &prop(&res, "batchTransferIdx"),
    )
    .unwrap();
    let scoped: js_sys::Array = wallet
        .list_transfers(JsValue::from_str("AnyOrNone"), scope.into())
        .unwrap()
        .dyn_into()
        .unwrap();
    assert!(scoped.length() > 0);
    let begin_psbt = prop(&res, "psbt")
        .as_string()
        .unwrap()
        .parse::<rgb_lib_wasm::bitcoin::Psbt>()
        .unwrap()
        .serialize();
    for t in scoped.iter() {
        assert_eq!(
            prop(&t, "batchTransferIdx").as_f64(),
            prop(&res, "batchTransferIdx").as_f64()
        );
        assert_eq!(prop(&t, "kind").as_string().as_deref(), Some("Inflation"));
        let psbt: Vec<u8> = serde_wasm_bindgen::from_value(prop(&t, "psbt")).unwrap();
        assert_eq!(psbt, begin_psbt);
    }

    // the change of the inflation right is listed in an array
    let filter = serde_wasm_bindgen::to_value(&AssetFilter::Id(asset_id.clone())).unwrap();
    let transfers: js_sys::Array = wallet
        .list_transfers(filter, JsValue::from_str("All"))
        .unwrap()
        .dyn_into()
        .unwrap();
    let inflation = transfers
        .iter()
        .find(|t| prop(t, "kind").as_string().as_deref() == Some("Inflation"))
        .unwrap();
    let change_utxos: js_sys::Array = prop(&inflation, "changeUtxos").dyn_into().unwrap();
    assert_eq!(change_utxos.length(), 1);
    assert!(prop(&inflation, "changeUtxo").is_undefined());
}

#[wasm_bindgen_test]
async fn listing_sync_and_send_end_take_js_values() {
    let wallet = wallet();
    let online = online(&wallet).await;
    let pay = async || {
        let address = wallet.get_address().unwrap();
        helper_post(
            "fund",
            &format!(r#"{{"address":"{address}","amount":"0.001"}}"#),
        )
        .await;
        helper_post("mine", r#"{"blocks":1}"#).await;
        esplora_synced().await;
    };
    pay().await;
    let settled = |b: &JsValue| prop(&prop(b, "vanilla"), "settled");

    let balance = wallet.get_btc_balance(JsValue::NULL, true).await.unwrap();
    assert!(settled(&balance).loose_eq(&JsValue::from(0u64)));
    let e = err(wallet
        .get_btc_balance(JsValue::NULL, false)
        .await
        .unwrap_err());
    assert_eq!(e, "The Online object is needed to perform this operation");
    let balance = wallet.get_btc_balance(online.clone(), false).await.unwrap();
    assert!(settled(&balance).loose_eq(&JsValue::from(100_000u64)));
    // each listing call sees a new payment only when it syncs
    pay().await;
    for (skip_sync, expected) in [(true, 1), (false, 2)] {
        let unspents: js_sys::Array = wallet
            .list_unspents(online.clone(), false, skip_sync)
            .await
            .unwrap()
            .dyn_into()
            .unwrap();
        assert_eq!(unspents.length(), expected);
    }
    pay().await;
    for (online, skip_sync, expected) in [(JsValue::UNDEFINED, true, 2), (online.clone(), false, 3)]
    {
        let txs: js_sys::Array = wallet
            .list_transactions(online, skip_sync)
            .await
            .unwrap()
            .dyn_into()
            .unwrap();
        assert_eq!(txs.length(), expected);
    }

    let e = err(wallet
        .sync(online.clone(), JsValue::from_str("bogus"))
        .await
        .unwrap_err());
    assert!(e.starts_with("Invalid sync options"), "{e}");
    for options in [
        r#"{"keychain":{"Vanilla":{"lookback":20}},"strategy":"FastSync"}"#,
        r#"{"keychain":"Colored","strategy":"FullScan"}"#,
    ] {
        let options = js_sys::JSON::parse(options).unwrap();
        wallet.sync(online.clone(), options).await.unwrap();
    }

    let e = err(wallet
        .send_end(online.clone(), "not a psbt")
        .await
        .unwrap_err());
    assert!(e.starts_with("Invalid PSBT"), "{e}");
}

#[wasm_bindgen_test]
async fn check_indexer_url_reads_arguments() {
    const URL: &str = "http://127.0.0.1:8094/regtest/api";
    assert!(!err(check_indexer_url(URL, "Nope").await.unwrap_err()).is_empty());
    let e = err(check_indexer_url("http://127.0.0.1:1", "Regtest")
        .await
        .unwrap_err());
    assert!(e.contains("not a valid electrum nor esplora server"), "{e}");
    assert_eq!(
        check_indexer_url(URL, "Regtest").await.unwrap().as_string(),
        Some("Esplora".into())
    );
}
