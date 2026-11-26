use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

use rgb_lib_wasm::bdk_wallet::bitcoin::secp256k1::{Secp256k1, SecretKey, rand::rngs::OsRng};
use rgb_lib_wasm::wallet::backup_store::{
    BACKUP_NAMESPACE, JsKvStore, KvStore, StoredValue, backup_key, decode_backup_blob,
};
use rgb_lib_wasm::wallet::vss::{VssBackupConfig, VssStore};
use rgb_lib_wasm::wallet::{
    AssetFilter, DatabaseType, ExportedContract, Online, ReceiveData, SinglesigKeys, TransferScope,
    WalletData, WalletDescriptors,
};
use rgb_lib_wasm::{AssetSchema, Assignment, BitcoinNetwork, WitnessVersion, generate_keys};
use rgb_lib_wasm_bindings::{
    JsAbortSignal, WasmInvoice, WasmWallet, check_indexer_url, exported_contract_to_js,
    fill_spv_proofs, inspect_consignment, inspect_fascia, max_expiration_secs,
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
    wallet.go_online(options, None).await.unwrap()
}

async fn full_sync(wallet: &WasmWallet, online: &JsValue) {
    for options in [
        r#"{"keychain":"Colored","strategy":"FullSync"}"#,
        r#"{"keychain":{"Vanilla":{"lookback":20}},"strategy":"FullSync"}"#,
    ] {
        let options = js_sys::JSON::parse(options).unwrap();
        wallet.sync(online.clone(), options, None).await.unwrap();
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
    let e = err(wallet.go_online(options, None).await.unwrap_err());
    assert!(e.contains("failedWitnessReceiveGraceSecs"), "{e}");

    let options = js_sys::JSON::parse(
        r#"{"indexerConfig":{"Esplora":{"url":"http://127.0.0.1:1"}},"skipConsistencyCheck":true,
        "failedWitnessReceiveGraceSecs":604800,"proxyAuthKeys":{}}"#,
    )
    .unwrap();
    let e = err(wallet.go_online(options, None).await.unwrap_err());
    assert!(e.contains("vanillaSyncLookback"), "{e}");

    let options = js_sys::JSON::parse(
        r#"{"indexerConfig":{"Esplora":{"url":"http://127.0.0.1:1"}},"skipConsistencyCheck":true,
        "failedWitnessReceiveGraceSecs":604800,"proxyAuthKeys":{"rpc://127.0.0.1:3000/json-rpc":"not hex"},
        "vanillaSyncLookback":20}"#,
    )
    .unwrap();
    let e = err(wallet.go_online(options, None).await.unwrap_err());
    assert!(e.starts_with("Invalid proxy authentication key"), "{e}");

    let options = |chain_id: &str| {
        js_sys::JSON::parse(&format!(
            r#"{{"indexerConfig":{{"Esplora":{{"url":"http://127.0.0.1:8094/regtest/api"}}}},
            "skipConsistencyCheck":true,"failedWitnessReceiveGraceSecs":604800,"proxyAuthKeys":{{}},
            "vanillaSyncLookback":20,"evmChains":{{"{chain_id}":{{"rpcUrl":"http://127.0.0.1:8545","minMintLockBlocks":2,"maxMintLockBlocks":1000}}}}}}"#
        ))
        .unwrap()
    };
    wallet.go_online(options("31337"), None).await.unwrap();
    let e = err(wallet.go_online(options("1"), None).await.unwrap_err());
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
fn get_received_mint_is_empty_when_unknown() {
    let mints: js_sys::Array = wallet()
        .get_received_mint("unknown")
        .unwrap()
        .dyn_into()
        .unwrap();
    assert_eq!(mints.length(), 0);
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
fn export_contract_fails_for_unknown_asset() {
    let e = err(wallet().export_contract("rgb:unknown").unwrap_err());
    assert!(e.starts_with("Asset with id rgb:unknown not found"), "{e}");
}

#[wasm_bindgen_test]
fn import_contract_fails_for_invalid_bytes() {
    let e = err(wallet()
        .import_contract(b"not a contract", vec![])
        .unwrap_err());
    assert_eq!(e, "Invalid contract");
}

#[wasm_bindgen_test]
fn exported_contract_to_js_gives_uint8_arrays() {
    let v = exported_contract_to_js(&ExportedContract {
        contract: vec![1, 2],
        media: vec![vec![3], vec![4, 5]],
    })
    .unwrap();
    let contract = prop(&v, "contract");
    assert!(contract.is_instance_of::<js_sys::Uint8Array>());
    assert_eq!(js_sys::Uint8Array::from(contract).to_vec(), vec![1, 2]);
    let media: js_sys::Array = prop(&v, "media").dyn_into().unwrap();
    assert_eq!(media.length(), 2);
    assert!(
        media
            .iter()
            .all(|m| m.is_instance_of::<js_sys::Uint8Array>())
    );
    let media: Vec<Vec<u8>> = media
        .iter()
        .map(|m| js_sys::Uint8Array::from(m).to_vec())
        .collect();
    assert_eq!(media, vec![vec![3], vec![4, 5]]);
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

#[wasm_bindgen_test]
fn inspect_fascia_js() {
    let psbt = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/inspection/fascia_send.psbt"
    ))
    .trim();
    let fascia = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/inspection/fascia_send.fascia"
    ));
    let expected: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/inspection/fascia_send.json"
    )))
    .unwrap();
    let get = |v: &JsValue, key: &str| js_sys::Reflect::get(v, &JsValue::from_str(key)).unwrap();

    let inspection = inspect_fascia(psbt, fascia, None, u64::MAX, u32::MAX, "Regtest").unwrap();
    assert_eq!(
        get(&inspection, "txid"),
        JsValue::from_str(expected["txid"].as_str().unwrap())
    );
    assert_eq!(
        get(&inspection, "commitmentHex"),
        JsValue::from_str(expected["commitment_hex"].as_str().unwrap())
    );
    let operations: js_sys::Array = get(&inspection, "operations").dyn_into().unwrap();
    assert_eq!(operations.length(), 1);

    let error = |result: Result<JsValue, JsValue>| result.unwrap_err().as_string().unwrap();
    assert!(
        error(inspect_fascia(
            psbt,
            &[0],
            None,
            u64::MAX,
            u32::MAX,
            "Regtest"
        ))
        .contains("Invalid fascia")
    );
    assert!(inspect_fascia(psbt, fascia, None, u64::MAX, u32::MAX, "Bogus").is_err());
    let w = wallet();
    let size = fascia.len() as u64;
    let too_large = format!("larger than the max allowed size of {} bytes", size - 1);
    assert!(
        error(inspect_fascia(
            psbt,
            fascia,
            None,
            size - 1,
            u32::MAX,
            "Regtest"
        ))
        .contains(&too_large)
    );
    inspect_fascia(psbt, fascia, None, size, u32::MAX, "Regtest").unwrap();
    assert_eq!(
        error(inspect_fascia(psbt, fascia, None, u64::MAX, 0, "Regtest")),
        "Error during RGB inspection: fascia bundle count 1 exceeds the maximum of 0"
    );
    inspect_fascia(psbt, fascia, None, u64::MAX, 1, "Regtest").unwrap();
    // the size check runs before the ownership check
    assert!(error(w.inspect_fascia(psbt, fascia, None, size - 1, u32::MAX)).contains(&too_large));
    assert!(
        error(w.inspect_fascia(psbt, fascia, None, u64::MAX, u32::MAX))
            .contains("found a PSBT input that does not belong to this wallet")
    );
    let mut non_segwit = psbt.parse::<rgb_lib_wasm::bitcoin::Psbt>().unwrap();
    non_segwit.inputs[0].witness_utxo = None;
    assert!(
        error(w.inspect_fascia(&non_segwit.to_string(), fascia, None, u64::MAX, u32::MAX))
            .contains("RGB-related PSBTs require segwit")
    );
}

