//! Persistence E2E tests: verify wallet state survives simulated page reloads.
//!
//! Each test simulates a browser reload by:
//! 1. Performing operations on a wallet
//! 2. Explicitly flushing the wallet to IndexedDB
//! 3. Dropping the wallet
//! 4. Restoring through the production IndexedDB API
//! 5. Verifying state survived

use std::collections::HashMap;
use std::str::FromStr;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

mod utils;

use rgb_lib_wasm::bitcoin::psbt::Psbt;
use rgb_lib_wasm::wallet::idb_store::load_snapshot;
use rgb_lib_wasm::wallet::{AssetFilter, DatabaseType, Recipient, Wallet, WalletData, WitnessData};
use rgb_lib_wasm::{
    AssetSchema, Assignment, BitcoinNetwork, Encryption, Error, TransferStatus, generate_keys,
};
use utils::*;

fn test_wallet_data(
    keys: &rgb_lib_wasm::keys::Keys,
    schemas: Vec<AssetSchema>,
    data_dir: &str,
) -> WalletData {
    WalletData {
        data_dir: data_dir.to_string(),
        bitcoin_network: BitcoinNetwork::Regtest,
        database_type: DatabaseType::Sqlite,
        max_allocations_per_utxo: 5,
        account_xpub_vanilla: keys.account_xpub_vanilla.clone(),
        account_xpub_colored: keys.account_xpub_colored.clone(),
        mnemonic: Some(keys.mnemonic.clone()),
        master_fingerprint: keys.master_fingerprint.clone(),
        vanilla_keychain: None,
        supported_schemas: schemas,
        reuse_addresses: false,
    }
}

fn transport_endpoint() -> String {
    format!("rpc://{}", PROXY_URL.trim_start_matches("http://"))
}

async fn fund_and_sync(wallet: &mut Wallet, online: &rgb_lib_wasm::wallet::Online, amount: &str) {
    let addr = wallet.get_address().unwrap();
    fund_address(&addr, amount).await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();
}

async fn create_utxos(wallet: &mut Wallet, online: &rgb_lib_wasm::wallet::Online, num: u8) {
    let unsigned = wallet
        .create_utxos_begin(online.clone(), true, Some(num), None, 1, true)
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
}

/// Simulate a browser page reload through the production restore API.
async fn simulate_reload(wallet: Wallet, wd: &WalletData) -> Wallet {
    wallet.flush().await.unwrap();
    drop(wallet);
    Wallet::restore(wd.clone()).await.unwrap()
}

async fn flushed_wallet_with_asset(name: &str) -> (WalletData, String) {
    let keys = generate_keys(BitcoinNetwork::Regtest);
    let wd = test_wallet_data(
        &keys,
        vec![AssetSchema::Nia],
        &format!("/tmp/{name}-{}", js_sys::Date::now()),
    );
    let mut wallet = Wallet::new(wd.clone()).unwrap();
    let online = wallet.go_online(go_online_options(false)).await.unwrap();
    fund_and_sync(&mut wallet, &online, "1.0").await;
    create_utxos(&mut wallet, &online, 1).await;
    let asset = wallet
        .issue_asset_nia("STAY".into(), "Stay".into(), 0, vec![1000])
        .unwrap();
    wallet.flush().await.unwrap();
    drop(wallet);
    (wd, asset.asset_id)
}

async fn stored_sequence(key: &str) -> u64 {
    load_snapshot(key).await.unwrap().unwrap().sequence
}

fn has_asset(wallet: &Wallet, asset_id: &str) -> bool {
    wallet
        .list_assets(vec![AssetSchema::Nia])
        .unwrap()
        .nia
        .unwrap_or_default()
        .iter()
        .any(|a| a.asset_id == asset_id)
}

/// Test: flush fails when it does not write the snapshot.
#[wasm_bindgen_test]
async fn test_flush_errors_when_snapshot_not_written() {
    let (wd, _) = flushed_wallet_with_asset("flush-unwritten").await;
    let second = Wallet::new(wd.clone()).unwrap();
    let key = second.idb_key();
    let before = stored_sequence(&key).await;
    let result = second.flush().await;
    assert_eq!(stored_sequence(&key).await, before);
    assert!(
        matches!(result, Err(Error::Persistence { .. })),
        "flush returned {result:?} but wrote nothing"
    );
}

