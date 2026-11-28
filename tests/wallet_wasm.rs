use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

mod utils;

use bdk_wallet::bitcoin::secp256k1::{Secp256k1, SecretKey, rand::rngs::OsRng};
use rgb_lib_wasm::wallet::backup_store::{
    BACKUP_NAMESPACE, KvStore, backup_key, decode_backup_blob, encode_backup_blob,
};
use rgb_lib_wasm::wallet::rust_only::{ValidateConsignmentResult, validate_consignment_offchain};
use rgb_lib_wasm::wallet::vss::{VssBackupConfig, VssStore};
use rgb_lib_wasm::wallet::{
    AssetFilter, DatabaseType, IndexerConfig, Invoice, OnlineOptions, SinglesigKeys, TransferScope,
    Wallet, WalletData,
};
use rgb_lib_wasm::{
    AssetSchema, Assignment, BitcoinNetwork, Error, TransferStatus, WitnessVersion, generate_keys,
};
use utils::{PROXY_URL, VSS_SERVER_URL, go_online_options, run_js, transport_endpoint, within};

fn test_wallet_data(schemas: Vec<AssetSchema>) -> (WalletData, SinglesigKeys) {
    let keys = generate_keys(BitcoinNetwork::Regtest, WitnessVersion::Taproot);
    (
        WalletData {
            data_dir: "/tmp/rgb_test".to_string(),
            bitcoin_network: BitcoinNetwork::Regtest,
            database_type: DatabaseType::Sqlite,
            max_allocations_per_utxo: 5,
            supported_schemas: schemas,
            allow_reckless_schemas: false,
        },
        SinglesigKeys::from_keys(&keys, None),
    )
}

#[wasm_bindgen_test]
fn test_wallet_new_signing() {
    let (wd, keys) = test_wallet_data(vec![AssetSchema::Nia]);
    assert!(Wallet::new(wd, keys).is_ok());
}

#[wasm_bindgen_test]
fn test_wallet_new_watch_only() {
    let keys = generate_keys(BitcoinNetwork::Regtest, WitnessVersion::Taproot);
    let wd = WalletData {
        data_dir: "/tmp/rgb_test_wo".to_string(),
        bitcoin_network: BitcoinNetwork::Regtest,
        database_type: DatabaseType::Sqlite,
        max_allocations_per_utxo: 5,
        supported_schemas: vec![AssetSchema::Nia],
        allow_reckless_schemas: false,
    };
    let keys = SinglesigKeys::from_keys_no_mnemonic(&keys, None);
    assert!(Wallet::new(wd, keys).is_ok());
}

#[wasm_bindgen_test]
fn test_wallet_new_fingerprint_mismatch() {
    let keys = generate_keys(BitcoinNetwork::Regtest, WitnessVersion::Taproot);
    let wd = WalletData {
        data_dir: "/tmp/rgb_test_fp".to_string(),
        bitcoin_network: BitcoinNetwork::Regtest,
        database_type: DatabaseType::Sqlite,
        max_allocations_per_utxo: 5,
        supported_schemas: vec![AssetSchema::Nia],
        allow_reckless_schemas: false,
    };
    let keys = SinglesigKeys {
        master_fingerprint: "deadbeef".to_string(),
        ..SinglesigKeys::from_keys(&keys, None)
    };
    let result = Wallet::new(wd, keys);
    assert!(result.is_err());
    let err = format!("{:?}", result.err().unwrap());
    assert!(err.contains("FingerprintMismatch"));
}

