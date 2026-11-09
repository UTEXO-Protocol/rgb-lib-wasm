use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

mod utils;

use bdk_wallet::bitcoin::secp256k1::{Secp256k1, SecretKey, rand::rngs::OsRng};
use rgb_lib_wasm::wallet::rust_only::{ValidateConsignmentResult, validate_consignment_offchain};
use rgb_lib_wasm::wallet::vss::{
    GetObjectRequest, KeyValue, PutObjectRequest, VssBackupConfig, VssEncryptionMetadata,
    WasmVssClient, decrypt_data,
};
use rgb_lib_wasm::wallet::{
    AssetFilter, DatabaseType, IndexerConfig, Invoice, OnlineOptions, Wallet, WalletData,
};
use rgb_lib_wasm::{AssetSchema, Assignment, BitcoinNetwork, Error, TransferStatus, generate_keys};
use utils::{PROXY_URL, VSS_SERVER_URL, go_online_options, transport_endpoint};
use wasm_bindgen::JsValue;

fn test_wallet_data(schemas: Vec<AssetSchema>) -> WalletData {
    let keys = generate_keys(BitcoinNetwork::Regtest);
    WalletData {
        data_dir: "/tmp/rgb_test".to_string(),
        bitcoin_network: BitcoinNetwork::Regtest,
        database_type: DatabaseType::Sqlite,
        max_allocations_per_utxo: 5,
        account_xpub_vanilla: keys.account_xpub_vanilla,
        account_xpub_colored: keys.account_xpub_colored,
        mnemonic: Some(keys.mnemonic),
        master_fingerprint: keys.master_fingerprint,
        vanilla_keychain: None,
        supported_schemas: schemas,
        reuse_addresses: false,
    }
}

fn assert_bfa_refused_on_mainnet() {
    let keys = generate_keys(BitcoinNetwork::Mainnet);
    let wd = WalletData {
        bitcoin_network: BitcoinNetwork::Mainnet,
        account_xpub_vanilla: keys.account_xpub_vanilla,
        account_xpub_colored: keys.account_xpub_colored,
        mnemonic: Some(keys.mnemonic),
        master_fingerprint: keys.master_fingerprint,
        ..test_wallet_data(vec![AssetSchema::Bfa])
    };
    assert!(matches!(Wallet::new(wd), Err(Error::CannotUseBfaOnMainnet)));
}

#[wasm_bindgen_test]
fn test_wallet_new_signing() {
    let wd = test_wallet_data(vec![AssetSchema::Nia]);
    assert!(Wallet::new(wd).is_ok());
    assert_bfa_refused_on_mainnet();
}

#[wasm_bindgen_test]
fn test_wallet_new_watch_only() {
    let keys = generate_keys(BitcoinNetwork::Regtest);
    let wd = WalletData {
        data_dir: "/tmp/rgb_test_wo".to_string(),
        bitcoin_network: BitcoinNetwork::Regtest,
        database_type: DatabaseType::Sqlite,
        max_allocations_per_utxo: 5,
        account_xpub_vanilla: keys.account_xpub_vanilla,
        account_xpub_colored: keys.account_xpub_colored,
        mnemonic: None,
        master_fingerprint: keys.master_fingerprint,
        vanilla_keychain: None,
        supported_schemas: vec![AssetSchema::Nia],
        reuse_addresses: false,
    };
    assert!(Wallet::new(wd).is_ok());
}

#[wasm_bindgen_test]
fn test_wallet_new_fingerprint_mismatch() {
    let keys = generate_keys(BitcoinNetwork::Regtest);
    let wd = WalletData {
        data_dir: "/tmp/rgb_test_fp".to_string(),
        bitcoin_network: BitcoinNetwork::Regtest,
        database_type: DatabaseType::Sqlite,
        max_allocations_per_utxo: 5,
        account_xpub_vanilla: keys.account_xpub_vanilla,
        account_xpub_colored: keys.account_xpub_colored,
        mnemonic: Some(keys.mnemonic),
        master_fingerprint: "deadbeef".to_string(),
        vanilla_keychain: None,
        supported_schemas: vec![AssetSchema::Nia],
        reuse_addresses: false,
    };
    let result = Wallet::new(wd);
    assert!(result.is_err());
    let err = format!("{:?}", result.err().unwrap());
    assert!(err.contains("FingerprintMismatch"));
}

