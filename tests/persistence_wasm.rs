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
use rgb_lib_wasm::wallet::{DatabaseType, Recipient, Wallet, WalletData, WitnessData};
use rgb_lib_wasm::{AssetSchema, Assignment, BitcoinNetwork, RgbTransport, generate_keys};
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
    let online = funded
        .go_online(false, ESPLORA_URL.to_string())
        .await
        .unwrap();
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
    let online = thin
        .go_online(false, ESPLORA_URL.to_string())
        .await
        .unwrap();

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
    let mut wallet = simulate_reload(wallet, &wd_a).await;
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
        .send_begin(online.clone(), recipient_map, false, 1, 1, None)
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
    let mut wallet = simulate_reload(wallet, &wd_a).await;
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
    let mut reloaded = simulate_reload(fresh_wallet, &wd_a).await;
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

/// Test the exact Lightning incoming-funding operation: `accept_transfer` must not report success
/// until its RGB stock mutation has committed to IndexedDB.
#[wasm_bindgen_test]
async fn test_accept_transfer_is_durable_before_success() {
    let keys_a = generate_keys(BitcoinNetwork::Regtest);
    let keys_b = generate_keys(BitcoinNetwork::Regtest);
    let wd_a = test_wallet_data(&keys_a, vec![AssetSchema::Nia], "/tmp/persist_accept_a");
    let wd_b = test_wallet_data(&keys_b, vec![AssetSchema::Nia], "/tmp/persist_accept_b");
    let mut wallet_a = Wallet::new(wd_a).unwrap();
    let mut wallet_b = Wallet::new(wd_b.clone()).unwrap();
    let online_a = wallet_a
        .go_online(false, ESPLORA_URL.to_string())
        .await
        .unwrap();
    let online_b = wallet_b
        .go_online(false, ESPLORA_URL.to_string())
        .await
        .unwrap();

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
    let transport = transport_endpoint();
    let receive = wallet_b
        .witness_receive(
            None,
            Assignment::Fungible(100),
            None,
            vec![transport.clone()],
            1,
        )
        .unwrap();
    let recipient_id = receive.recipient_id.clone();
    let recipient_script =
        rgb_lib_wasm::utils::script_buf_from_recipient_id(receive.recipient_id.clone())
            .unwrap()
            .unwrap();
    let recipient = Recipient {
        recipient_id,
        witness_data: Some(WitnessData {
            amount_sat: 2000,
            blinding: Some(777),
        }),
        assignment: Assignment::Fungible(100),
        transport_endpoints: vec![transport.clone()],
    };
    let mut recipient_map = HashMap::new();
    recipient_map.insert(asset.asset_id, vec![recipient]);

    let unsigned = wallet_a
        .send_begin(online_a.clone(), recipient_map, false, 1, 1, None)
        .await
        .unwrap();
    let signed = wallet_a.sign_psbt(unsigned, None).unwrap();
    let psbt = Psbt::from_str(&signed).unwrap();
    let recipient_vout = psbt
        .unsigned_tx
        .output
        .iter()
        .position(|output| output.script_pubkey == recipient_script)
        .unwrap() as u32;
    let send = wallet_a.send_end(online_a, signed, false).await.unwrap();
    let (consignment_bytes, _) = get_consignment_from_proxy(&receive.recipient_id).await;
    wallet_a
        .post_consignment(
            PROXY_URL,
            send.txid.clone(),
            &consignment_bytes,
            send.txid.clone(),
            Some(recipient_vout),
        )
        .await
        .unwrap();

    let (_, assignments) = wallet_b
        .accept_transfer(
            online_b,
            send.txid,
            recipient_vout,
            RgbTransport::from_str(&transport).unwrap(),
            777,
        )
        .await
        .unwrap();
    assert_eq!(assignments, vec![Assignment::Fungible(100)]);
    let contract_ids = wallet_b.rgb_contract_ids().unwrap();

    drop(wallet_b);
    let restored = Wallet::restore(wd_b).await.unwrap();
    assert_eq!(restored.rgb_contract_ids().unwrap(), contract_ids);
}

/// Test: Lightning-style funding can post a consignment before broadcast, survive a browser
/// reload, and complete without posting the already-used recipient endpoint again.
#[wasm_bindgen_test]
async fn test_pending_funding_transfer_completes_after_reload() {
    let unique = js_sys::Date::now().to_string();
    let keys_a = generate_keys(BitcoinNetwork::Regtest);
    let keys_b = generate_keys(BitcoinNetwork::Regtest);
    let wd_a = test_wallet_data(
        &keys_a,
        vec![AssetSchema::Nia],
        &format!("/tmp/persist-pending-funding-a-{unique}"),
    );
    let wd_b = test_wallet_data(
        &keys_b,
        vec![AssetSchema::Nia],
        &format!("/tmp/persist-pending-funding-b-{unique}"),
    );
    let mut wallet_a = Wallet::new(wd_a.clone()).unwrap();
    let mut wallet_b = Wallet::new(wd_b).unwrap();
    let online_a = wallet_a
        .go_online(false, ESPLORA_URL.to_string())
        .await
        .unwrap();

    fund_and_sync(&mut wallet_a, &online_a, "1.0").await;
    create_utxos(&mut wallet_a, &online_a, 5).await;
    let asset = wallet_a
        .issue_asset_nia("LDKF".to_string(), "LDK Funding".to_string(), 0, vec![1000])
        .unwrap();

    let transport = transport_endpoint();
    let receive = wallet_b
        .witness_receive(
            None,
            Assignment::Fungible(100),
            None,
            vec![transport.clone()],
            1,
        )
        .unwrap();
    let recipient = Recipient {
        recipient_id: receive.recipient_id.clone(),
        witness_data: Some(WitnessData {
            amount_sat: 2000,
            blinding: Some(777),
        }),
        assignment: Assignment::Fungible(100),
        transport_endpoints: vec![transport],
    };
    let recipient_map = HashMap::from([(asset.asset_id, vec![recipient])]);

    let unsigned = wallet_a
        .send_begin(online_a.clone(), recipient_map, true, 1, 1, None)
        .await
        .unwrap();
    let signed = wallet_a.sign_psbt(unsigned, None).unwrap();
    let funding_txid = Psbt::from_str(&signed)
        .unwrap()
        .unsigned_tx
        .compute_txid()
        .to_string();
    // Persist the not-yet-posted state, then simulate a page exit after the proxy accepted the
    // consignment but before the used endpoint flag could be flushed.
    wallet_a.flush().await.unwrap();
    wallet_a
        .post_pending_consignments(funding_txid.clone())
        .await
        .unwrap();
    let (consignment, proxy_txid) = get_consignment_from_proxy(&receive.recipient_id).await;
    assert!(!consignment.is_empty());
    assert_eq!(proxy_txid, funding_txid);

    drop(wallet_a);
    let mut wallet_a = Wallet::restore(wd_a.clone()).await.unwrap();
    wallet_a
        .post_pending_consignments(funding_txid.clone())
        .await
        .unwrap();
    let mut wallet_a = simulate_reload(wallet_a, &wd_a).await;
    let restored_online = wallet_a
        .go_online(true, ESPLORA_URL.to_string())
        .await
        .unwrap();
    let completed = wallet_a
        .send_end(restored_online, signed, false)
        .await
        .unwrap();
    assert_eq!(completed.txid, funding_txid);
    wallet_a.flush().await.unwrap();
}