/// Test: a wallet that did not load the snapshot cannot overwrite it.
#[wasm_bindgen_test]
async fn test_unloaded_wallet_cannot_overwrite_snapshot() {
    let (wd, asset_id) = flushed_wallet_with_asset("flush-overwrite").await;
    let second = Wallet::new(wd.clone()).unwrap();
    let stored = stored_sequence(&second.idb_key()).await;
    for _ in 0..=stored {
        let _ = second.flush().await;
    }
    drop(second);
    let restored = Wallet::restore(wd).await.unwrap();
    assert!(
        has_asset(&restored, &asset_id),
        "a wallet that did not load the snapshot overwrote it"
    );
}

/// Test: Address reuse pinned index survives page reload.
#[wasm_bindgen_test]
async fn test_address_reuse_persists_across_reload() {
    use rgb_lib_wasm::bdk_wallet::KeychainKind;

    let keys = generate_keys(BitcoinNetwork::Regtest);
    let wd = WalletData {
        data_dir: "/tmp/persist_reuse".to_string(),
        bitcoin_network: BitcoinNetwork::Regtest,
        database_type: DatabaseType::Sqlite,
        max_allocations_per_utxo: 5,
        account_xpub_vanilla: keys.account_xpub_vanilla.clone(),
        account_xpub_colored: keys.account_xpub_colored.clone(),
        mnemonic: Some(keys.mnemonic.clone()),
        master_fingerprint: keys.master_fingerprint.clone(),
        vanilla_keychain: None,
        supported_schemas: vec![AssetSchema::Nia],
        reuse_addresses: true,
    };

    // Session 1: get pinned address, rotate, verify new pinned address
    let mut wallet = Wallet::new(wd.clone()).unwrap();
    let addr_initial = wallet.get_address().unwrap();
    assert!(addr_initial.starts_with("bcrt1"));

    // Same address on repeated calls
    assert_eq!(addr_initial, wallet.get_address().unwrap());

    // Rotate to index 1
    let rotated = wallet.rotate_address(KeychainKind::Internal).unwrap();
    assert_ne!(addr_initial, rotated);
    assert_eq!(rotated, wallet.get_address().unwrap());

    // Simulate browser refresh
    let mut wallet = simulate_reload(wallet, &wd).await;

    // After reload, rotated address should be preserved
    let addr_after_reload = wallet.get_address().unwrap();
    assert_eq!(
        rotated, addr_after_reload,
        "rotated address must survive page reload"
    );

    // Reuse still works after reload
    assert_eq!(addr_after_reload, wallet.get_address().unwrap());

    // Rotate again after reload — should work and produce a new address
    let rotated2 = wallet.rotate_address(KeychainKind::Internal).unwrap();
    assert_ne!(addr_after_reload, rotated2);
    assert_eq!(rotated2, wallet.get_address().unwrap());
}

/// Test: Address reuse pinned index survives encrypted backup + restore.
#[wasm_bindgen_test]
async fn test_address_reuse_persists_across_backup_restore() {
    use rgb_lib_wasm::bdk_wallet::KeychainKind;

    let keys = generate_keys(BitcoinNetwork::Regtest);
    let wd = WalletData {
        data_dir: "/tmp/persist_reuse_backup".to_string(),
        bitcoin_network: BitcoinNetwork::Regtest,
        database_type: DatabaseType::Sqlite,
        max_allocations_per_utxo: 5,
        account_xpub_vanilla: keys.account_xpub_vanilla.clone(),
        account_xpub_colored: keys.account_xpub_colored.clone(),
        mnemonic: Some(keys.mnemonic.clone()),
        master_fingerprint: keys.master_fingerprint.clone(),
        vanilla_keychain: None,
        supported_schemas: vec![AssetSchema::Nia],
        reuse_addresses: true,
    };

    // Create wallet, rotate address a few times
    let mut wallet = Wallet::new(wd.clone()).unwrap();
    let addr_index_0 = wallet.get_address().unwrap();
    wallet.rotate_address(KeychainKind::Internal).unwrap();
    wallet.rotate_address(KeychainKind::Internal).unwrap();
    let addr_index_2 = wallet.get_address().unwrap();
    assert_ne!(addr_index_0, addr_index_2);

    // Create encrypted backup
    let password = "test_password_42";
    let backup_bytes = wallet.backup(password).unwrap();

    // Restore into a fresh wallet
    let mut wallet2 = Wallet::new(wd.clone()).unwrap();
    // Fresh wallet starts at index 0
    assert_eq!(addr_index_0, wallet2.get_address().unwrap());

    // Restore from backup — should bring back index 2
    wallet2.restore_backup(&backup_bytes, password).unwrap();
    let addr_after_restore = wallet2.get_address().unwrap();
    assert_eq!(
        addr_index_2, addr_after_restore,
        "backup/restore must preserve the pinned address index"
    );

    // Rotate should continue from index 2 → 3
    let rotated = wallet2.rotate_address(KeychainKind::Internal).unwrap();
    assert_ne!(addr_index_2, rotated);
    assert_eq!(rotated, wallet2.get_address().unwrap());
}