#[wasm_bindgen_test]
fn inspect_psbt_js() {
    let w = wallet();
    let psbt = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/inspection/psbt_send.psbt"
    ));
    let expected: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/inspection/psbt_send.json"
    )))
    .unwrap();

    let v = w.inspect_psbt(psbt.trim()).unwrap();
    assert_eq!(
        prop(&v, "txid"),
        JsValue::from_str(expected["txid"].as_str().unwrap())
    );
    let outputs = js_sys::Array::from(&prop(&v, "outputs"));
    assert_eq!(prop(&outputs.get(0), "isOpReturn"), JsValue::TRUE);
    let total = prop(&v, "totalInputSat");
    assert!(total.is_bigint());
    assert!(total.loose_eq(&JsValue::from(
        expected["total_input_sat"].as_u64().unwrap()
    )));
    assert!(err(w.inspect_psbt("x").unwrap_err()).starts_with("Invalid PSBT"));
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
        .create_utxos_begin(
            online.clone(),
            true,
            false,
            Some(5),
            None,
            1,
            true,
            false,
            None,
        )
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
    let exported = wallet.export_contract(&asset_id).unwrap();
    let contract = prop(&exported, "contract");
    assert!(contract.is_instance_of::<js_sys::Uint8Array>());
    assert!(js_sys::Uint8Array::from(contract).length() > 0);
    let media: js_sys::Array = prop(&exported, "media").dyn_into().unwrap();
    assert_eq!(media.length(), 0);
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
        .inflate_begin(online.clone(), &asset_id, amounts(), 1, 1, true, None)
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
        .inflate_begin(online.clone(), &asset_id, amounts(), 1, 1, false, None)
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
        .sync(online.clone(), JsValue::from_str("bogus"), None)
        .await
        .unwrap_err());
    assert!(e.starts_with("Invalid sync options"), "{e}");
    for options in [
        r#"{"keychain":{"Vanilla":{"lookback":20}},"strategy":"FastSync"}"#,
        r#"{"keychain":"Colored","strategy":"FullScan"}"#,
    ] {
        let options = js_sys::JSON::parse(options).unwrap();
        wallet.sync(online.clone(), options, None).await.unwrap();
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

#[wasm_bindgen_test]
async fn fill_spv_proofs_reads_arguments() {
    use rgb_lib_wasm::{FileContent, RgbTransfer};
    const URL: &str = "http://127.0.0.1:8094/regtest/api";
    let get = |v: &JsValue, key: &str| js_sys::Reflect::get(v, &JsValue::from_str(key)).unwrap();
    let e = err(fill_spv_proofs(&[0], URL).await.unwrap_err());
    assert!(e.contains("Invalid consignment"), "{e}");

    // the fixture witnesses are unknown to the indexer
    let mut consignment = RgbTransfer::load(
        &include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/inspection/send.rgbc"
        ))[..],
    )
    .unwrap();
    for bw in consignment.bundles.iter_mut() {
        bw.spv_proof = None;
    }
    let mut bytes = vec![];
    consignment.save(&mut bytes).unwrap();
    let fill = fill_spv_proofs(&bytes, URL).await.unwrap();
    let without: js_sys::Array = get(&fill, "withoutProof").dyn_into().unwrap();
    assert_eq!(without.length(), consignment.bundles.len() as u32);
    for w in without.iter() {
        assert_eq!(
            get(&w, "details"),
            JsValue::from_str("TX is unknown or not mined")
        );
    }
    assert_eq!(get(&fill, "rewritten"), JsValue::FALSE);
    assert!(get(&fill, "consignment").is_null());

    // an indexer that proves every witness
    js_sys::eval(
        r#"globalThis.__origFetch = globalThis.fetch;
        globalThis.fetch = function (input, init) {
            const u = input instanceof Request ? input.url : String(input);
            if (u.includes('/merkle-proof')) {
                const resp = new Response('{"block_height":1,"merkle":[],"pos":0}', {status: 200});
                return Promise.resolve(Object.defineProperty(resp, 'url', {value: u}));
            }
            return globalThis.__origFetch(input, init);
        };"#,
    )
    .unwrap();
    let fill = fill_spv_proofs(&bytes, URL).await;
    js_sys::eval("globalThis.fetch = globalThis.__origFetch;").unwrap();
    let fill = fill.unwrap();
    assert_eq!(get(&fill, "rewritten"), JsValue::TRUE);
    let filled: js_sys::Uint8Array = get(&fill, "consignment").dyn_into().unwrap();
    let filled = RgbTransfer::load(&filled.to_vec()[..]).unwrap();
    for bw in filled.bundles.iter() {
        assert_eq!(bw.spv_proof.as_ref().unwrap().block_height.get(), 1);
    }
}