/// Tests wallet queries on a fresh wallet: address, balance, transactions,
/// transfers, unspents, list_assets, get_wallet_data, delete_transfers, backup_info
#[wasm_bindgen_test]
async fn test_wallet_queries_fresh() {
    let (wd, keys) = test_wallet_data(vec![AssetSchema::Nia, AssetSchema::Ifa]);
    let mut wallet = Wallet::new(wd.clone(), keys.clone()).unwrap();

    // get_address: two calls return different regtest addresses
    let addr1 = wallet.get_address().unwrap();
    let addr2 = wallet.get_address().unwrap();
    assert!(addr1.starts_with("bcrt1"));
    assert_ne!(addr1, addr2);

    // get_wallet_data: round-trip matches input
    let returned = wallet.get_wallet_data();
    assert_eq!(returned.bitcoin_network, wd.bitcoin_network);
    assert_eq!(
        returned.max_allocations_per_utxo,
        wd.max_allocations_per_utxo
    );
    assert_eq!(returned.supported_schemas, wd.supported_schemas);

    // get_keys: round-trip matches input
    let returned = wallet.get_keys();
    assert_eq!(returned.account_xpub_vanilla, keys.account_xpub_vanilla);
    assert_eq!(returned.account_xpub_colored, keys.account_xpub_colored);
    assert_eq!(returned.mnemonic, keys.mnemonic);
    assert_eq!(returned.master_fingerprint, keys.master_fingerprint);

    // get_btc_balance: zero on unfunded wallet
    let balance = wallet.get_btc_balance(None, true).await.unwrap();
    assert_eq!(balance.vanilla.settled, 0);
    assert_eq!(balance.vanilla.future, 0);
    assert_eq!(balance.colored.settled, 0);
    assert_eq!(balance.colored.future, 0);

    // list_transactions: empty on fresh wallet
    let txs = wallet.list_transactions(None, true).await.unwrap();
    assert!(txs.is_empty());

    // list_transfers: empty on fresh wallet
    let transfers = wallet
        .list_transfers(AssetFilter::None, TransferScope::All)
        .unwrap();
    assert!(transfers.is_empty());

    // list_unspents: empty on fresh wallet
    let unspents = wallet.list_unspents(None, false, true).await.unwrap();
    assert!(unspents.is_empty());

    // list_assets: empty for all schema types
    let assets = wallet.list_assets(vec![AssetSchema::Nia]).unwrap();
    assert!(assets.nia.unwrap().is_empty());
    let assets = wallet.list_assets(vec![AssetSchema::Ifa]).unwrap();
    assert!(assets.ifa.unwrap().is_empty());
    let assets = wallet.list_assets(vec![]).unwrap();
    assert!(assets.nia.unwrap().is_empty());
    assert!(assets.ifa.unwrap().is_empty());

    // delete_transfers: no transfers to delete returns false
    let result = wallet.delete_transfers(None, false).unwrap();
    assert!(!result);

    // backup_info: verify it doesn't error on fresh wallet
    let _info = wallet.backup_info().unwrap();
}

fn rcv_expiration() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64 + 86400
}

/// Tests receive operations: witness_receive (needs the proxy), list_transfers
/// after receive, and error cases for invalid asset IDs.
/// Note: blind_receive requires funded UTXOs so it's tested in integration_wasm.
#[wasm_bindgen_test]
async fn test_receive_and_transfers() {
    let (wd, keys) = test_wallet_data(vec![AssetSchema::Nia]);
    let mut wallet = Wallet::new(wd, keys).unwrap();
    let online = wallet.go_online(go_online_options(true)).await.unwrap();

    // witness_receive: success
    let expiration = rcv_expiration();
    let recv = wallet
        .witness_receive(
            Some(online.clone()),
            None,
            Assignment::Any,
            expiration,
            vec![transport_endpoint(PROXY_URL)],
            1,
            None,
            None,
        )
        .await
        .unwrap();
    assert!(!recv.invoice.is_empty());
    assert_eq!(recv.public_key.len(), 64);
    assert!(!recv.recipient_id.is_empty());
    assert_eq!(recv.expiration_timestamp, expiration);

    // list_transfers: should have a pending transfer from witness_receive
    let transfers = wallet
        .list_transfers(AssetFilter::None, TransferScope::All)
        .unwrap();
    assert!(
        !transfers.is_empty(),
        "Should have transfers after witness_receive"
    );
    assert!(
        transfers
            .iter()
            .any(|t| t.status == TransferStatus::WaitingCounterparty),
        "Should have WaitingCounterparty transfer"
    );
    let transfer = transfers
        .iter()
        .find(|t| t.recipient_id == Some(recv.recipient_id.clone()))
        .unwrap();
    assert_eq!(transfer.expiration_timestamp, Some(expiration));

    // witness_receive: invalid asset
    let err = wallet
        .witness_receive(
            Some(online.clone()),
            Some("nonexistent_asset_id".into()),
            Assignment::Fungible(100),
            rcv_expiration(),
            vec![transport_endpoint(PROXY_URL)],
            1,
            None,
            None,
        )
        .await
        .unwrap_err();
    assert!(format!("{:?}", err).contains("AssetNotFound"));
    // Note: blind_receive requires funded UTXOs even for error paths,
    // so it's only tested in integration_wasm.rs

    // Invoice: parse the invoice from witness_receive
    let invoice = Invoice::new(recv.invoice.clone()).unwrap();
    assert_eq!(invoice.invoice_string(), recv.invoice);

    let data = invoice.invoice_data();
    assert_eq!(data.recipient_id, recv.recipient_id);
    assert_eq!(data.network, BitcoinNetwork::Regtest);
    assert_eq!(data.assignment, Assignment::Any);
    assert!(!data.transport_endpoints.is_empty());
    assert_eq!(data.expiration_timestamp, Some(expiration));
    assert_eq!(data.public_key, Some(recv.public_key.clone()));
    assert_eq!(data.encryption, rgb_lib_wasm::Encryption::Required);

    // witness_receive: without being online
    let err = wallet
        .witness_receive(
            None,
            None,
            Assignment::Any,
            rcv_expiration(),
            vec![transport_endpoint(PROXY_URL)],
            1,
            None,
            None,
        )
        .await
        .unwrap_err();
    assert!(format!("{:?}", err).contains("OnlineNeeded"));

    // Invoice: BFA assignments
    let bfa_invoice = |state: &str, name: Option<&str>| {
        // the invoice carries the schema ID without its prefix and mnemonic
        let schema_id = rgb_lib_wasm::SchemaId::from(AssetSchema::Bfa).to_string();
        let schema_id = schema_id
            .trim_start_matches("rgb:sch:")
            .split('#')
            .next()
            .unwrap();
        let mut invoice = format!("rgb:~/{schema_id}/{state}/{}", recv.recipient_id);
        if let Some(name) = name {
            invoice.push_str(&format!("?assignment_name={name}"));
        }
        Invoice::new(invoice)
    };
    for (state, name, assignment) in [
        (AMOUNT_100, Some("assetOwner"), Assignment::Fungible(100)),
        ("~", Some("assetOwner"), Assignment::Fungible(0)),
        (AMOUNT_100, None, Assignment::Any),
        ("~", None, Assignment::Any),
        ("~", Some("linkRight"), Assignment::LinkRight),
    ] {
        let data = bfa_invoice(state, name).unwrap().invoice_data();
        assert_eq!(data.asset_schema, Some(AssetSchema::Bfa));
        assert_eq!(data.assignment, assignment);
    }
    for (state, name) in [
        (AMOUNT_100, "inflationAllowance"),
        (AMOUNT_100, "linkRight"),
        ("~", "other"),
    ] {
        assert!(matches!(
            bfa_invoice(state, Some(name)),
            Err(Error::InvalidInvoice { .. })
        ));
    }

    // Invoice: invalid string should fail
    assert!(Invoice::new("not-a-valid-invoice".to_string()).is_err());
}