/// A wallet restored with a thin BDK state (no revealed SPKs) cannot recover its BTC balance
/// through an incremental `sync`; `full_scan` rebuilds it from the indexer.
#[wasm_bindgen_test]
async fn test_btc_balance_recovers_via_full_scan_after_thin_restore() {
    let unique = js_sys::Date::now().to_string();
    let keys = generate_keys(BitcoinNetwork::Regtest);

    // Fund a vanilla address and confirm a non-zero BTC balance.
    let wd1 = test_wallet_data(
        &keys,
        vec![AssetSchema::Nia],
        &format!("/tmp/fullscan-funded-{unique}"),
    );
    let mut funded = Wallet::new(wd1.clone()).unwrap();
    let online = funded.go_online(go_online_options(false)).await.unwrap();
    let addr = funded.get_address().unwrap();
    fund_address(&addr, "1.0").await;
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    funded.sync(online.clone()).await.unwrap();
    let funded_balance = funded.get_btc_balance(None, true).unwrap();
    assert!(funded_balance.vanilla.settled > 0);

    // Same keys, fresh BDK store (distinct data_dir): a thin state with no revealed SPKs.
    let wd2 = test_wallet_data(
        &keys,
        vec![AssetSchema::Nia],
        &format!("/tmp/fullscan-thin-{unique}"),
    );
    let mut thin = Wallet::new(wd2).unwrap();
    let online = thin.go_online(go_online_options(false)).await.unwrap();

    thin.sync(online.clone()).await.unwrap();
    let after_incremental = thin.get_btc_balance(None, true).unwrap();
    assert_eq!(
        after_incremental.vanilla.settled, 0,
        "incremental sync cannot recover a thin BDK state"
    );

    thin.full_scan(online.clone()).await.unwrap();
    let after_full_scan = thin.get_btc_balance(None, true).unwrap();
    assert_eq!(
        after_full_scan.vanilla.settled, funded_balance.vanilla.settled,
        "full_scan recovers the BTC balance"
    );
}