#[wasm_bindgen_test]
async fn one_step_calls_need_online() {
    let wallet = wallet();
    let online = || {
        serde_wasm_bindgen::to_value(&Online {
            id: 1,
            indexer_url: "http://127.0.0.1:1".to_string(),
        })
        .unwrap()
    };
    let map = || js_sys::Object::new().into();
    let e = err(wallet
        .send(
            online(),
            map(),
            false,
            1,
            1,
            JsValue::from(now_secs() + 3600),
            None,
        )
        .await
        .unwrap_err());
    assert!(e.starts_with("Wallet is offline"), "{e}");
    let e = err(wallet
        .send(
            online(),
            map(),
            false,
            1,
            1,
            JsValue::from_str("soon"),
            None,
        )
        .await
        .unwrap_err());
    assert!(e.starts_with("Invalid expiration timestamp:"), "{e}");
}

#[wasm_bindgen_test]
async fn one_step_calls_return_results() {
    let (wallet, online) = funded_wallet(vec![AssetSchema::Nia, AssetSchema::Ifa]).await;
    let created = wallet
        .create_utxos(online.clone(), false, true, Some(1), None, 1, true)
        .await
        .unwrap();
    assert_eq!(created, 1);
    let txid = wallet
        .send_btc(
            online.clone(),
            &wallet.get_address().unwrap(),
            1000,
            1,
            true,
        )
        .await
        .unwrap();
    txid.parse::<rgb_lib_wasm::bitcoin::Txid>().unwrap();

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
    let res = wallet
        .inflate(
            online.clone(),
            &asset_id,
            serde_wasm_bindgen::to_value(&vec![50u64]).unwrap(),
            1,
            1,
        )
        .await
        .unwrap();
    assert!(prop(&res, "txid").is_string());
    assert!(prop(&res, "batchTransferIdx").as_f64().is_some());
    assert!(prop(&res, "entropy").is_bigint());

    let txid = wallet
        .drain_to(
            online.clone(),
            &wallet_with_schemas(vec![AssetSchema::Nia])
                .get_address()
                .unwrap(),
            1,
        )
        .await
        .unwrap();
    txid.parse::<rgb_lib_wasm::bitcoin::Txid>().unwrap();
}

