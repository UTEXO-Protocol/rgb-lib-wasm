use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

use rgb_lib_wasm::wallet::{DatabaseType, Invoice, Wallet, WalletData};
use rgb_lib_wasm::{AssetSchema, Assignment, BitcoinNetwork, TransferStatus, generate_keys};

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
    }
}

#[wasm_bindgen_test]
fn test_wallet_new_signing() {
    let wd = test_wallet_data(vec![AssetSchema::Nia]);
    assert!(Wallet::new(wd).is_ok());
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
    let transfers = wallet.list_transfers(None).unwrap();
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

/// Tests receive operations: witness_receive (works offline), list_transfers
/// after receive, and error cases for invalid asset IDs.
/// Note: blind_receive requires funded UTXOs so it's tested in integration_wasm.
#[wasm_bindgen_test]
fn test_receive_and_transfers() {
    let mut wallet = Wallet::new(test_wallet_data(vec![AssetSchema::Nia])).unwrap();

    // witness_receive: success
    let recv = wallet
        .witness_receive(
            None,
            Assignment::Any,
            None,
            vec!["rpc://127.0.0.1".into()],
            1,
        )
        .unwrap();
    assert!(!recv.invoice.is_empty());
    assert!(!recv.recipient_id.is_empty());
    assert!(recv.expiration_timestamp.is_some());

    // list_transfers: should have a pending transfer from witness_receive
    let transfers = wallet.list_transfers(None).unwrap();
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

    // witness_receive: invalid asset
    let err = wallet
        .witness_receive(
            Some("nonexistent_asset_id".into()),
            Assignment::Fungible(100),
            None,
            vec!["rpc://127.0.0.1".into()],
            1,
        )
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
    assert_eq!(data.expiration_timestamp, recv.expiration_timestamp);

    // Invoice: invalid string should fail
    assert!(Invoice::new("not-a-valid-invoice".to_string()).is_err());
}

/// Tests error paths and backup: invalid PSBT, issue without UTXOs,
/// nonexistent asset balance, backup/restore round-trip
#[wasm_bindgen_test]
fn test_errors_and_backup() {
    let mut wallet = Wallet::new(test_wallet_data(vec![AssetSchema::Nia])).unwrap();

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
            None,
            Assignment::Any,
            None,
            vec!["rpc://127.0.0.1".into()],
            1,
        )
        .unwrap();

    let password = "test_pass_123";
    let backup_bytes = wallet.backup(password).unwrap();
    assert!(!backup_bytes.is_empty());

    // Wrong password fails
    assert!(wallet.restore_backup(&backup_bytes, "wrong").is_err());

    // Correct password restores
    wallet.restore_backup(&backup_bytes, password).unwrap();

    // State survives: transfer still exists
    let transfers = wallet.list_transfers(None).unwrap();
    assert!(
        !transfers.is_empty(),
        "Transfers should survive backup/restore"
    );
}