/// Test: BDK wallet state (BTC balance) survives page reload.
/// Test: Issued RGB asset survives reload — list_assets, get_asset_balance,
///       get_asset_metadata, and send_begin all work after reload.
/// Test: Backup restore state persists to IndexedDB across reload.
/// Test: Signed PSBT survives reload during two-wallet send flow.
#[wasm_bindgen_test]
async fn test_persistence_across_reload() {
    // Generate keys once — reused across reloads (same wallet identity)
    let keys_a = generate_keys(BitcoinNetwork::Regtest);
    let keys_b = generate_keys(BitcoinNetwork::Regtest);
    let wd_a = test_wallet_data(&keys_a, vec![AssetSchema::Nia], "/tmp/persist_a");
    let mut wallet = Wallet::new(wd_a.clone()).unwrap();
    let online = wallet.go_online(go_online_options(false)).await.unwrap();

    // Fund, create UTXOs, and issue asset — all in one session
    fund_and_sync(&mut wallet, &online, "1.0").await;
    create_utxos(&mut wallet, &online, 7).await;

    let nia = wallet
        .issue_asset_nia(
            "PRST".to_string(),
            "Persist Token".to_string(),
            0,
            vec![1000],
        )
        .unwrap();
    assert_eq!(nia.balance.settled, 1000);
    let asset_id = nia.asset_id.clone();
    let btc_balance = wallet.get_btc_balance(None, true).unwrap();

    // Reload
    let mut wallet = simulate_reload(wallet, &wd_a).await;
    // No sync yet, so the balance comes from the BDK changeset only
    assert_eq!(wallet.get_btc_balance(None, true).unwrap(), btc_balance);
    let online = wallet.go_online(go_online_options(true)).await.unwrap();
    wallet.sync(online.clone()).await.unwrap();

    // list_assets: asset should exist
    let assets = wallet.list_assets(vec![AssetSchema::Nia]).unwrap();
    let nia_list = assets.nia.unwrap();
    assert!(
        nia_list.iter().any(|a| a.asset_id == asset_id),
        "Issued asset should survive reload in list_assets"
    );

    // get_asset_balance: balance should match
    let balance = wallet.get_asset_balance(asset_id.clone()).unwrap();
    assert_eq!(balance.settled, 1000, "Asset balance should survive reload");

    // get_asset_metadata: metadata should be available
    let metadata = wallet.get_asset_metadata(asset_id.clone()).unwrap();
    assert_eq!(metadata.name, "Persist Token");
    assert_eq!(metadata.ticker, Some("PRST".to_string()));

    // send_begin: Stock should have the contract (this was Bug 2)
    let wd_b = test_wallet_data(&keys_b, vec![AssetSchema::Nia], "/tmp/persist_b");
    let mut wallet_b = Wallet::new(wd_b).unwrap();
    let online_b = wallet_b.go_online(go_online_options(false)).await.unwrap();
    fund_and_sync(&mut wallet_b, &online_b, "1.0").await;
    create_utxos(&mut wallet_b, &online_b, 5).await;

    let transport = transport_endpoint();
    let recv_b = witness_rcv(
        &mut wallet_b,
        &online_b,
        None,
        Assignment::Fungible(100),
        default_rcv_expiration(),
        vec![transport.clone()],
    )
    .await
    .unwrap();

    let recipient = Recipient {
        recipient_id: recv_b.recipient_id.clone(),
        witness_data: Some(WitnessData {
            amount_sat: 2000,
            blinding: None,
        }),
        assignment: Assignment::Fungible(100),
        transport_endpoints: vec![transport.clone()],
        public_key: Some(recv_b.public_key.clone()),
        encryption: Encryption::Required,
        expiration_timestamp: Some(recv_b.expiration_timestamp),
    };
    let mut recipient_map = HashMap::new();
    recipient_map.insert(asset_id.clone(), vec![recipient]);

    // send_begin must succeed — proves Stock has the contract after reload
    let unsigned_psbt = wallet
        .send_begin(
            online.clone(),
            recipient_map,
            false,
            1,
            1,
            default_send_expiration(),
            Some(transport.clone()),
            None,
            false,
        )
        .await
        .unwrap()
        .psbt;
    assert!(
        !unsigned_psbt.is_empty(),
        "send_begin should succeed after reload (Stock has contract)"
    );

    // Complete the send
    let signed_psbt = wallet.sign_psbt(unsigned_psbt, None).unwrap();
    let send_result = wallet
        .send_end(online.clone(), signed_psbt, false)
        .await
        .unwrap();
    assert!(!send_result.txid.is_empty());

    // === Section 3: Signed PSBT survives reload during send ===
    // Reload wallet A after send_end but before refresh
    let mut wallet = simulate_reload(wallet, &wd_a).await;
    let online = wallet.go_online(go_online_options(true)).await.unwrap();
    wallet.sync(online.clone()).await.unwrap();

    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();
    wallet_b.sync(online_b.clone()).await.unwrap();

    // Receiver refreshes: picks up consignment, ACKs
    let recv_refresh = wallet_b
        .refresh(online_b.clone(), None, vec![], false)
        .await
        .unwrap();
    for (idx, refreshed) in &recv_refresh {
        assert!(
            refreshed.failure.is_none(),
            "Receiver refresh failed for transfer {idx}: {:?}",
            refreshed.failure,
        );
    }

    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();

    // Sender (reloaded) refreshes: sees ACK, broadcasts — needs signed PSBT from IDB
    let send_refresh = wallet
        .refresh(online.clone(), None, vec![], false)
        .await
        .unwrap();
    for (idx, refreshed) in &send_refresh {
        assert!(
            refreshed.failure.is_none(),
            "Sender refresh after reload failed for transfer {idx}: {:?}",
            refreshed.failure,
        );
    }

    // === Section 4: Backup restore persists to IndexedDB ===
    // Create a witness receive to add more state
    let _recv = witness_rcv(
        &mut wallet,
        &online,
        None,
        Assignment::Any,
        default_rcv_expiration(),
        vec![transport.clone()],
    )
    .await
    .unwrap();

    // Backup
    let btc_balance = wallet.get_btc_balance(None, true).unwrap();
    let password = "persist_test_pw";
    let backup_bytes = wallet.backup(password).unwrap();
    assert!(!backup_bytes.is_empty());

    // Restore into a fresh wallet with same keys
    let mut fresh_wallet = Wallet::new(wd_a.clone()).unwrap();
    fresh_wallet
        .go_online(go_online_options(true))
        .await
        .unwrap();
    fresh_wallet
        .restore_backup(&backup_bytes, password)
        .unwrap();
    // No sync yet, so the balance comes from the BDK changeset only
    assert_eq!(
        fresh_wallet.get_btc_balance(None, true).unwrap(),
        btc_balance
    );

    // Verify restore worked in memory
    let transfers_after_restore = fresh_wallet
        .list_transfers(AssetFilter::NoAsset, None)
        .unwrap();
    assert!(
        !transfers_after_restore.is_empty(),
        "Transfers should exist after backup restore"
    );

    // Reload again: state should survive (Bug 3 regression)
    let mut reloaded = simulate_reload(fresh_wallet, &wd_a).await;
    reloaded.go_online(go_online_options(true)).await.unwrap();

    let transfers_after_reload = reloaded.list_transfers(AssetFilter::NoAsset, None).unwrap();
    assert!(
        !transfers_after_reload.is_empty(),
        "Transfers should survive reload after backup restore"
    );

    let assets_after_reload = reloaded.list_assets(vec![AssetSchema::Nia]).unwrap();
    assert!(
        assets_after_reload
            .nia
            .unwrap()
            .iter()
            .any(|a| a.asset_id == asset_id),
        "Issued asset should survive reload after backup restore"
    );
}