async fn funded_with(btc: &str) -> (WasmWallet, JsValue) {
    let wallet = wallet();
    let online = online(&wallet).await;
    let address = wallet.get_address().unwrap();
    helper_post(
        "fund",
        &format!(r#"{{"address":"{address}","amount":"{btc}"}}"#),
    )
    .await;
    esplora_synced().await;
    full_sync(&wallet, &online).await;
    (wallet, online)
}

fn insufficient(e: JsValue) -> (u64, u64) {
    let e = err(e);
    let rest = e
        .strip_prefix("Insufficient bitcoin funds: needed '")
        .unwrap_or_else(|| panic!("unexpected error: {e}"));
    let (needed, rest) = rest.split_once("', available '").unwrap();
    let available = rest.strip_suffix('\'').unwrap();
    (needed.parse().unwrap(), available.parse().unwrap())
}

fn pending_len(wallet: &WasmWallet) -> u32 {
    js_sys::Array::from(&wallet.list_pending_vanilla_txs().unwrap()).length()
}

#[wasm_bindgen_test]
async fn create_utxos_exact_partial_funds() {
    let (wallet, online) = funded_with("0.000026").await;
    let begin = wallet
        .create_utxos_begin(
            online.clone(),
            false,
            true,
            Some(5),
            Some(1000),
            1,
            true,
            false,
            None,
        )
        .await
        .unwrap_err();
    let one_step = wallet
        .create_utxos(online.clone(), false, true, Some(5), Some(1000), 1, true)
        .await
        .unwrap_err();
    for e in [begin, one_step] {
        let (needed, available) = insufficient(e);
        assert_eq!(available, 2600);
        assert!(needed > 5000);
    }
    assert_eq!(pending_len(&wallet), 0);
    let created = wallet
        .create_utxos(online, false, false, Some(5), Some(1000), 1, true)
        .await
        .unwrap();
    assert_eq!(created, 2);
}

#[wasm_bindgen_test]
async fn create_utxos_exact_short_by_fee() {
    let (wallet, online) = funded_with("0.0000205").await;
    let e = wallet
        .create_utxos_begin(
            online.clone(),
            false,
            true,
            Some(2),
            Some(1000),
            1,
            true,
            false,
            None,
        )
        .await
        .unwrap_err();
    let (needed, available) = insufficient(e);
    assert_eq!(available, 2050);
    assert!(needed > 2050);
    assert_eq!(pending_len(&wallet), 0);
    let created = wallet
        .create_utxos(online, false, false, Some(2), Some(1000), 1, true)
        .await
        .unwrap();
    assert_eq!(created, 1);
}

#[wasm_bindgen_test]
async fn refresh_reports_proxy_failures_to_js() {
    // Stub the `transfer.list` answers of the proxies on ports 2 to 5.
    js_sys::eval(
        r#"if (!globalThis.__lookupFetch) {
            globalThis.__lookupFetch = globalThis.fetch;
            globalThis.fetch = async function (input, init) {
                const orig = () => globalThis.__lookupFetch.call(this, input, init);
                if (!(input instanceof Request)) return orig();
                const stub = { '2': 'http', '3': 'jsonrpc', '4': 'invalid', '5': 'stall' }[new URL(input.url).port];
                if (!stub) return orig();
                let method = null;
                try { method = (await input.clone().json()).method; } catch (e) {}
                if (method !== 'transfer.list') return orig();
                const headers = { 'content-type': 'application/json' };
                let resp;
                if (stub === 'http') resp = new Response('unavailable', { status: 503 });
                else if (stub === 'jsonrpc') resp = new Response('{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"internal"}}', { status: 200, headers });
                else if (stub === 'invalid') resp = new Response('not valid json', { status: 200, headers });
                else {
                    // the body never ends; an abort errors it with the abort reason, as fetch does
                    const signal = input.signal;
                    resp = new Response(new ReadableStream({ start(c) { signal.addEventListener('abort', () => c.error(signal.reason)); } }), { status: 200, headers });
                }
                // reqwest parses the response URL
                Object.defineProperty(resp, 'url', { value: input.url });
                return resp;
            };
        }"#,
    )
    .unwrap();
    let wallet = wallet();
    let online = online(&wallet).await;
    let mut idxs = vec![];
    for port in 1..=6 {
        let eps = if port == 6 {
            endpoints()
        } else {
            let stub = format!("rpc://127.0.0.1:{port}/json-rpc");
            serde_wasm_bindgen::to_value(&vec![stub.as_str(), "rpc://127.0.0.1:3000/json-rpc"])
                .unwrap()
        };
        let data = wallet
            .witness_receive(
                online.clone(),
                None,
                any(),
                (now_secs() + 100_000).into(),
                eps,
                1,
                None,
            )
            .await
            .unwrap();
        idxs.push(prop(&data, "batchTransferIdx"));
    }

    let res = js_sys::Map::from(
        wallet
            .refresh(online, None, js_sys::Array::new().into(), true, None)
            .await
            .unwrap(),
    );
    let names = [
        "ProxyUnreachable",
        "ProxyHttpError",
        "ProxyJsonRpcError",
        "ProxyInvalidResponse",
        "ProxyTimeout",
    ];
    for (port, (idx, name)) in (1..).zip(idxs.iter().zip(names)) {
        let entry = res.get(idx);
        let failure = prop(&entry, "failure");
        let keys: Vec<String> = js_sys::Object::keys(&failure.clone().into())
            .iter()
            .map(|k| k.as_string().unwrap())
            .collect();
        assert_eq!(keys, vec![name]);
        let kind = prop(&failure, name);
        let endpoint = prop(&kind, "endpoint").as_string().unwrap();
        assert!(
            endpoint.ends_with(&format!("127.0.0.1:{port}/json-rpc")),
            "{endpoint}"
        );
        assert!(prop(&entry, "updatedStatus").is_undefined());
        match name {
            "ProxyHttpError" => assert_eq!(prop(&kind, "status").as_f64(), Some(503.0)),
            "ProxyJsonRpcError" => assert_eq!(prop(&kind, "code").as_f64(), Some(-32603.0)),
            _ => {}
        }
    }
    let entry = res.get(&idxs[5]);
    assert!(prop(&entry, "failure").is_undefined());
    assert!(prop(&entry, "updatedStatus").is_undefined());

    for idx in &idxs {
        let scope = js_sys::Object::new();
        js_sys::Reflect::set(&scope, &"BatchTransfer".into(), idx).unwrap();
        let transfers: js_sys::Array = wallet
            .list_transfers(JsValue::from_str("AnyOrNone"), scope.into())
            .unwrap()
            .dyn_into()
            .unwrap();
        assert!(transfers.length() > 0);
        for t in transfers.iter() {
            assert_eq!(
                prop(&t, "status").as_string().as_deref(),
                Some("WaitingCounterparty")
            );
        }
    }
}

const ABORT_URL: &str = "http://abort.invalid/api";