/// The BFA schema ID as an invoice carries it
/// The amount 100 as an invoice carries it
const AMOUNT_100: &str = "BF";

/// Tests the public paths of the shared types and the RGB re-exports
#[wasm_bindgen_test]
fn public_api_exports() {
    let _: Option<(
        rgb_lib_wasm::wallet::RecipientInfo,
        rgb_lib_wasm::wallet::TransportEndpoint,
        rgb_lib_wasm::wallet::Outpoint,
        rgb_lib_wasm::TypeOfTransition,
    )> = None::<(
        rgb_lib_wasm::no_wallet::RecipientInfo,
        rgb_lib_wasm::no_wallet::TransportEndpoint,
        rgb_lib_wasm::no_wallet::Outpoint,
        rgb_lib_wasm::TypeOfTransition,
    )>;
    let _: Option<(
        rgb_lib_wasm::AssignmentType,
        rgb_lib_wasm::OpId,
        rgb_lib_wasm::GraphSeal,
        rgb_lib_wasm::BuilderSeal<rgb_lib_wasm::GraphSeal>,
    )> = None;
    let _: Option<(
        rgb_lib_wasm::Genesis,
        rgb_lib_wasm::SpvProof,
        rgb_lib_wasm::WitnessBundle,
        rgb_lib_wasm::LoadError,
    )> = None;
}