/// Test: the contract of a received consignment is in IndexedDB once the refresh that accepts the
/// consignment has been flushed.
#[wasm_bindgen_test]
async fn test_received_contract_survives_reload() {
    let keys_a = generate_keys(BitcoinNetwork::Regtest);
    let keys_b = generate_keys(BitcoinNetwork::Regtest);
    let wd_a = test_wallet_data(&keys_a, vec![AssetSchema::Nia], "/tmp/persist_accept_a");
    let wd_b = test_wallet_data(&keys_b, vec![AssetSchema::Nia], "/tmp/persist_accept_b");
    let mut wallet_a = Wallet::new(wd_a).unwrap();
    let mut wallet_b = Wallet::new(wd_b.clone()).unwrap();
    let online_a = wallet_a.go_online(go_online_options(false)).await.unwrap();
    let online_b = wallet_b.go_online(go_online_options(false)).await.unwrap();

    fund_and_sync(&mut wallet_a, &online_a, "1.0").await;
    fund_and_sync(&mut wallet_b, &online_b, "1.0").await;
    create_utxos(&mut wallet_a, &online_a, 5).await;
    create_utxos(&mut wallet_b, &online_b, 5).await;

    let asset = wallet_a
        .issue_asset_nia(
            "LDKT".to_string(),
            "LDK Transfer".to_string(),
            0,
            vec![1000],
        )
        .unwrap();
    let receive = witness_rcv(
        &mut wallet_b,
        &online_b,
        None,
        Assignment::Fungible(100),
        default_rcv_expiration(),
        vec![transport_endpoint()],
    )
    .await
    .unwrap();
    let recipient_map = HashMap::from([(
        asset.asset_id,
        vec![utils::recipient(
            &receive.recipient_id,
            100,
            Some(2000),
            PROXY_URL,
        )],
    )]);
    let unsigned = wallet_a
        .send_begin(
            online_a.clone(),
            recipient_map,
            false,
            1,
            1,
            default_send_expiration(),
            Some(transport_endpoint()),
            None,
            false,
        )
        .await
        .unwrap()
        .psbt;
    let signed = wallet_a.sign_psbt(unsigned, None).unwrap();
    wallet_a.send_end(online_a, signed, false).await.unwrap();

    // the ACK post fails after the download
    rpc_fail(PROXY_URL, "ack.post");
    let _ = wallet_b.refresh(online_b, None, vec![], false).await;
    clear_rpc_hooks();
    wallet_b.flush().await.unwrap();
    drop(wallet_b);

    // after a reload the cached consignment completes the receive, with no download
    let mut wallet_b = Wallet::restore(wd_b.clone()).await.unwrap();
    let online_b = wallet_b.go_online(go_online_options(false)).await.unwrap();
    rpc_fail(PROXY_URL, "consignment.get");
    let refresh = wallet_b
        .refresh(online_b, None, vec![], false)
        .await
        .unwrap();
    clear_rpc_hooks();
    assert!(refresh.values().all(|r| r.failure.is_none()));
    assert_eq!(
        transfer_of(&wallet_b, &receive.recipient_id).status,
        TransferStatus::WaitingConfirmations
    );
    // the refresh imports the contract of the consignment
    let contract_ids = wallet_b.rgb_contract_ids().unwrap();
    assert_eq!(contract_ids.len(), 1);

    wallet_b.flush().await.unwrap();
    drop(wallet_b);
    let restored = Wallet::restore(wd_b).await.unwrap();
    assert_eq!(restored.rgb_contract_ids().unwrap(), contract_ids);
}