// Serve `ABORT_URL` from a stub that counts requests. With `stall`, it answers only on abort.
fn abort_stub(stall: bool) {
    js_sys::eval(&format!(
        r#"globalThis.__origFetch = globalThis.__origFetch || globalThis.fetch;
        globalThis.__stub = {{ count: 0, aborts: 0, stall: {stall} }};
        globalThis.fetch = function (input, init) {{
            const u = input instanceof Request ? input.url : String(input);
            if (!u.startsWith('{ABORT_URL}')) return globalThis.__origFetch(input, init);
            const s = globalThis.__stub;
            s.count++;
            const signal = (init && init.signal) || (input instanceof Request ? input.signal : null);
            if (s.stall) return new Promise((_, rej) => signal && signal.addEventListener('abort',
                () => {{ s.aborts++; rej(signal.reason); }}, {{ once: true }}));
            let body = '0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206';
            if (u.endsWith('/blocks')) body = JSON.stringify([{{ id: body, height: 0, version: 1,
                timestamp: 1296688602, tx_count: 1, size: 285, weight: 1140,
                merkle_root: '4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b',
                previousblockhash: null, mediantime: 1296688602, nonce: 2, bits: 545259519,
                difficulty: 0 }}]);
            else if (u.includes('/scripthash/')) body = '[]';
            const resp = new Response(body, {{ status: 200 }});
            return Promise.resolve(Object.defineProperty(resp, 'url', {{ value: u }}));
        }};"#
    ))
    .unwrap();
}

fn stub_restore() {
    js_sys::eval("globalThis.fetch = globalThis.__origFetch;").unwrap();
}

fn stub_set_stall(stall: bool) {
    js_sys::eval(&format!("globalThis.__stub.stall = {stall};")).unwrap();
}

fn stub_count(key: &str) -> f64 {
    js_sys::eval(&format!("globalThis.__stub.{key}"))
        .unwrap()
        .as_f64()
        .unwrap()
}

fn js_signal(code: &str) -> Option<JsAbortSignal> {
    Some(js_sys::eval(code).unwrap().unchecked_into())
}

fn abort_options() -> JsValue {
    js_sys::JSON::parse(&format!(
        r#"{{"indexerConfig":{{"Esplora":{{"url":"{ABORT_URL}"}}}},
        "skipConsistencyCheck":true,"failedWitnessReceiveGraceSecs":604800,"proxyAuthKeys":{{}},
        "vanillaSyncLookback":20}}"#
    ))
    .unwrap()
}

fn full_scan() -> JsValue {
    js_sys::JSON::parse(r#"{"keychain":"Colored","strategy":"FullScan"}"#).unwrap()
}

#[wasm_bindgen_test]
async fn signal_abort_controller_stops_go_online_in_flight() {
    abort_stub(true);
    let wallet = wallet();
    let signal = js_signal(
        "(() => { const c = new AbortController(); setTimeout(() => c.abort(), 300); \
         return c.signal; })()",
    );
    let start = js_sys::Date::now();
    let res = wallet.go_online(abort_options(), signal).await;
    assert_eq!(err(res.unwrap_err()), "Aborted");
    assert!(js_sys::Date::now() - start < 5000.0);
    assert!(stub_count("aborts") >= 1.0);
    // the borrow is released
    wallet.get_wallet_data().unwrap();
    stub_set_stall(false);
    let res = wallet.go_online(abort_options(), None).await;
    stub_restore();
    res.unwrap();
}

#[wasm_bindgen_test]
async fn signal_timeout_stops_sync() {
    abort_stub(false);
    let wallet = wallet();
    let online = wallet.go_online(abort_options(), None).await.unwrap();
    stub_set_stall(true);
    let res = wallet
        .sync(
            online.clone(),
            full_scan(),
            js_signal("AbortSignal.timeout(300)"),
        )
        .await;
    assert_eq!(err(res.unwrap_err()), "Aborted");
    stub_set_stall(false);
    let res = wallet.sync(online, full_scan(), None).await;
    stub_restore();
    res.unwrap();
}

#[wasm_bindgen_test]
async fn signal_already_aborted_stops_every_call() {
    use rgb_lib_wasm::{Encryption, wallet::Recipient};
    abort_stub(false);
    let wallet = wallet();
    let online = serde_wasm_bindgen::to_value(&Online {
        id: 1,
        indexer_url: ABORT_URL.to_string(),
    })
    .unwrap();
    let recipient = serde_wasm_bindgen::to_value(&Recipient {
        recipient_id: "id".to_string(),
        witness_data: None,
        assignment: Assignment::Fungible(1),
        transport_endpoints: vec![],
        public_key: None,
        encryption: Encryption::Optional,
        expiration_timestamp: None,
    })
    .unwrap();
    let fired = || js_signal("AbortSignal.abort()");
    let o = || online.clone();
    let empty = || JsValue::from(js_sys::Object::new());
    let amounts = || JsValue::from(js_sys::Array::of1(&JsValue::from(1)));
    let errs = vec![
        wallet.go_online(abort_options(), fired()).await.err(),
        wallet.sync(o(), full_scan(), fired()).await.err(),
        wallet
            .refresh(o(), None, js_sys::Array::new().into(), false, fired())
            .await
            .err(),
        wallet
            .create_utxos_begin(o(), false, false, None, None, 1, false, false, fired())
            .await
            .err(),
        wallet
            .send_begin(
                o(),
                empty(),
                false,
                1,
                1,
                0.into(),
                None,
                None,
                false,
                fired(),
            )
            .await
            .err(),
        wallet
            .send_btc_begin(o(), "addr", 1000, 1, false, false, fired())
            .await
            .err(),
        wallet
            .drain_to_begin(o(), "addr", 1, false, fired())
            .await
            .err(),
        wallet
            .inflate_begin(o(), "id", amounts(), 1, 1, false, fired())
            .await
            .err(),
        wallet
            .burn_begin(o(), "id", 1, None, 1, 1, false, fired())
            .await
            .err(),
        wallet
            .link_begin(o(), "id", "id", 1, 1, false, false, fired())
            .await
            .err(),
        wallet
            .mint_begin(o(), "id", recipient, 1, 1, 1, "rpc://x", fired())
            .await
            .err(),
    ];
    let count = stub_count("count");
    stub_restore();
    for e in errs {
        assert_eq!(e.map(err).as_deref(), Some("Aborted"));
    }
    assert_eq!(count, 0.0);
}

#[wasm_bindgen_test]
async fn signal_refuses_a_fake_signal() {
    abort_stub(false);
    let wallet = wallet();
    let online = wallet.go_online(abort_options(), None).await.unwrap();
    for fake in ["({})", "Object.create(AbortSignal.prototype)"] {
        let res = wallet
            .refresh(
                online.clone(),
                None,
                js_sys::Array::new().into(),
                true,
                js_signal(fake),
            )
            .await;
        assert_eq!(err(res.unwrap_err()), "Invalid AbortSignal");
    }
    let res = wallet
        .refresh(online, None, js_sys::Array::new().into(), true, None)
        .await;
    stub_restore();
    res.unwrap();
}

const VSS_SERVER_URL: &str = "http://127.0.0.1:8082/vss";

/// A memory store that follows the `BackupStore` contract. `mode` breaks versioned writes.
const MEMORY_STORE_JS: &str = r#"(() => {
    const s = { data: new Map(), calls: [], active: 0, maxActive: 0, delayMs: 0, mode: 'ok' };
    const conflict = () => Object.assign(new Error('version conflict'), { code: 'CONFLICT' });
    const run = async (call, f) => {
        s.calls.push(call);
        s.active += 1;
        s.maxActive = Math.max(s.maxActive, s.active);
        try {
            if (s.delayMs) await new Promise(r => setTimeout(r, s.delayMs));
            return f();
        } finally {
            s.active -= 1;
        }
    };
    s.read = (ns, key) => run(['read', ns, key], () => {
        const e = s.data.get(ns + '/' + key);
        return e ? { value: e.value.slice(), version: e.version } : null;
    });
    s.write = (ns, key, value, expected) => run(['write', ns, key, expected, value.length], () => {
        const k = ns + '/' + key;
        const cur = s.data.get(k);
        let ok;
        if (s.mode === 'ignoreExpected') ok = true;
        else if (expected === undefined) ok = !cur || s.mode === 'createOverwrites';
        else ok = !!cur && cur.version === expected;
        if (!ok) throw conflict();
        const version = (cur ? cur.version : 0) + 1;
        s.data.set(k, { value: value.slice(), version });
        return s.mode === 'noVersion' ? undefined : version;
    });
    s.remove = (ns, key, version) => run(['remove', ns, key, version], () => {
        const k = ns + '/' + key;
        const cur = s.data.get(k);
        if (version !== undefined && (!cur || cur.version !== version)) throw conflict();
        s.data.delete(k);
    });
    s.list = ns => run(['list', ns], () =>
        [...s.data.keys()].filter(k => k.startsWith(ns + '/')).map(k => k.slice(ns.length + 1)));
    return s;
})()"#;