/// Tests wallet queries on a fresh wallet: address, balance, transactions,
/// transfers, unspents, list_assets, get_wallet_data, delete_transfers, backup_info
#[wasm_bindgen_test]
fn test_wallet_queries_fresh() {
    let wd = test_wallet_data(vec![AssetSchema::Nia, AssetSchema::Ifa]);
    let mut wallet = Wallet::new(wd.clone()).unwrap();

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
    assert_eq!(returned.account_xpub_vanilla, wd.account_xpub_vanilla);
    assert_eq!(returned.account_xpub_colored, wd.account_xpub_colored);
    assert_eq!(returned.mnemonic, wd.mnemonic);
    assert_eq!(returned.master_fingerprint, wd.master_fingerprint);
    assert_eq!(returned.supported_schemas, wd.supported_schemas);

    // get_btc_balance: zero on unfunded wallet
    let balance = wallet.get_btc_balance(None, true).unwrap();
    assert_eq!(balance.vanilla.settled, 0);
    assert_eq!(balance.vanilla.future, 0);
    assert_eq!(balance.colored.settled, 0);
    assert_eq!(balance.colored.future, 0);

    // list_transactions: empty on fresh wallet
    let txs = wallet.list_transactions(None, true).unwrap();
    assert!(txs.is_empty());

    // list_transfers: empty on fresh wallet
    let transfers = wallet.list_transfers(AssetFilter::NoAsset, None).unwrap();
    assert!(transfers.is_empty());

    // list_unspents: empty on fresh wallet
    let unspents = wallet.list_unspents(None, false, true).unwrap();
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
    let mut wallet = Wallet::new(test_wallet_data(vec![AssetSchema::Nia])).unwrap();
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
        )
        .await
        .unwrap();
    assert!(!recv.invoice.is_empty());
    assert_eq!(recv.public_key.len(), 64);
    assert!(!recv.recipient_id.is_empty());
    assert_eq!(recv.expiration_timestamp, expiration);

    // list_transfers: should have a pending transfer from witness_receive
    let transfers = wallet.list_transfers(AssetFilter::NoAsset, None).unwrap();
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

/// Tests error paths and backup: invalid PSBT, issue without UTXOs,
/// nonexistent asset balance, backup/restore round-trip
#[wasm_bindgen_test]
async fn test_errors_and_backup() {
    let mut wallet = Wallet::new(test_wallet_data(vec![AssetSchema::Nia])).unwrap();
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

    // Wallet::new: BFA on mainnet
    assert_bfa_refused_on_mainnet();

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
    let transfers = wallet.list_transfers(AssetFilter::NoAsset, None).unwrap();
    assert!(
        !transfers.is_empty(),
        "Transfers should survive backup/restore"
    );
}