/// Test: an Initiated send survives a browser reload and completes with its operation key.
#[wasm_bindgen_test]
async fn test_initiated_send_completes_after_reload() {
    let unique = js_sys::Date::now().to_string();
    let keys_a = generate_keys(BitcoinNetwork::Regtest);
    let keys_b = generate_keys(BitcoinNetwork::Regtest);
    let wd_a = test_wallet_data(
        &keys_a,
        vec![AssetSchema::Nia],
        &format!("/tmp/persist-initiated-a-{unique}"),
    );
    let wd_b = test_wallet_data(
        &keys_b,
        vec![AssetSchema::Nia],
        &format!("/tmp/persist-initiated-b-{unique}"),
    );
    let mut wallet_a = Wallet::new(wd_a.clone()).unwrap();
    let mut wallet_b = Wallet::new(wd_b).unwrap();
    let online_a = wallet_a.go_online(go_online_options(false)).await.unwrap();
    let online_b = wallet_b.go_online(go_online_options(false)).await.unwrap();

    fund_and_sync(&mut wallet_a, &online_a, "1.0").await;
    create_utxos(&mut wallet_a, &online_a, 5).await;
    let asset = wallet_a
        .issue_asset_nia("LDKF".to_string(), "LDK Funding".to_string(), 0, vec![1000])
        .unwrap();

    let receive = witness_rcv(
        &mut wallet_b,
        &online_b,
        None,
        Assignment::Fungible(100),
        default_rcv_expiration(),
        vec![transport_endpoint()],
    )
    .await
    .unwrap();
    let recipient_map = HashMap::from([(
        asset.asset_id,
        vec![utils::recipient(
            &receive.recipient_id,
            100,
            Some(2000),
            PROXY_URL,
        )],
    )]);
    let unsigned = wallet_a
        .send_begin(
            online_a.clone(),
            recipient_map,
            false,
            1,
            1,
            default_send_expiration(),
            Some(transport_endpoint()),
            None,
            false,
        )
        .await
        .unwrap()
        .psbt;
    let signed = wallet_a.sign_psbt(unsigned, None).unwrap();
    let txid = Psbt::from_str(&signed)
        .unwrap()
        .unsigned_tx
        .compute_txid()
        .to_string();

    let mut wallet_a = simulate_reload(wallet_a, &wd_a).await;
    let restored_online = wallet_a.go_online(go_online_options(true)).await.unwrap();
    let completed = wallet_a
        .send_end(restored_online.clone(), signed, false)
        .await
        .unwrap();
    assert_eq!(completed.txid, txid);

    wallet_b
        .refresh(online_b.clone(), None, vec![], false)
        .await
        .unwrap();
    settle(&mut wallet_b, &online_b, &[receive.recipient_id.clone()]).await;
    wallet_a.flush().await.unwrap();
}