fn memory_store() -> JsValue {
    js_sys::eval(MEMORY_STORE_JS).unwrap()
}

fn run_on(target: &JsValue, body: &str) -> JsValue {
    js_sys::Function::new_with_args("s", body)
        .call1(&JsValue::NULL, target)
        .unwrap()
}

/// A backup destination: a memory store from JS, or a VSS store.
struct Dest {
    memory: Option<JsValue>,
    store_id: String,
    key: SecretKey,
}

impl Dest {
    fn new(memory: bool) -> Self {
        let (key, pubkey) = Secp256k1::new().generate_keypair(&mut OsRng);
        Self {
            memory: memory.then(memory_store),
            store_id: format!("kv-{pubkey}"),
            key,
        }
    }

    fn both() -> [Self; 2] {
        [Self::new(true), Self::new(false)]
    }

    fn vss_config(&self) -> VssBackupConfig {
        VssBackupConfig::new(VSS_SERVER_URL.to_string(), self.store_id.clone(), self.key)
    }

    fn configure(&self, wallet: &WasmWallet) {
        let key_hex = hex::encode(self.key.secret_bytes());
        match &self.memory {
            Some(store) => wallet
                .configure_backup_store(store.clone().unchecked_into(), &self.store_id, &key_hex)
                .unwrap(),
            None => wallet
                .configure_vss_backup(VSS_SERVER_URL, &self.store_id, &key_hex)
                .unwrap(),
        }
    }

    fn raw(&self) -> Box<dyn KvStore> {
        match &self.memory {
            Some(store) => Box::new(JsKvStore::new(store.clone()).unwrap()),
            None => Box::new(VssStore::new(&self.vss_config())),
        }
    }

    async fn stored(&self, fingerprint: &str) -> Option<StoredValue> {
        let key = backup_key(fingerprint, BitcoinNetwork::Regtest);
        self.raw().read(BACKUP_NAMESPACE, &key).await.unwrap()
    }
}