#[wasm_bindgen_test]
fn test_restore_backup_wrapping_length_fails() {
    let mut wallet = Wallet::new(test_wallet_data(vec![AssetSchema::Nia])).unwrap();
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

fn run_js(body: &str) -> JsValue {
    js_sys::Function::new_no_args(body)
        .call0(&JsValue::NULL)
        .unwrap()
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
    let wallet = Wallet::new(test_wallet_data(vec![AssetSchema::Nia])).unwrap();
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
    let mut wallet = Wallet::restore(test_wallet_data(vec![AssetSchema::Nia]))
        .await
        .unwrap();
    let after_create = persist_calls();
    let backup = wallet.backup("password").unwrap();
    let restored = wallet.restore_backup(&backup, "password");
    let after_restore = persist_calls();
    run_js("delete navigator.storage.persist;");
    assert!(restored.is_ok(), "{restored:?}");
    assert_eq!(after_create, 1.0);
    assert_eq!(after_restore, 2.0);
}

// ---------------------------------------------------------------------------
// Address reuse e2e tests
// ---------------------------------------------------------------------------

fn test_wallet_data_reuse(schemas: Vec<AssetSchema>) -> WalletData {
    let keys = generate_keys(BitcoinNetwork::Regtest);
    WalletData {
        data_dir: "/tmp/rgb_test_reuse".to_string(),
        bitcoin_network: BitcoinNetwork::Regtest,
        database_type: DatabaseType::Sqlite,
        max_allocations_per_utxo: 5,
        account_xpub_vanilla: keys.account_xpub_vanilla,
        account_xpub_colored: keys.account_xpub_colored,
        mnemonic: Some(keys.mnemonic),
        master_fingerprint: keys.master_fingerprint,
        vanilla_keychain: None,
        supported_schemas: schemas,
        reuse_addresses: true,
    }
}

/// With reuse enabled, get_address returns the same address every time.
#[wasm_bindgen_test]
fn test_address_reuse_returns_same() {
    let wd = test_wallet_data_reuse(vec![AssetSchema::Nia]);
    let mut wallet = Wallet::new(wd).unwrap();

    let addr1 = wallet.get_address().unwrap();
    let addr2 = wallet.get_address().unwrap();
    let addr3 = wallet.get_address().unwrap();
    assert!(addr1.starts_with("bcrt1"));
    assert_eq!(addr1, addr2);
    assert_eq!(addr2, addr3);
}

/// Without reuse, addresses differ (existing behavior preserved).
#[wasm_bindgen_test]
fn test_address_no_reuse_returns_different() {
    let wd = test_wallet_data(vec![AssetSchema::Nia]);
    let mut wallet = Wallet::new(wd).unwrap();

    let addr1 = wallet.get_address().unwrap();
    let addr2 = wallet.get_address().unwrap();
    assert_ne!(addr1, addr2);
}

/// rotate_address bumps to next address; get_address then returns the new one.
#[wasm_bindgen_test]
fn test_rotate_address() {
    use rgb_lib_wasm::bdk_wallet::KeychainKind;

    let wd = test_wallet_data_reuse(vec![AssetSchema::Nia]);
    let mut wallet = Wallet::new(wd).unwrap();

    let old_addr = wallet.get_address().unwrap();
    let rotated = wallet.rotate_address(KeychainKind::Internal).unwrap();
    let new_addr = wallet.get_address().unwrap();

    assert_ne!(
        old_addr, rotated,
        "rotate should produce a different address"
    );
    assert_eq!(
        rotated, new_addr,
        "get_address should return the rotated address"
    );

    // Rotate again — should produce yet another address
    let rotated2 = wallet.rotate_address(KeychainKind::Internal).unwrap();
    assert_ne!(rotated, rotated2, "second rotation should differ");
}

/// rotate_address errors when reuse_addresses is false.
#[wasm_bindgen_test]
fn test_rotate_address_disabled_errors() {
    use rgb_lib_wasm::bdk_wallet::KeychainKind;

    let wd = test_wallet_data(vec![AssetSchema::Nia]);
    let mut wallet = Wallet::new(wd).unwrap();

    let result = wallet.rotate_address(KeychainKind::Internal);
    assert!(result.is_err());
    assert!(format!("{:?}", result.unwrap_err()).contains("AddressReuseDisabled"));
}

/// External and Internal keychains have independent pinned indices.
#[wasm_bindgen_test]
fn test_keychains_independent() {
    use rgb_lib_wasm::bdk_wallet::KeychainKind;

    let wd = test_wallet_data_reuse(vec![AssetSchema::Nia]);
    let mut wallet = Wallet::new(wd).unwrap();

    // get_address uses Internal keychain
    let internal_addr = wallet.get_address().unwrap();

    // Rotate External — should NOT affect Internal
    wallet.rotate_address(KeychainKind::External).unwrap();
    wallet.rotate_address(KeychainKind::External).unwrap();

    let internal_after = wallet.get_address().unwrap();
    assert_eq!(
        internal_addr, internal_after,
        "rotating External must not change Internal"
    );
}

/// WalletData round-trip preserves the reuse_addresses field.
#[wasm_bindgen_test]
fn test_wallet_data_preserves_reuse_field() {
    let wd = test_wallet_data_reuse(vec![AssetSchema::Nia]);
    let wallet = Wallet::new(wd).unwrap();
    let returned = wallet.get_wallet_data();
    assert!(returned.reuse_addresses, "reuse_addresses should be true");

    let wd2 = test_wallet_data(vec![AssetSchema::Nia]);
    let wallet2 = Wallet::new(wd2).unwrap();
    let returned2 = wallet2.get_wallet_data();
    assert!(
        !returned2.reuse_addresses,
        "reuse_addresses should be false"
    );
}

/// JSON without reuse_addresses deserializes to false (backward compat).
#[wasm_bindgen_test]
fn test_reuse_addresses_serde_default() {
    // JSON without the reuse_addresses field should deserialize to false
    let keys = generate_keys(BitcoinNetwork::Regtest);
    let json = format!(
        r#"{{
            "data_dir": "/tmp/serde_test",
            "bitcoin_network": "Regtest",
            "database_type": "Sqlite",
            "max_allocations_per_utxo": 5,
            "account_xpub_vanilla": "{}",
            "account_xpub_colored": "{}",
            "mnemonic": "{}",
            "master_fingerprint": "{}",
            "vanilla_keychain": null,
            "supported_schemas": ["Nia"]
        }}"#,
        keys.account_xpub_vanilla,
        keys.account_xpub_colored,
        keys.mnemonic,
        keys.master_fingerprint,
    );
    let wd: WalletData = serde_json::from_str(&json).unwrap();
    assert!(!wd.reuse_addresses, "missing field should default to false");
}

