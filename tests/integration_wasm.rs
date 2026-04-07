use std::collections::HashMap;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

mod utils;

use rgb_lib_wasm::wallet::{DatabaseType, Recipient, Wallet, WalletData, WitnessData};
use rgb_lib_wasm::{AssetSchema, Assignment, BitcoinNetwork, TransferStatus, generate_keys};
use utils::*;

fn test_wallet_data(schemas: Vec<AssetSchema>) -> WalletData {
    let keys = generate_keys(BitcoinNetwork::Regtest);
    WalletData {
        data_dir: "/tmp/rgb_integration".to_string(),
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

#[wasm_bindgen_test]
async fn test_full_wallet_flow() {
    let wd = test_wallet_data(vec![AssetSchema::Nia, AssetSchema::Ifa]);
    let mut wallet = Wallet::new(wd).unwrap();
    let online = wallet
        .go_online(false, ESPLORA_URL.to_string())
        .await
        .unwrap();

    fund_and_sync(&mut wallet, &online, "1.0").await;

    let balance = wallet.get_btc_balance(None, true).unwrap();
    let total = balance.vanilla.settled
        + balance.vanilla.future
        + balance.colored.settled
        + balance.colored.future;
    assert!(total > 0);

    let txs = wallet.list_transactions(None, true).unwrap();
    assert!(!txs.is_empty());
    assert!(txs[0].received > 0);

    create_utxos(&mut wallet, &online, 7).await;

    let nia = wallet
        .issue_asset_nia("TEST".to_string(), "Test NIA".to_string(), 0, vec![1000])
        .unwrap();
    assert!(!nia.asset_id.is_empty());
    assert_eq!(nia.ticker, "TEST");
    assert_eq!(nia.name, "Test NIA");
    assert_eq!(nia.balance.settled, 1000);

    let assets = wallet.list_assets(vec![AssetSchema::Nia]).unwrap();
    let nia_assets = assets.nia.unwrap();
    assert!(nia_assets.iter().any(|a| a.ticker == "TEST"));

    // get_asset_balance: verify matches list_assets
    let nia_balance = wallet.get_asset_balance(nia.asset_id.clone()).unwrap();
    assert_eq!(
        nia_balance.settled, 1000,
        "get_asset_balance settled should be 1000 after issuance"
    );

    // get_wallet_data: verify round-trip
    let wd_returned = wallet.get_wallet_data();
    assert_eq!(wd_returned.bitcoin_network, BitcoinNetwork::Regtest);
    assert_eq!(wd_returned.max_allocations_per_utxo, 5);

    // list_unspents (colored): verify UTXOs exist after issuance
    let colored_unspents = wallet.list_unspents(None, false, true).unwrap();
    assert!(
        !colored_unspents.is_empty(),
        "Should have colored unspents after issuance"
    );

    let ifa = wallet
        .issue_asset_ifa(
            "TIFA".to_string(),
            "Test IFA".to_string(),
            0,
            vec![1000],
            vec![5000],
            None,
        )
        .unwrap();
    assert!(!ifa.asset_id.is_empty());
    assert_eq!(ifa.ticker, "TIFA");
    assert_eq!(ifa.balance.settled, 1000);

    let transport = transport_endpoint();

    // Two-wallet NIA send: wallet A → wallet B
    // Must happen before blind_receive which locks the NIA UTXO via pending_blinded
    let wd_b = test_wallet_data(vec![AssetSchema::Nia]);
    let mut wallet_b = Wallet::new(wd_b).unwrap();
    let online_b = wallet_b
        .go_online(false, ESPLORA_URL.to_string())
        .await
        .unwrap();

    fund_and_sync(&mut wallet_b, &online_b, "1.0").await;
    create_utxos(&mut wallet_b, &online_b, 5).await;

    let recv_b = wallet_b
        .witness_receive(
            None,
            Assignment::Fungible(200),
            None,
            vec![transport.clone()],
            1,
        )
        .unwrap();

    // 2000 sats witness amount covers the dust output for the RGB transfer
    let recipient = Recipient {
        recipient_id: recv_b.recipient_id.clone(),
        witness_data: Some(WitnessData {
            amount_sat: 2000,
            blinding: None,
        }),
        assignment: Assignment::Fungible(200),
        transport_endpoints: vec![transport.clone()],
    };
    let mut recipient_map = HashMap::new();
    recipient_map.insert(nia.asset_id.clone(), vec![recipient]);

    let unsigned_psbt = wallet
        .send_begin(online.clone(), recipient_map, false, 1, 1)
        .await
        .unwrap();
    let signed_psbt = wallet.sign_psbt(unsigned_psbt, None).unwrap();
    let send_result = wallet
        .send_end(online.clone(), signed_psbt, false)
        .await
        .unwrap();
    assert!(!send_result.txid.is_empty());

    // validate_consignment_offchain: fetch from proxy and validate before broadcast
    let (consignment_bytes, proxy_txid) = get_consignment_from_proxy(&recv_b.recipient_id).await;
    assert!(!consignment_bytes.is_empty());
    assert_eq!(proxy_txid, send_result.txid);
    let validation = rgb_lib_wasm::wallet::rust_only::validate_consignment_offchain(
        &consignment_bytes,
        &send_result.txid,
        BitcoinNetwork::Regtest,
    )
    .unwrap();
    assert!(
        validation.valid,
        "Consignment should be valid, got error: {:?} details: {:?}",
        validation.error, validation.details,
    );

    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();
    wallet_b.sync(online_b.clone()).await.unwrap();

    // Receiver refreshes first: picks up consignment from proxy, validates, ACKs
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

    // Mine the ACK so sender can see it
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();

    // Sender refreshes: sees ACK, broadcasts TX → WaitingConfirmations
    // This is the exact flow that failed with "signed PSBT not found in transfer artifacts"
    let send_refresh = wallet
        .refresh(online.clone(), None, vec![], false)
        .await
        .unwrap();
    for (idx, refreshed) in &send_refresh {
        assert!(
            refreshed.failure.is_none(),
            "Sender refresh failed for transfer {idx}: {:?}",
            refreshed.failure,
        );
    }

    let a_assets = wallet.list_assets(vec![AssetSchema::Nia]).unwrap();
    let a_nia = a_assets
        .nia
        .unwrap()
        .into_iter()
        .find(|a| a.asset_id == nia.asset_id)
        .unwrap();
    // spendable excludes UTXOs locked by pending transfers
    assert!(
        a_nia.balance.spendable < 1000,
        "Sender spendable NIA should decrease, got: {:?}",
        a_nia.balance,
    );

    // list_transfers: verify NIA send created transfer records
    let a_transfers = wallet.list_transfers(Some(nia.asset_id.clone())).unwrap();
    assert!(
        !a_transfers.is_empty(),
        "Sender should have NIA transfer records"
    );
    // At least one transfer should be settled or waiting confirmations
    assert!(
        a_transfers
            .iter()
            .any(|t| t.status == TransferStatus::Settled
                || t.status == TransferStatus::WaitingConfirmations),
        "Should have a settled or confirming NIA transfer, got: {:?}",
        a_transfers.iter().map(|t| &t.status).collect::<Vec<_>>(),
    );

    // get_asset_balance after send: verify sender balance decreased
    let nia_bal_after_send = wallet.get_asset_balance(nia.asset_id.clone()).unwrap();
    assert!(
        nia_bal_after_send.spendable < 1000,
        "get_asset_balance spendable should decrease after send, got: {}",
        nia_bal_after_send.spendable,
    );

    let b_assets = wallet_b.list_assets(vec![AssetSchema::Nia]).unwrap();
    let b_nia = b_assets.nia.unwrap();
    assert!(!b_nia.is_empty(), "Receiver should have NIA asset");
    let b_balance = &b_nia[0].balance;
    assert!(
        b_balance.settled > 0 || b_balance.future > 0 || b_balance.spendable > 0,
        "Receiver should have NIA balance, got: {:?}",
        b_balance,
    );

    // BTC send: wallet A → wallet B
    let addr_b2 = wallet_b.get_address().unwrap();
    let btc_before = wallet.get_btc_balance(None, true).unwrap();
    let total_before = btc_before.vanilla.settled + btc_before.vanilla.future;

    // 50_000 sats
    let unsigned_btc = wallet
        .send_btc_begin(online.clone(), addr_b2, 50_000, 1, false)
        .await
        .unwrap();
    let signed_btc = wallet.sign_psbt(unsigned_btc, None).unwrap();
    let btc_txid = wallet
        .send_btc_end(online.clone(), signed_btc, false)
        .await
        .unwrap();
    assert!(!btc_txid.is_empty());

    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();

    let btc_after = wallet.get_btc_balance(None, true).unwrap();
    let total_after = btc_after.vanilla.settled + btc_after.vanilla.future;
    assert!(
        total_after < total_before,
        "Sender BTC should decrease after send"
    );

    // Fee estimation: regtest esplora returns empty fee map
    let fee_result = wallet.get_fee_estimation(online.clone(), 1).await;
    assert!(
        fee_result.is_ok()
            || format!("{:?}", fee_result.unwrap_err()).contains("CannotEstimateFees"),
        "Fee estimation should succeed or return CannotEstimateFees in regtest"
    );

    // IFA inflation: inflate the IFA asset by 500
    let ifa_balance_before = ifa.balance.settled;
    let unsigned_inflate = wallet
        .inflate_begin(online.clone(), ifa.asset_id.clone(), vec![500], 1, 1)
        .await
        .unwrap();
    let signed_inflate = wallet.sign_psbt(unsigned_inflate, None).unwrap();
    let inflate_result = wallet
        .inflate_end(online.clone(), signed_inflate)
        .await
        .unwrap();
    assert!(!inflate_result.txid.is_empty());

    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();

    let ifa_assets = wallet.list_assets(vec![AssetSchema::Ifa]).unwrap();
    let ifa_after = ifa_assets
        .ifa
        .unwrap()
        .into_iter()
        .find(|a| a.asset_id == ifa.asset_id)
        .unwrap();
    assert!(
        ifa_after.balance.settled > ifa_balance_before
            || ifa_after.balance.future > 0
            || ifa_after.balance.spendable > ifa_balance_before,
        "IFA balance should increase after inflation, got: {:?}",
        ifa_after.balance,
    );

    // List unspents vanilla
    let unspents = wallet
        .list_unspents_vanilla(online.clone(), 0, false)
        .await
        .unwrap();
    assert!(!unspents.is_empty(), "Should have vanilla unspents");

    // Drain: wallet B drains to wallet A
    let drain_addr = wallet.get_address().unwrap();
    let unsigned_drain = wallet_b
        .drain_to_begin(online_b.clone(), drain_addr, false, 1)
        .await
        .unwrap();
    let signed_drain = wallet_b.sign_psbt(unsigned_drain, None).unwrap();
    let drain_txid = wallet_b
        .drain_to_end(online_b.clone(), signed_drain)
        .await
        .unwrap();
    assert!(!drain_txid.is_empty());

    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();
    wallet_b.sync(online_b.clone()).await.unwrap();

    // Drain verification: wallet_b vanilla balance should be ~0
    // (colored UTXOs with RGB allocations are preserved when destroy_assets=false)
    let b_balance_after_drain = wallet_b.get_btc_balance(None, true).unwrap();
    let b_vanilla_after =
        b_balance_after_drain.vanilla.settled + b_balance_after_drain.vanilla.future;
    assert!(
        b_vanilla_after < 1000,
        "Wallet B vanilla BTC should be near-zero after drain, got: {}",
        b_vanilla_after,
    );

    // wallet_a should have received the drained funds
    let a_balance_after_drain = wallet.get_btc_balance(None, true).unwrap();
    let a_total_after_drain =
        a_balance_after_drain.vanilla.settled + a_balance_after_drain.vanilla.future;
    assert!(
        a_total_after_drain > 0,
        "Wallet A should have received drained BTC"
    );

    // check_proxy_url
    rgb_lib_wasm::wallet::rust_only::check_proxy_url(PROXY_URL)
        .await
        .unwrap();

    // get_tx_height: btc_txid was mined earlier
    let height = wallet
        .get_tx_height(online.clone(), btc_txid.clone())
        .await
        .unwrap();
    assert!(height.is_some(), "Mined TX should have a height");

    // Backup / restore round-trip
    // backup_info before backup
    let needs_backup = wallet.backup_info().unwrap();
    // Wallet has state, so backup should be required
    assert!(needs_backup, "backup_info should indicate backup needed");

    let password = "test_password_123";
    let backup_bytes = wallet.backup(password).unwrap();
    assert!(!backup_bytes.is_empty(), "Backup should produce bytes");

    // backup_info after backup: should no longer need backup
    let needs_backup_after = wallet.backup_info().unwrap();
    assert!(
        !needs_backup_after,
        "backup_info should indicate no backup needed after backup"
    );

    // Wrong password should fail
    let bad_restore = wallet.restore_backup(&backup_bytes, "wrong_password");
    assert!(bad_restore.is_err(), "Wrong password should fail");

    // Correct password should succeed
    wallet.restore_backup(&backup_bytes, password).unwrap();

    // Verify state after restore
    let assets_after = wallet.list_assets(vec![AssetSchema::Nia]).unwrap();
    let nia_after = assets_after.nia.unwrap();
    assert!(
        nia_after.iter().any(|a| a.ticker == "TEST"),
        "NIA asset should survive backup/restore"
    );

    let balance_after = wallet.get_btc_balance(None, true).unwrap();
    let total_after_restore = balance_after.vanilla.settled
        + balance_after.vanilla.future
        + balance_after.colored.settled
        + balance_after.colored.future;
    assert!(
        total_after_restore > 0,
        "BTC balance should survive backup/restore"
    );

    // VSS Cloud Backup: full round-trip against real server
    use bdk_wallet::bitcoin::secp256k1::{Secp256k1 as Secp, rand::rngs::OsRng as SecpRng};
    use rgb_lib_wasm::wallet::vss::VssBackupConfig;

    let secp = Secp::new();
    let (signing_key, _) = secp.generate_keypair(&mut SecpRng);
    let store_id = format!("test-wallet-{}", js_sys::Date::now() as u64);

    let vss_config = VssBackupConfig::new(
        utils::VSS_SERVER_URL.to_string(),
        store_id.clone(),
        signing_key,
    );
    wallet.configure_vss_backup(&vss_config);

    // Check info before backup
    let info_before = wallet.vss_backup_info().await.unwrap();
    assert!(!info_before.backup_exists, "No backup should exist yet");
    assert!(info_before.backup_required, "Backup should be required");

    // Upload backup
    let version = wallet.vss_backup().await.unwrap();
    assert!(version > 0, "Backup version should be positive");

    // Check info after backup
    let info_after = wallet.vss_backup_info().await.unwrap();
    assert!(info_after.backup_exists, "Backup should exist after upload");
    assert!(info_after.server_version.is_some());

    // Restore from VSS into the same wallet
    wallet.vss_restore_backup().await.unwrap();

    // Verify state after VSS restore
    let vss_assets = wallet.list_assets(vec![AssetSchema::Nia]).unwrap();
    let vss_nia = vss_assets.nia.unwrap();
    assert!(
        vss_nia.iter().any(|a| a.ticker == "TEST"),
        "NIA asset should survive VSS backup/restore"
    );

    let vss_balance = wallet.get_btc_balance(None, true).unwrap();
    let vss_total = vss_balance.vanilla.settled
        + vss_balance.vanilla.future
        + vss_balance.colored.settled
        + vss_balance.colored.future;
    assert!(
        vss_total > 0,
        "BTC balance should survive VSS backup/restore"
    );

    // Wrong signing key should fail to decrypt
    let (wrong_key, _) = secp.generate_keypair(&mut SecpRng);
    let wrong_config = VssBackupConfig::new(
        utils::VSS_SERVER_URL.to_string(),
        store_id.clone(),
        wrong_key,
    );
    wallet.configure_vss_backup(&wrong_config);
    let wrong_restore = wallet.vss_restore_backup().await;
    assert!(
        wrong_restore.is_err(),
        "Wrong signing key should fail decryption"
    );

    // Restore correct config for cleanup
    wallet.configure_vss_backup(&vss_config);

    // disable_vss_backup: verify it doesn't crash
    wallet.disable_vss_backup();

    // Receive API tests (after send, since blind_receive locks NIA UTXO)
    let blind_recv = wallet
        .blind_receive(
            Some(nia.asset_id.clone()),
            Assignment::Fungible(100),
            Some(3600),
            vec![transport.clone()],
            1,
        )
        .unwrap();
    assert!(!blind_recv.invoice.is_empty());
    assert!(!blind_recv.recipient_id.is_empty());
    assert!(blind_recv.expiration_timestamp.is_some());

    let witness_recv = wallet
        .witness_receive(None, Assignment::Any, Some(0), vec![transport.clone()], 1)
        .unwrap();
    assert!(!witness_recv.invoice.is_empty());
    assert!(witness_recv.expiration_timestamp.is_none());

    // fail_transfers: no pending transfers to fail — should succeed with false
    let failed = wallet
        .fail_transfers(online.clone(), None, false, true)
        .await
        .unwrap();
    assert!(!failed, "No pending transfers to fail should return false");

    // delete_transfers: no failed transfers to delete — should succeed with false
    let deleted = wallet.delete_transfers(None, false).unwrap();
    assert!(
        !deleted,
        "No failed transfers to delete should return false"
    );
}