/// Two wallet objects with the same keys and their own data dirs.
async fn wallet_pair() -> (WasmWallet, WasmWallet, SinglesigKeys) {
    let (wd, keys) = wallet_json(vec![AssetSchema::Nia]);
    let mut data: WalletData = serde_json::from_str(&wd).unwrap();
    let mut make = async |n: u8| {
        data.data_dir = format!("/tmp/rgb_kv-{n}-{}", js_sys::Date::now());
        let wd = serde_json::to_string(&data).unwrap();
        WasmWallet::create(&wd, &keys).await.unwrap()
    };
    let (a, b) = (make(0).await, make(1).await);
    (a, b, serde_json::from_str(&keys).unwrap())
}

fn version(v: JsValue) -> i64 {
    v.as_f64()
        .map(|n| n as i64)
        .unwrap_or_else(|| i64::try_from(v).unwrap())
}

async fn kv_contract(store: &dyn KvStore) {
    let ns = "contract";
    let conflict = |r: Result<i64, rgb_lib_wasm::Error>| {
        assert!(
            matches!(r, Err(rgb_lib_wasm::Error::VssVersionConflict { .. })),
            "{r:?}"
        )
    };
    let read = async |k| store.read(ns, k).await.unwrap();
    assert_eq!(read("a").await, None);
    assert!(store.list(ns).await.unwrap().is_empty());
    conflict(store.write(ns, "a", vec![1], Some(1)).await);
    assert!(matches!(
        store.remove(ns, "a", Some(1)).await,
        Err(rgb_lib_wasm::Error::VssVersionConflict { .. })
    ));
    assert_eq!(read("a").await, None);
    let v1 = store.write(ns, "a", vec![1], None).await.unwrap();
    assert!(v1 >= 1);
    let first = Some(StoredValue {
        value: vec![1],
        version: v1,
    });
    assert_eq!(read("a").await, first);
    conflict(store.write(ns, "a", vec![2], None).await);
    assert_eq!(read("a").await, first);
    conflict(store.write(ns, "a", vec![2], Some(v1 + 1)).await);
    assert_eq!(read("a").await, first);
    let big = vec![9; 64 * 1024];
    let v2 = store.write(ns, "a", big.clone(), Some(v1)).await.unwrap();
    assert!(v2 > v1);
    assert_eq!(
        read("a").await,
        Some(StoredValue {
            value: big,
            version: v2
        })
    );
    store.write(ns, "b", vec![], None).await.unwrap();
    store.write(ns, "c", vec![3], None).await.unwrap();
    let mut keys = store.list(ns).await.unwrap();
    keys.sort();
    assert_eq!(keys, ["a", "b", "c"]);
    assert!(store.list("other").await.unwrap().is_empty());
    assert!(matches!(
        store.remove(ns, "a", Some(v1)).await,
        Err(rgb_lib_wasm::Error::VssVersionConflict { .. })
    ));
    assert!(read("a").await.is_some());
    store.remove(ns, "a", Some(v2)).await.unwrap();
    assert_eq!(read("a").await, None);
    store.remove(ns, "b", None).await.unwrap();
    store.remove(ns, "b", None).await.unwrap();
    store.remove(ns, "c", None).await.unwrap();
    assert!(store.list(ns).await.unwrap().is_empty());
}

#[wasm_bindgen_test]
async fn backup_store_contract() {
    kv_contract(&JsKvStore::new(memory_store()).unwrap()).await;
    let vss = Dest::new(false);
    kv_contract(&VssStore::new(&vss.vss_config()).with_list_page_size(1)).await;
}

#[wasm_bindgen_test]
async fn backup_store_backup_and_restore() {
    let dest = Dest::new(true);
    let store = dest.memory.as_ref().unwrap();
    let (a, b, keys) = wallet_pair().await;
    let online = online(&a).await;
    let exp = JsValue::from(now_secs() + 100_000);
    let receive = a
        .witness_receive(online, None, any(), exp, endpoints(), 1, None)
        .await
        .unwrap();
    let recipient_id: String = prop(&receive, "recipientId").as_string().unwrap();
    dest.configure(&a);
    let v = version(a.vss_backup().await.unwrap());
    assert!(v >= 1);

    dest.configure(&b);
    b.vss_restore_backup().await.unwrap();
    let filter = serde_wasm_bindgen::to_value(&AssetFilter::AnyOrNone).unwrap();
    let scope = serde_wasm_bindgen::to_value(&TransferScope::All).unwrap();
    let transfers = b.list_transfers(filter, scope).unwrap();
    let found = js_sys::Function::new_with_args("t, r", "return t.some(x => x.recipientId === r);")
        .call2(&JsValue::NULL, &transfers, &recipient_id.as_str().into())
        .unwrap();
    assert_eq!(found, JsValue::TRUE);
    let info = b.vss_backup_info().await.unwrap();
    assert_eq!(prop(&info, "backup_exists"), JsValue::TRUE);
    assert_eq!(version(prop(&info, "server_version")), v);

    let key = backup_key(&keys.master_fingerprint, BitcoinNetwork::Regtest);
    let calls = run_on(
        store,
        &format!("return s.calls.every(c => c[1] === 'backup' && c[2] === '{key}');"),
    );
    assert_eq!(calls, JsValue::TRUE);
    let blob = dest.stored(&keys.master_fingerprint).await.unwrap().value;
    for needle in [
        keys.master_fingerprint.as_str(),
        &recipient_id,
        &keys.account_xpub_colored,
        &keys.account_xpub_vanilla,
    ] {
        let needle = needle.as_bytes();
        assert!(!blob.windows(needle.len()).any(|w| w == needle));
    }
}