struct VssStore {
    client: WasmVssClient,
    store_id: String,
    key: SecretKey,
}

impl VssStore {
    fn new() -> Self {
        let (key, public_key) = Secp256k1::new().generate_keypair(&mut OsRng);
        Self {
            client: WasmVssClient::new(VSS_SERVER_URL.to_string(), key),
            store_id: format!("restore-checks-{public_key}"),
            key,
        }
    }

    fn config(&self) -> VssBackupConfig {
        VssBackupConfig::new(VSS_SERVER_URL.to_string(), self.store_id.clone(), self.key)
    }

    async fn get(&self, key: &str) -> KeyValue {
        let request = GetObjectRequest {
            store_id: self.store_id.clone(),
            key: key.to_string(),
        };
        self.client
            .get_object(&request)
            .await
            .unwrap()
            .value
            .unwrap()
    }

    async fn put(&self, key: &str, value: Vec<u8>) {
        let request = PutObjectRequest {
            store_id: self.store_id.clone(),
            global_version: None,
            transaction_items: vec![KeyValue {
                key: key.to_string(),
                version: self.get(key).await.version,
                value,
            }],
            delete_items: vec![],
        };
        self.client.put_object(&request).await.unwrap();
    }

    async fn metadata(&self) -> VssEncryptionMetadata {
        serde_json::from_slice(&self.get("backup/metadata").await.value).unwrap()
    }

    async fn manifest(&self) -> serde_json::Value {
        serde_json::from_slice(&self.get("backup/manifest").await.value).unwrap()
    }

    async fn plaintext(&self) -> serde_json::Value {
        let data = self.get("backup/data").await.value;
        let plain = decrypt_data(&data, &self.key, &self.metadata().await, None).unwrap();
        serde_json::from_slice(&plain).unwrap()
    }

    async fn set_manifest(&self, field: &str, value: serde_json::Value) {
        let mut manifest = self.manifest().await;
        manifest[field] = value;
        self.put("backup/manifest", serde_json::to_vec(&manifest).unwrap())
            .await;
    }

    async fn put_data(&self, data: Vec<u8>) {
        let len = data.len();
        self.put("backup/data", data).await;
        self.set_manifest("total_size", len.into()).await;
    }
}

async fn wallet_with_vss_backup() -> (Wallet, VssStore) {
    let mut wallet = Wallet::new(test_wallet_data(vec![AssetSchema::Nia])).unwrap();
    let store = VssStore::new();
    wallet.configure_vss_backup(&store.config());
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
        )
        .await
        .unwrap()
        .recipient_id
}

fn has_transfer(wallet: &Wallet, recipient_id: &str) -> bool {
    wallet
        .list_transfers(AssetFilter::NoAsset, None)
        .unwrap()
        .iter()
        .any(|t| t.recipient_id.as_deref() == Some(recipient_id))
}

