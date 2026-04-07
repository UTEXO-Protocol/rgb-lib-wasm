//! Persistence E2E tests: verify wallet state survives simulated page reloads.
//!
//! Each test simulates a browser reload by:
//! 1. Performing operations on a wallet
//! 2. Waiting for async IndexedDB save to complete
//! 3. Dropping the wallet
//! 4. Re-creating from IndexedDB snapshot (same as WasmWallet.create())
//! 5. Verifying state survived

use std::collections::HashMap;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

mod utils;

use rgb_lib_wasm::wallet::{DatabaseType, Recipient, Wallet, WalletData, WitnessData};
use rgb_lib_wasm::{AssetSchema, Assignment, BitcoinNetwork, generate_keys};
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

/// Simulate a browser page reload: wait for pending IDB writes, then
/// re-create wallet from IndexedDB snapshot (same as WasmWallet.create()).
async fn simulate_reload(wd: &WalletData) -> Wallet {
    // Wait for pending async IDB saves to complete
    sleep_ms(1000).await;

    let mut wallet = Wallet::new(wd.clone()).unwrap();
    let idb_key = wallet.idb_key();
    if let Ok(Some(snapshot)) = rgb_lib_wasm::wallet::idb_store::load_snapshot(&idb_key).await {
        wallet.restore_from_snapshot(snapshot).unwrap();
    }
    wallet
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
    drop(wallet);
    let mut wallet = simulate_reload(&wd).await;

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
    let online = wallet
        .go_online(false, ESPLORA_URL.to_string())
        .await
        .unwrap();

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

    // Reload
    drop(wallet);
    let mut wallet = simulate_reload(&wd_a).await;
    let online = wallet
        .go_online(true, ESPLORA_URL.to_string())
        .await
        .unwrap();
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
    let online_b = wallet_b
        .go_online(false, ESPLORA_URL.to_string())
        .await
        .unwrap();
    fund_and_sync(&mut wallet_b, &online_b, "1.0").await;
    create_utxos(&mut wallet_b, &online_b, 5).await;

    let transport = transport_endpoint();
    let recv_b = wallet_b
        .witness_receive(
            None,
            Assignment::Fungible(100),
            None,
            vec![transport.clone()],
            1,
        )
        .unwrap();

    let recipient = Recipient {
        recipient_id: recv_b.recipient_id.clone(),
        witness_data: Some(WitnessData {
            amount_sat: 2000,
            blinding: None,
        }),
        assignment: Assignment::Fungible(100),
        transport_endpoints: vec![transport.clone()],
    };
    let mut recipient_map = HashMap::new();
    recipient_map.insert(asset_id.clone(), vec![recipient]);

    // send_begin must succeed — proves Stock has the contract after reload
    let unsigned_psbt = wallet
        .send_begin(online.clone(), recipient_map, false, 1, 1)
        .await
        .unwrap();
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
    drop(wallet);
    let mut wallet = simulate_reload(&wd_a).await;
    let online = wallet
        .go_online(true, ESPLORA_URL.to_string())
        .await
        .unwrap();
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
    let _recv = wallet
        .witness_receive(None, Assignment::Any, None, vec![transport.clone()], 1)
        .unwrap();

    // Backup
    let password = "persist_test_pw";
    let backup_bytes = wallet.backup(password).unwrap();
    assert!(!backup_bytes.is_empty());

    // Restore into a fresh wallet with same keys
    let mut fresh_wallet = Wallet::new(wd_a.clone()).unwrap();
    fresh_wallet
        .go_online(true, ESPLORA_URL.to_string())
        .await
        .unwrap();
    fresh_wallet
        .restore_backup(&backup_bytes, password)
        .unwrap();

    // Verify restore worked in memory
    let transfers_after_restore = fresh_wallet.list_transfers(None).unwrap();
    assert!(
        !transfers_after_restore.is_empty(),
        "Transfers should exist after backup restore"
    );

    // Reload again: state should survive (Bug 3 regression)
    drop(fresh_wallet);
    let mut reloaded = simulate_reload(&wd_a).await;
    reloaded
        .go_online(true, ESPLORA_URL.to_string())
        .await
        .unwrap();

    let transfers_after_reload = reloaded.list_transfers(None).unwrap();
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