/// Tests error paths and backup: invalid PSBT, issue without UTXOs,
/// nonexistent asset balance, backup/restore round-trip
#[wasm_bindgen_test]
async fn test_errors_and_backup() {
    let (wd, keys) = test_wallet_data(vec![AssetSchema::Nia]);
    let mut wallet = Wallet::new(wd, keys).unwrap();
    let online = wallet.go_online(go_online_options(true)).await.unwrap();

    // sign_psbt: invalid PSBT
    assert!(
        wallet
            .sign_psbt("not-a-valid-psbt".to_string(), None)
            .is_err()
    );

    // finalize_psbt: invalid PSBT
    assert!(
        wallet
            .finalize_psbt("not-a-valid-psbt".to_string(), None)
            .is_err()
    );

    // issue_asset_nia: no UTXOs should fail
    let result =
        wallet.issue_asset_nia("FAIL".to_string(), "Should Fail".to_string(), 0, vec![1000]);
    assert!(result.is_err(), "Issuing without UTXOs should fail");

    // get_asset_balance: nonexistent asset
    let err = wallet.get_asset_balance("nonexistent_asset_id".to_string());
    assert!(err.is_err());
    assert!(format!("{:?}", err.unwrap_err()).contains("AssetNotFound"));

    // backup/restore round-trip: use witness_receive to create state
    let _recv = wallet
        .witness_receive(
            Some(online),
            None,
            Assignment::Any,
            rcv_expiration(),
            vec![transport_endpoint(PROXY_URL)],
            1,
            None,
            None,
        )
        .await
        .unwrap();

    let password = "test_pass_123";
    let backup_bytes = wallet.backup(password).unwrap();
    assert!(!backup_bytes.is_empty());

    // Wrong password fails
    assert!(wallet.restore_backup(&backup_bytes, "wrong").is_err());

    // Correct password restores
    wallet.restore_backup(&backup_bytes, password).unwrap();

    // State survives: transfer still exists
    let transfers = wallet
        .list_transfers(AssetFilter::None, TransferScope::All)
        .unwrap();
    assert!(
        !transfers.is_empty(),
        "Transfers should survive backup/restore"
    );
}

#[wasm_bindgen_test]
fn test_restore_backup_wrapping_length_fails() {
    let (wd, keys) = test_wallet_data(vec![AssetSchema::Nia]);
    let mut wallet = Wallet::new(wd, keys).unwrap();
    assert!(matches!(
        wallet.restore_backup(&[0xff, 0xff, 0xff, 0xff, 1, 2, 3, 4], "pw"),
        Err(Error::InvalidBackup)
    ));
}

#[wasm_bindgen_test]
fn test_validate_consignment_offchain_invalid_bytes() {
    let result = validate_consignment_offchain(
        b"garbage data",
        "0000000000000000000000000000000000000000000000000000000000000000",
        BitcoinNetwork::Regtest,
    );
    assert!(result.is_err());
}

#[wasm_bindgen_test]
fn test_validate_consignment_result_serializes() {
    let result = ValidateConsignmentResult {
        valid: true,
        warnings: Some(vec!["test warning".to_string()]),
        error: None,
        details: None,
    };
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"valid\":true"));
    assert!(json.contains("test warning"));
}

#[wasm_bindgen_test]
async fn test_save_uses_strict_durability() {
    run_js(
        "const original = IDBDatabase.prototype.transaction;
        globalThis.__idbTransactionOriginal = original;
        globalThis.__idbTransactions = [];
        IDBDatabase.prototype.transaction = function (stores, mode, options) {
            globalThis.__idbTransactions.push(`${mode}:${options && options.durability}`);
            return original.apply(this, arguments);
        };",
    );
    let (wd, keys) = test_wallet_data(vec![AssetSchema::Nia]);
    let wallet = Wallet::new(wd, keys).unwrap();
    let flushed = wallet.flush().await;
    let transactions: Vec<String> = js_sys::Array::from(&run_js(
        "IDBDatabase.prototype.transaction = globalThis.__idbTransactionOriginal;
        return globalThis.__idbTransactions;",
    ))
    .iter()
    .map(|t| t.as_string().unwrap())
    .collect();
    flushed.unwrap();
    let saves: Vec<&String> = transactions
        .iter()
        .filter(|t| t.starts_with("readwrite:"))
        .collect();
    assert!(
        !saves.is_empty() && saves.iter().all(|t| *t == "readwrite:strict"),
        "{transactions:?}"
    );
}

#[wasm_bindgen_test]
async fn test_create_and_restore_request_persistent_storage() {
    run_js(
        "globalThis.__persistCalls = 0;
        navigator.storage.persist = function () {
            globalThis.__persistCalls += 1;
            return Promise.resolve(false);
        };",
    );
    let persist_calls = || {
        js_sys::Reflect::get(&js_sys::global(), &"__persistCalls".into())
            .unwrap()
            .as_f64()
            .unwrap()
    };
    let (wd, keys) = test_wallet_data(vec![AssetSchema::Nia]);
    let mut wallet = Wallet::restore(wd, keys).await.unwrap();
    let after_create = persist_calls();
    let backup = wallet.backup("password").unwrap();
    let restored = wallet.restore_backup(&backup, "password");
    let after_restore = persist_calls();
    run_js("delete navigator.storage.persist;");
    assert!(restored.is_ok(), "{restored:?}");
    assert_eq!(after_create, 1.0);
    assert_eq!(after_restore, 2.0);
}

struct VssFixture {
    store: VssStore,
    config: VssBackupConfig,
    key: SecretKey,
}