/// A VSS store that holds another wallet's plaintext state does not restore.
#[wasm_bindgen_test]
async fn test_vss_restore_refuses_other_wallet_state() {
    let (mut a, store_a) = wallet_with_vss_backup().await;
    let recv_id = witness_receive_on(&mut a).await;
    let (b, store_b) = wallet_with_vss_backup().await;

    let mut payload = store_b.plaintext().await;
    payload["bdk_changeset"] = serde_json::Value::Null;
    store_a
        .put_data(serde_json::to_vec(&payload).unwrap())
        .await;
    store_a.set_manifest("encrypted", false.into()).await;
    store_a.set_manifest("version", 99.into()).await;
    let fingerprint = b.get_wallet_data().master_fingerprint;
    store_a
        .put("backup/fingerprint", fingerprint.into_bytes())
        .await;

    let result = a.vss_restore_backup().await;
    assert!(result.is_err(), "restore of a tampered store succeeded");
    assert!(has_transfer(&a, &recv_id));
}

#[wasm_bindgen_test]
async fn test_vss_restore_refuses_unencrypted_data() {
    let (mut wallet, store) = wallet_with_vss_backup().await;
    let data = serde_json::to_vec(&store.plaintext().await).unwrap();
    store.put_data(data).await;
    store.set_manifest("encrypted", false.into()).await;
    let err = wallet.vss_restore_backup().await.unwrap_err();
    assert!(matches!(err, Error::InvalidBackup), "{err:?}");
}

#[wasm_bindgen_test]
async fn test_vss_restore_refuses_other_fingerprint() {
    let (mut wallet, store) = wallet_with_vss_backup().await;
    let other = generate_keys(BitcoinNetwork::Regtest).master_fingerprint;
    store.put("backup/fingerprint", other.into_bytes()).await;
    let err = wallet.vss_restore_backup().await.unwrap_err();
    assert!(matches!(err, Error::FingerprintMismatch), "{err:?}");
}

#[wasm_bindgen_test]
async fn test_vss_restore_refuses_unknown_version() {
    let (mut wallet, store) = wallet_with_vss_backup().await;
    store.set_manifest("version", 99.into()).await;
    let err = wallet.vss_restore_backup().await.unwrap_err();
    assert!(
        matches!(err, Error::UnsupportedBackupVersion { .. }),
        "{err:?}"
    );
}

#[wasm_bindgen_test]
async fn test_vss_restore_refuses_wrong_chunk_count() {
    let (mut wallet, store) = wallet_with_vss_backup().await;
    store.set_manifest("chunk_count", 2.into()).await;
    let err = wallet.vss_restore_backup().await.unwrap_err();
    assert!(matches!(err, Error::InvalidBackup), "{err:?}");
}

#[wasm_bindgen_test]
async fn test_vss_restore_refuses_wrong_total_size() {
    let (mut wallet, store) = wallet_with_vss_backup().await;
    let size = store.manifest().await["total_size"].as_u64().unwrap();
    store.set_manifest("total_size", (size + 1).into()).await;
    let err = wallet.vss_restore_backup().await.unwrap_err();
    assert!(matches!(err, Error::InvalidBackup), "{err:?}");
}

/// Return `None` when `fut` is not ready within `ms`.
async fn within<F: std::future::Future>(ms: u32, fut: F) -> Option<F::Output> {
    let timer = js_sys::Function::new_with_args("ms", "return new Promise(r => setTimeout(r, ms))")
        .call1(&JsValue::NULL, &ms.into())
        .unwrap();
    let mut timer = std::pin::pin!(wasm_bindgen_futures::JsFuture::from(js_sys::Promise::from(
        timer
    )));
    let mut fut = std::pin::pin!(fut);
    std::future::poll_fn(|cx| {
        if let std::task::Poll::Ready(out) = fut.as_mut().poll(cx) {
            return std::task::Poll::Ready(Some(out));
        }
        timer.as_mut().poll(cx).map(|_| None)
    })
    .await
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
    let mut wallet = Wallet::new(test_wallet_data(vec![AssetSchema::Nia])).unwrap();
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