#[wasm_bindgen_test]
async fn backup_store_conflict_overwrites_nothing() {
    for dest in Dest::both() {
        let (a, b, keys) = wallet_pair().await;
        dest.configure(&a);
        dest.configure(&b);
        let v1 = version(a.vss_backup().await.unwrap());
        let s1 = dest.stored(&keys.master_fingerprint).await;
        let e = err(b.vss_backup().await.unwrap_err());
        assert!(e.starts_with("VSS version conflict"), "{e}");
        assert_eq!(dest.stored(&keys.master_fingerprint).await, s1);

        b.vss_restore_backup().await.unwrap();
        let v2 = version(b.vss_backup().await.unwrap());
        assert!(v2 > v1);
        let s2 = dest.stored(&keys.master_fingerprint).await;
        let e = err(a.vss_backup().await.unwrap_err());
        assert!(e.starts_with("VSS version conflict"), "{e}");
        assert_eq!(dest.stored(&keys.master_fingerprint).await, s2);
    }
}

#[wasm_bindgen_test]
async fn backup_store_recreates_lost_backup() {
    for dest in Dest::both() {
        let (a, _, keys) = wallet_pair().await;
        dest.configure(&a);
        a.vss_backup().await.unwrap();
        match &dest.memory {
            Some(store) => {
                run_on(store, "s.data.clear();");
            }
            None => dest
                .raw()
                .remove(
                    BACKUP_NAMESPACE,
                    &backup_key(&keys.master_fingerprint, BitcoinNetwork::Regtest),
                    None,
                )
                .await
                .unwrap(),
        }
        let v = version(a.vss_backup().await.unwrap());
        assert!(v >= 1);
        let blob = dest.stored(&keys.master_fingerprint).await.unwrap().value;
        decode_backup_blob(&blob, &keys.master_fingerprint, &dest.key).unwrap();
        assert!(version(a.vss_backup().await.unwrap()) > v);
    }
}

#[wasm_bindgen_test]
async fn backup_store_version_survives_reload() {
    let dest = Dest::new(true);
    let (wd, keys) = wallet_json(vec![AssetSchema::Nia]);
    let mut data: WalletData = serde_json::from_str(&wd).unwrap();
    data.data_dir = format!("/tmp/rgb_kv_reload-{}", js_sys::Date::now());
    let wd = serde_json::to_string(&data).unwrap();
    let wallet = WasmWallet::create(&wd, &keys).await.unwrap();
    dest.configure(&wallet);
    let v1 = version(wallet.vss_backup().await.unwrap());
    wallet.flush().await.unwrap();
    drop(wallet);

    let wallet = WasmWallet::create(&wd, &keys).await.unwrap();
    dest.configure(&wallet);
    assert!(version(wallet.vss_backup().await.unwrap()) > v1);
    let key_hex = hex::encode(dest.key.secret_bytes());
    let store = dest.memory.clone().unwrap().unchecked_into();
    wallet
        .configure_backup_store(store, "other", &key_hex)
        .unwrap();
    let e = err(wallet.vss_backup().await.unwrap_err());
    assert!(e.starts_with("VSS version conflict"), "{e}");
}

#[wasm_bindgen_test]
fn backup_store_refuses_incomplete_store() {
    let wallet = wallet();
    let key_hex = "11".repeat(32);
    assert!(
        wallet
            .configure_backup_store(JsValue::NULL.unchecked_into(), "x", &key_hex)
            .is_err()
    );
    let store = js_sys::eval("({read() {}, write() {}, remove() {}})").unwrap();
    let e = err(wallet
        .configure_backup_store(store.unchecked_into(), "x", &key_hex)
        .unwrap_err());
    assert!(e.contains("backup store has no list method"), "{e}");
}

#[wasm_bindgen_test]
async fn backup_store_read_is_untrusted() {
    for bad in [
        "5",
        "({value: 'x', version: 1})",
        "({value: new Uint8Array(100), version: 0})",
        "({value: new Uint8Array(100), version: 1.5})",
        "({value: new Uint8Array(100), version: 2 ** 53})",
    ] {
        let store = js_sys::eval(&format!(
            "({{read() {{ return {bad}; }}, write() {{}}, remove() {{}}, list() {{}}}})"
        ))
        .unwrap();
        let store = JsKvStore::new(store).unwrap();
        assert!(
            store.read(BACKUP_NAMESPACE, "wallet").await.is_err(),
            "{bad}"
        );
    }
    for list in [
        "'a'",
        "[1]",
        "Array.from({length: 10001}, (_, i) => 'k' + i)",
    ] {
        let store = js_sys::eval(&format!(
            "({{read() {{}}, write() {{}}, remove() {{}}, list() {{ return {list}; }}}})"
        ))
        .unwrap();
        let store = JsKvStore::new(store).unwrap();
        assert!(store.list(BACKUP_NAMESPACE).await.is_err(), "{list}");
    }
}

#[wasm_bindgen_test]
async fn backup_store_calls_are_serialized() {
    let dest = Dest::new(true);
    let store = dest.memory.clone().unwrap();
    run_on(&store, "s.delayMs = 50;");
    let wallet = std::rc::Rc::new(wallet());
    dest.configure(&wallet);
    let w1 = wallet.clone();
    let backup = wasm_bindgen_futures::future_to_promise(async move { w1.vss_backup().await });
    let w2 = wallet.clone();
    let info = wasm_bindgen_futures::future_to_promise(async move { w2.vss_backup_info().await });
    let all = js_sys::Promise::all(&js_sys::Array::of2(&backup, &info));
    wasm_bindgen_futures::JsFuture::from(all).await.unwrap();
    assert_eq!(prop(&store, "maxActive"), JsValue::from(1));
}