impl VssFixture {
    fn new() -> Self {
        let (key, public_key) = Secp256k1::new().generate_keypair(&mut OsRng);
        let config = VssBackupConfig::new(
            VSS_SERVER_URL.to_string(),
            format!("restore-checks-{public_key}"),
            key,
        );
        Self {
            store: VssStore::new(&config),
            config,
            key,
        }
    }

    async fn blob(&self, fingerprint: &str) -> Vec<u8> {
        let key = backup_key(fingerprint, BitcoinNetwork::Regtest);
        let stored = self.store.read(BACKUP_NAMESPACE, &key).await;
        stored.unwrap().unwrap().value
    }

    async fn put(&self, fingerprint: &str, blob: Vec<u8>) {
        let key = backup_key(fingerprint, BitcoinNetwork::Regtest);
        let stored = self.store.read(BACKUP_NAMESPACE, &key).await;
        let version = stored.unwrap().unwrap().version;
        self.store
            .write(BACKUP_NAMESPACE, &key, blob, Some(version))
            .await
            .unwrap();
    }

    async fn payload(&self, fingerprint: &str) -> serde_json::Value {
        let plain =
            decode_backup_blob(&self.blob(fingerprint).await, fingerprint, &self.key).unwrap();
        serde_json::from_slice(&plain).unwrap()
    }
}

async fn wallet_with_vss_backup() -> (Wallet, VssFixture) {
    let (wd, keys) = test_wallet_data(vec![AssetSchema::Nia]);
    let mut wallet = Wallet::new(wd, keys).unwrap();
    let store = VssFixture::new();
    wallet.configure_vss_backup(&store.config);
    wallet.vss_backup().await.unwrap();
    (wallet, store)
}

async fn witness_receive_on(wallet: &mut Wallet) -> String {
    let online = wallet.go_online(go_online_options(true)).await.unwrap();
    wallet
        .witness_receive(
            Some(online),
            None,
            Assignment::Any,
            rcv_expiration(),
            vec![transport_endpoint(PROXY_URL)],
            1,
            None,
            None,
        )
        .await
        .unwrap()
        .recipient_id
}

fn has_transfer(wallet: &Wallet, recipient_id: &str) -> bool {
    wallet
        .list_transfers(AssetFilter::None, TransferScope::All)
        .unwrap()
        .iter()
        .any(|t| t.recipient_id.as_deref() == Some(recipient_id))
}

/// A VSS store that holds another wallet's state does not restore.
#[wasm_bindgen_test]
async fn test_vss_restore_refuses_other_wallet_state() {
    let (mut a, store_a) = wallet_with_vss_backup().await;
    let recv_id = witness_receive_on(&mut a).await;
    let (b, store_b) = wallet_with_vss_backup().await;

    let mut payload = store_b.payload(&b.get_keys().master_fingerprint).await;
    payload["bdk_changeset"] = serde_json::Value::Null;
    let fingerprint = a.get_keys().master_fingerprint;
    let plain = serde_json::to_vec(&payload).unwrap();
    let blob = encode_backup_blob(&plain, &fingerprint, &store_a.key).unwrap();
    store_a.put(&fingerprint, blob).await;

    let result = a.vss_restore_backup().await;
    assert!(result.is_err(), "restore of a tampered store succeeded");
    assert!(has_transfer(&a, &recv_id));
}

#[wasm_bindgen_test]
async fn test_vss_restore_refuses_unencrypted_data() {
    let (mut wallet, store) = wallet_with_vss_backup().await;
    let payload = store.payload(&wallet.get_keys().master_fingerprint).await;
    let mut blob = vec![1];
    blob.extend([0; 51]);
    blob.extend(serde_json::to_vec(&payload).unwrap());
    store.put(&wallet.get_keys().master_fingerprint, blob).await;
    let err = wallet.vss_restore_backup().await.unwrap_err();
    assert!(
        matches!(err, Error::VssError { .. } | Error::InvalidBackup),
        "{err:?}"
    );
}

#[wasm_bindgen_test]
async fn test_vss_restore_refuses_other_fingerprint() {
    let (mut wallet, store) = wallet_with_vss_backup().await;
    let payload = store.payload(&wallet.get_keys().master_fingerprint).await;
    let other = generate_keys(BitcoinNetwork::Regtest, WitnessVersion::Taproot).master_fingerprint;
    let plain = serde_json::to_vec(&payload).unwrap();
    store
        .put(
            &wallet.get_keys().master_fingerprint,
            encode_backup_blob(&plain, &other, &store.key).unwrap(),
        )
        .await;
    let err = wallet.vss_restore_backup().await.unwrap_err();
    assert!(matches!(err, Error::FingerprintMismatch), "{err:?}");
}

/// A stalled indexer or VSS server fails the call after the configured timeout.
#[wasm_bindgen_test]
async fn test_stalled_indexer_and_vss_time_out() {
    run_js(
        "const g = globalThis;
        g.__stallSaved = [g.fetch, g.__rgbLibNoStoreUrls];
        delete g.__rgbLibNoStoreUrls;
        g.__stallAborts = 0;
        const fetch = g.fetch;
        g.fetch = function (input, init) {
            const url = input instanceof Request ? input.url : String(input);
            if (!url.startsWith('http://stall.invalid/')) {
                return fetch.call(this, input, init);
            }
            const signal = (init && init.signal) || input.signal;
            return new Promise((_, reject) => signal.addEventListener('abort', () => {
                g.__stallAborts += 1;
                reject(signal.reason);
            }));
        };",
    );
    let (wd, keys) = test_wallet_data(vec![AssetSchema::Nia]);
    let mut wallet = Wallet::new(wd, keys).unwrap();
    let options = OnlineOptions {
        indexer_config: IndexerConfig::Esplora {
            url: "http://STALL.invalid:80/indexer".to_string(),
        },
        ..go_online_options(true)
    };
    let start = js_sys::Date::now();
    let online = within(15_000, wallet.go_online(options)).await;
    let online_ms = js_sys::Date::now() - start;

    wallet.configure_vss_backup(&VssBackupConfig::new(
        "http://stall.invalid/vss".to_string(),
        "stall".to_string(),
        SecretKey::from_slice(&[1; 32]).unwrap(),
    ));
    let start = js_sys::Date::now();
    let backup = within(15_000, wallet.vss_backup()).await;
    let backup_ms = js_sys::Date::now() - start;

    let aborts = run_js(
        "const g = globalThis;
        const [fetch, urls] = g.__stallSaved;
        g.fetch = fetch;
        if (urls === undefined) delete g.__rgbLibNoStoreUrls; else g.__rgbLibNoStoreUrls = urls;
        delete g.__stallSaved;
        return g.__stallAborts;",
    )
    .as_f64()
    .unwrap();
    assert!(
        matches!(online, Some(Err(Error::InvalidIndexer { .. }))),
        "go_online after {online_ms} ms: {online:?}"
    );
    assert!(
        matches!(backup, Some(Err(Error::VssError { .. }))),
        "vss_backup after {backup_ms} ms: {backup:?}"
    );
    assert!(online_ms >= 9_000.0, "go_online after {online_ms} ms");
    assert!(backup_ms >= 9_000.0, "vss_backup after {backup_ms} ms");
    assert_eq!(aborts, 2.0);
}

/// A busy indexer is asked again 3 times, after real pauses.
#[wasm_bindgen_test]
async fn test_busy_indexer_retried_after_pauses() {
    run_js(
        "const g = globalThis;
        g.__busySaved = [g.fetch, g.__rgbLibNoStoreUrls];
        delete g.__rgbLibNoStoreUrls;
        g.__busyHits = 0;
        const fetch = g.fetch;
        g.fetch = function (input, init) {
            const url = input instanceof Request ? input.url : String(input);
            if (!url.startsWith('http://busy.invalid/')) {
                return fetch.call(this, input, init);
            }
            g.__busyHits += 1;
            const resp = new Response('', { status: 503 });
            Object.defineProperty(resp, 'url', { value: url });
            return Promise.resolve(resp);
        };",
    );
    let (wd, keys) = test_wallet_data(vec![AssetSchema::Nia]);
    let mut wallet = Wallet::new(wd, keys).unwrap();
    let options = OnlineOptions {
        indexer_config: IndexerConfig::Esplora {
            url: "http://busy.invalid/indexer".to_string(),
        },
        ..go_online_options(true)
    };
    let start = js_sys::Date::now();
    let online = within(15_000, wallet.go_online(options)).await;
    let online_ms = js_sys::Date::now() - start;
    let hits = run_js(
        "const g = globalThis;
        const [fetch, urls] = g.__busySaved;
        g.fetch = fetch;
        if (urls === undefined) delete g.__rgbLibNoStoreUrls; else g.__rgbLibNoStoreUrls = urls;
        delete g.__busySaved;
        return g.__busyHits;",
    )
    .as_f64()
    .unwrap();
    assert!(
        matches!(online, Some(Err(Error::InvalidIndexer { .. }))),
        "go_online after {online_ms} ms: {online:?}"
    );
    assert_eq!(hits, 4.0);
    // the pauses are 256, 512 and 1024 ms
    assert!(online_ms >= 1_700.0, "go_online after {online_ms} ms");
}

// ---------------------------------------------------------------------------
// Saved wallet configuration
// ---------------------------------------------------------------------------

use AssetSchema::{Ifa, Nia};
use BitcoinNetwork::{Regtest, Signet};
use rgb_lib_wasm::wallet::idb_store::{load_snapshot, save_snapshot};

fn config_data(name: &str, bitcoin_network: BitcoinNetwork) -> WalletData {
    WalletData {
        data_dir: format!("/tmp/config-{name}-{}", js_sys::Date::now()),
        bitcoin_network,
        database_type: DatabaseType::Sqlite,
        max_allocations_per_utxo: 5,
        supported_schemas: vec![Nia],
        allow_reckless_schemas: false,
    }
}

fn to_json<T: serde::Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap()
}

fn mnemonic(keys: &rgb_lib_wasm::keys::Keys) -> Option<String> {
    Some(keys.mnemonic.to_string())
}

async fn saved(wd: &WalletData, keys: SinglesigKeys) {
    Wallet::new(wd.clone(), keys)
        .unwrap()
        .flush()
        .await
        .unwrap();
}

fn assert_mismatch(err: Error, expected: &str) {
    assert!(
        matches!(&err, Error::WalletSettingMismatch { setting, .. } if setting == expected),
        "{err:?}"
    );
}

#[wasm_bindgen_test]
async fn config_load_success() {
    let keys = generate_keys(Signet, WitnessVersion::SegWitV0);
    let wd = WalletData {
        max_allocations_per_utxo: 1,
        supported_schemas: vec![Nia, Ifa],
        allow_reckless_schemas: true,
        ..config_data("load", Signet)
    };
    let sk = SinglesigKeys::from_keys(&keys, Some(2));
    let wallet = Wallet::new(wd.clone(), sk.clone()).unwrap();
    wallet.flush().await.unwrap();
    let (dir, descriptors) = (wallet.get_wallet_dir(), wallet.get_descriptors());
    drop(wallet);

    let mut loaded = Wallet::load(
        &wd.data_dir,
        Signet,
        &keys.master_fingerprint,
        mnemonic(&keys),
    )
    .await
    .unwrap();
    assert_eq!(loaded.get_wallet_dir(), dir);
    assert_eq!(loaded.get_descriptors(), descriptors);
    assert_eq!(to_json(&loaded.get_wallet_data()), to_json(&wd));
    assert_eq!(to_json(&loaded.get_keys()), to_json(&sk));
    loaded.get_address().unwrap();
    loaded.flush().await.unwrap();
}

#[wasm_bindgen_test]
async fn config_restore_updates_saved_settings() {
    let keys = generate_keys(Regtest, WitnessVersion::Taproot);
    let wd = WalletData {
        max_allocations_per_utxo: 1,
        ..config_data("update", Regtest)
    };
    saved(&wd, SinglesigKeys::from_keys(&keys, None)).await;
    let load = || Wallet::load(&wd.data_dir, Regtest, &keys.master_fingerprint, None);
    assert_eq!(
        load()
            .await
            .unwrap()
            .get_wallet_data()
            .max_allocations_per_utxo,
        1
    );

    let changed = WalletData {
        max_allocations_per_utxo: 3,
        supported_schemas: vec![Nia, Ifa],
        ..wd.clone()
    };
    let wallet = Wallet::restore(changed, SinglesigKeys::from_keys_no_mnemonic(&keys, None))
        .await
        .unwrap();
    wallet.flush().await.unwrap();
    drop(wallet);
    let got = load().await.unwrap().get_wallet_data();
    assert_eq!(got.max_allocations_per_utxo, 3);
    assert_eq!(got.supported_schemas, vec![Nia, Ifa]);
}

#[wasm_bindgen_test]
async fn config_restore_refuses_fixed_settings() {
    let keys = generate_keys(Regtest, WitnessVersion::Taproot);
    let wd = config_data("fixed", Regtest);
    saved(&wd, SinglesigKeys::from_keys(&keys, None)).await;

    let err = Wallet::restore(wd.clone(), SinglesigKeys::from_keys(&keys, Some(2)))
        .await
        .err()
        .unwrap();
    assert_mismatch(err, "vanilla_keychain");
}

#[wasm_bindgen_test]
async fn config_watch_only_toggle_success() {
    let keys = generate_keys(Regtest, WitnessVersion::Taproot);
    let fp = &keys.master_fingerprint;
    let wd = config_data("toggle", Regtest);
    saved(&wd, SinglesigKeys::from_keys(&keys, None)).await;
    let loaded = Wallet::load(&wd.data_dir, Regtest, fp, None).await.unwrap();
    assert!(loaded.get_keys().mnemonic.is_none());
    let watch_only = Wallet::new(
        wd.clone(),
        SinglesigKeys::from_keys_no_mnemonic(&keys, None),
    )
    .unwrap();
    assert_eq!(loaded.get_descriptors(), watch_only.get_descriptors());
    let loaded = Wallet::load(&wd.data_dir, Regtest, fp, mnemonic(&keys))
        .await
        .unwrap();
    assert_eq!(loaded.get_keys().mnemonic, Some(keys.mnemonic.clone()));

    let wd = config_data("toggle-watch-only", Regtest);
    saved(&wd, SinglesigKeys::from_keys_no_mnemonic(&keys, None)).await;
    let loaded = Wallet::load(&wd.data_dir, Regtest, fp, mnemonic(&keys))
        .await
        .unwrap();
    assert_eq!(loaded.get_keys().mnemonic, Some(keys.mnemonic.clone()));
}

#[wasm_bindgen_test]
async fn config_load_inexistent_manifest_fail() {
    let keys = generate_keys(Regtest, WitnessVersion::Taproot);
    let wd = config_data("inexistent", Regtest);
    let wallet = Wallet::new(wd.clone(), SinglesigKeys::from_keys(&keys, None)).unwrap();
    let load = || Wallet::load(&wd.data_dir, Regtest, &keys.master_fingerprint, None);
    let err = load().await.err().unwrap();
    assert!(
        matches!(&err, Error::InexistentWalletManifest { path } if *path == wallet.idb_key()),
        "{err:?}"
    );
    wallet.flush().await.unwrap();
    assert!(load().await.is_ok());
}

#[wasm_bindgen_test]
async fn config_load_fingerprint_mismatch_fail() {
    let wd = config_data("fingerprint", Regtest);
    let (keys_1, keys_2) = (
        generate_keys(Regtest, WitnessVersion::Taproot),
        generate_keys(Regtest, WitnessVersion::Taproot),
    );
    let wallet_1 = Wallet::new(wd.clone(), SinglesigKeys::from_keys(&keys_1, None)).unwrap();
    wallet_1.flush().await.unwrap();
    let wallet_2 = Wallet::new(wd.clone(), SinglesigKeys::from_keys(&keys_2, None)).unwrap();
    let snapshot = load_snapshot(&wallet_1.idb_key()).await.unwrap().unwrap();
    save_snapshot(&wallet_2.idb_key(), &snapshot, None)
        .await
        .unwrap();

    let err = Wallet::load(&wd.data_dir, Regtest, &keys_2.master_fingerprint, None)
        .await
        .err()
        .unwrap();
    assert!(matches!(err, Error::FingerprintMismatch), "{err:?}");
}

#[wasm_bindgen_test]
async fn config_load_wrong_mnemonic_fail() {
    let keys = generate_keys(Regtest, WitnessVersion::Taproot);
    let wd = config_data("wrong-mnemonic", Regtest);
    saved(&wd, SinglesigKeys::from_keys(&keys, None)).await;
    let other = generate_keys(Regtest, WitnessVersion::Taproot);
    let err = Wallet::load(
        &wd.data_dir,
        Regtest,
        &keys.master_fingerprint,
        mnemonic(&other),
    )
    .await
    .err()
    .unwrap();
    assert!(matches!(err, Error::InvalidBitcoinKeys), "{err:?}");
}

#[wasm_bindgen_test]
async fn config_load_after_backup_restore_success() {
    let keys = generate_keys(Regtest, WitnessVersion::Taproot);
    let wd_a = WalletData {
        max_allocations_per_utxo: 1,
        ..config_data("backup-a", Regtest)
    };
    let source = Wallet::new(wd_a, SinglesigKeys::from_keys(&keys, None)).unwrap();
    let backup = source.backup("password").unwrap();

    let wd_b = WalletData {
        max_allocations_per_utxo: 3,
        ..config_data("backup-b", Regtest)
    };
    let mut wallet = Wallet::new(wd_b.clone(), SinglesigKeys::from_keys(&keys, None)).unwrap();
    wallet.restore_backup(&backup, "password").unwrap();
    wallet.flush().await.unwrap();
    drop(wallet);

    let loaded = Wallet::load(
        &wd_b.data_dir,
        Regtest,
        &keys.master_fingerprint,
        mnemonic(&keys),
    )
    .await
    .unwrap();
    assert_eq!(loaded.get_descriptors(), source.get_descriptors());
    let got = loaded.get_wallet_data();
    assert_eq!(got.data_dir, wd_b.data_dir);
    assert_eq!(got.max_allocations_per_utxo, 3);
}
