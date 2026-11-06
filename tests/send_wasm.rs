use std::{collections::HashMap, str::FromStr};

use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

mod utils;

use rgb_lib_wasm::bitcoin::psbt::Psbt;
use rgb_lib_wasm::wallet::{
    AssetFilter, DatabaseType, Online, Recipient, SendBeginResult, Transfer, Wallet, WalletData,
};
use rgb_lib_wasm::{
    AssetSchema, Assignment, BitcoinNetwork, Encryption, Error, TransferKind, TransferStatus,
    generate_keys,
};
use utils::*;

fn test_wallet_data(schemas: Vec<AssetSchema>) -> WalletData {
    let keys = generate_keys(BitcoinNetwork::Regtest);
    WalletData {
        data_dir: "/tmp/rgb_send".to_string(),
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

async fn new_wallet() -> (Wallet, Online) {
    let mut wallet = Wallet::new(test_wallet_data(vec![AssetSchema::Nia])).unwrap();
    let online = wallet.go_online(go_online_options(false)).await.unwrap();
    (wallet, online)
}

async fn sender() -> (Wallet, Online) {
    let (mut wallet, online) = new_wallet().await;
    fund_and_sync(&mut wallet, &online, "1.0").await;
    create_utxos(&mut wallet, &online, true, Some(10), None).await;
    (wallet, online)
}

fn issue(wallet: &Wallet, ticker: &str, amount: u64) -> String {
    wallet
        .issue_asset_nia(ticker.to_string(), "Test".to_string(), 0, vec![amount])
        .unwrap()
        .asset_id
}

async fn blind(wallet: &mut Wallet, online: &Online, amount: u64, url: &str) -> String {
    blind_rcv(
        wallet,
        online,
        None,
        Assignment::Fungible(amount),
        default_rcv_expiration(),
        vec![transport_endpoint(url)],
    )
    .await
    .unwrap()
    .recipient_id
}

async fn begin(
    wallet: &mut Wallet,
    online: &Online,
    map: HashMap<String, Vec<Recipient>>,
    donation: bool,
    dry_run: bool,
) -> (SendBeginResult, String, String) {
    begin_with(
        wallet,
        online,
        map,
        donation,
        dry_run,
        default_send_expiration(),
    )
    .await
}

async fn begin_with(
    wallet: &mut Wallet,
    online: &Online,
    map: HashMap<String, Vec<Recipient>>,
    donation: bool,
    dry_run: bool,
    expiration: u64,
) -> (SendBeginResult, String, String) {
    let res = wallet
        .send_begin(
            online.clone(),
            map,
            donation,
            1,
            1,
            expiration,
            Some(transport_endpoint(PROXY_URL)),
            None,
            dry_run,
        )
        .await
        .unwrap();
    let signed = wallet.sign_psbt(res.psbt.clone(), None).unwrap();
    let txid = Psbt::from_str(&signed)
        .unwrap()
        .unsigned_tx
        .compute_txid()
        .to_string();
    (res, signed, txid)
}

async fn begin_dry(
    wallet: &mut Wallet,
    online: &Online,
    map: HashMap<String, Vec<Recipient>>,
    expiration: u64,
) -> Result<SendBeginResult, Error> {
    wallet
        .send_begin(
            online.clone(),
            map,
            false,
            1,
            1,
            expiration,
            Some(transport_endpoint(PROXY_URL)),
            None,
            true,
        )
        .await
}

async fn end(wallet: &mut Wallet, online: &Online, signed: &str) -> Result<(String, i32), Error> {
    wallet
        .send_end(online.clone(), signed.to_string(), false)
        .await
        .map(|r| (r.txid, r.batch_transfer_idx))
}

fn sends(wallet: &Wallet, txid: &str) -> Vec<Transfer> {
    wallet
        .list_transfers(AssetFilter::Any, None)
        .unwrap()
        .into_iter()
        .filter(|t| t.kind == TransferKind::Send && t.txid.as_deref() == Some(txid))
        .collect()
}

// Make every consignment post to the proxy at `url_part` fail, or stop the failures.
fn fail_posts(url_part: &str, fail: bool) {
    clear_rpc_hooks();
    if fail {
        rpc_fail(url_part, "consignment.post");
    }
}

fn balance(wallet: &Wallet, asset_id: &str) -> u64 {
    wallet
        .get_asset_balance(asset_id.to_string())
        .unwrap()
        .settled
}

#[wasm_bindgen_test]
async fn begin_end() {
    let (mut wallet, online) = sender().await;
    let asset_id = issue(&wallet, "TST", 1000);
    let (mut rcv, rcv_online) = sender().await;
    let rid = blind(&mut rcv, &rcv_online, 66, PROXY_URL).await;
    let map = HashMap::from([(asset_id.clone(), vec![recipient(&rid, 66, None, PROXY_URL)])]);
    wallet.backup("pw").unwrap();
    assert!(!wallet.backup_info().unwrap());

    // a dry run saves nothing
    let (res, _, txid) = begin(&mut wallet, &online, map.clone(), false, true).await;
    assert_eq!(res.batch_transfer_idx, None);
    assert!(sends(&wallet, &txid).is_empty());
    assert!(!wallet.backup_info().unwrap());

    // the operation expires at least the margin before the earliest invoice
    let early_rid = blind_rcv(
        &mut rcv,
        &rcv_online,
        None,
        Assignment::Fungible(34),
        default_rcv_expiration() - 1000,
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap()
    .recipient_id;
    let early = recipient(&early_rid, 34, None, PROXY_URL);
    let latest = early.expiration_timestamp.unwrap() - 600;
    let two = HashMap::from([(
        asset_id.clone(),
        vec![recipient(&rid, 66, None, PROXY_URL), early],
    )]);
    assert!(
        begin_dry(&mut wallet, &online, two.clone(), latest)
            .await
            .is_ok()
    );
    assert!(matches!(
        begin_dry(&mut wallet, &online, two, latest + 1).await,
        Err(Error::InvalidExpiration)
    ));

    sleep_ms(1100).await;
    let (res, signed, txid) = begin(&mut wallet, &online, map.clone(), false, false).await;
    let idx = res.batch_transfer_idx.unwrap();
    // the Initiated send reserves the only asset allocation
    assert!(
        wallet
            .send_begin(
                online.clone(),
                map,
                false,
                1,
                1,
                default_send_expiration(),
                Some(transport_endpoint(PROXY_URL)),
                None,
                false,
            )
            .await
            .is_err()
    );
    let saved = sends(&wallet, &txid);
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].status, TransferStatus::Initiated);
    assert_eq!(saved[0].batch_transfer_idx, idx);
    assert!(wallet.backup_info().unwrap());
    assert_eq!(res.details.min_confirmations, 1);
    assert!(!res.details.is_donation);
    assert!(!res.details.fascia.is_empty());

    assert_eq!(
        end(&mut wallet, &online, &signed).await,
        Ok((txid.clone(), idx))
    );
    let saved = sends(&wallet, &txid);
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].status, TransferStatus::WaitingCounterparty);
}

#[wasm_bindgen_test]
async fn send_end_retry_after_post_failure() {
    let (mut wallet, online) = new_wallet().await;
    fund_and_sync(&mut wallet, &online, "1.0").await;
    create_utxos(&mut wallet, &online, false, Some(1), None).await;
    let asset_id = issue(&wallet, "TST", 1000);
    let (mut rcv, rcv_online) = sender().await;

    // a send whose second post fails can be failed
    let ra = blind(&mut rcv, &rcv_online, 66, PROXY_URL).await;
    let rb = blind(&mut rcv, &rcv_online, 34, PROXY_B_URL).await;
    let map = HashMap::from([(
        asset_id.clone(),
        vec![
            recipient(&ra, 66, None, PROXY_URL),
            recipient(&rb, 34, None, PROXY_B_URL),
        ],
    )]);
    let (res, signed, txid) = begin(&mut wallet, &online, map, false, false).await;
    let idx = res.batch_transfer_idx.unwrap();
    fail_posts(":3010", true);
    assert!(matches!(
        end(&mut wallet, &online, &signed).await,
        Err(Error::Proxy { .. })
    ));
    let saved = sends(&wallet, &txid);
    assert_eq!(saved.len(), 2);
    assert!(saved.iter().all(|t| t.status == TransferStatus::Initiated));
    assert!(
        wallet
            .fail_transfers(online.clone(), Some(idx), false, false)
            .await
            .unwrap()
    );
    assert!(
        sends(&wallet, &txid)
            .iter()
            .all(|t| t.status == TransferStatus::Failed)
    );
    fail_posts(":3010", false);
    assert_eq!(
        end(&mut wallet, &online, &signed).await,
        Err(Error::CannotEndTransfer { txid })
    );

    // a send whose second post fails can be retried
    let ra = blind(&mut rcv, &rcv_online, 66, PROXY_URL).await;
    let rb = blind(&mut rcv, &rcv_online, 34, PROXY_B_URL).await;
    let map = HashMap::from([(
        asset_id.clone(),
        vec![
            recipient(&ra, 66, None, PROXY_URL),
            recipient(&rb, 34, None, PROXY_B_URL),
        ],
    )]);
    let (res, signed, txid) = begin(&mut wallet, &online, map, false, false).await;
    let idx = res.batch_transfer_idx.unwrap();
    fail_posts(":3010", true);
    assert!(matches!(
        end(&mut wallet, &online, &signed).await,
        Err(Error::Proxy { .. })
    ));
    let saved = sends(&wallet, &txid);
    assert_eq!(saved.len(), 2);
    assert!(
        saved
            .iter()
            .all(|t| t.status == TransferStatus::Initiated && t.batch_transfer_idx == idx)
    );
    fail_posts(":3010", false);
    assert_eq!(
        end(&mut wallet, &online, &signed).await,
        Ok((txid.clone(), idx))
    );
    let saved = sends(&wallet, &txid);
    assert_eq!(saved.len(), 2);
    for t in &saved {
        assert_eq!(t.status, TransferStatus::WaitingCounterparty);
        assert_eq!(t.transport_endpoints.len(), 1);
        assert!(t.transport_endpoints[0].used);
    }

    refresh(&mut rcv, &rcv_online).await;
    refresh(&mut wallet, &online).await;
    settle(&mut rcv, &rcv_online, &[ra.clone(), rb.clone()]).await;
    settle(&mut wallet, &online, &[ra, rb]).await;
    assert_eq!(balance(&rcv, &asset_id), 100);
    assert_eq!(balance(&wallet, &asset_id), 900);
}

// A browser cannot crash between the posts, so a failed post stands in for the crash.
async fn send_to_oneself_crash_impl(dry_run: bool) {
    let (mut wallet, online) = sender().await;
    let asset_id = issue(&wallet, "TST", 1000);
    let r1 = blind(&mut wallet, &online, 66, PROXY_URL).await;
    let r2 = witness_rcv(
        &mut wallet,
        &online,
        None,
        Assignment::Fungible(33),
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_B_URL)],
    )
    .await
    .unwrap()
    .recipient_id;
    let map = HashMap::from([(
        asset_id.clone(),
        vec![
            recipient(&r1, 66, None, PROXY_URL),
            recipient(&r2, 33, Some(1000), PROXY_B_URL),
        ],
    )]);
    let (res, signed, txid) = begin(&mut wallet, &online, map, false, dry_run).await;
    fail_posts(":3010", true);
    assert!(matches!(
        end(&mut wallet, &online, &signed).await,
        Err(Error::Proxy { .. })
    ));
    let saved = sends(&wallet, &txid);
    if dry_run {
        assert!(saved.is_empty());
    } else {
        assert_eq!(saved.len(), 2);
        assert!(saved.iter().all(|t| t.status == TransferStatus::Initiated));
    }
    refresh(&mut wallet, &online).await;
    assert_eq!(transfer_of(&wallet, &r1).txid, Some(txid.clone()));

    fail_posts(":3010", false);
    let (end_txid, idx) = end(&mut wallet, &online, &signed).await.unwrap();
    assert_eq!(end_txid, txid);
    if let Some(begin_idx) = res.batch_transfer_idx {
        assert_eq!(idx, begin_idx);
    }
    let transfers = wallet.list_transfers(AssetFilter::Any, None).unwrap();
    assert_eq!(transfers.len(), 5);
    let count = |k: TransferKind| transfers.iter().filter(|t| t.kind == k).count();
    assert_eq!(count(TransferKind::Issuance), 1);
    assert_eq!(count(TransferKind::Send), 2);
    assert_eq!(count(TransferKind::ReceiveBlind), 1);
    assert_eq!(count(TransferKind::ReceiveWitness), 1);
    assert!(
        sends(&wallet, &txid)
            .iter()
            .all(|t| t.status == TransferStatus::WaitingCounterparty)
    );
    let r1_transfer = transfers
        .iter()
        .find(|t| t.kind == TransferKind::ReceiveBlind)
        .unwrap();
    assert_eq!(r1_transfer.status, TransferStatus::WaitingBroadcast);
}

#[wasm_bindgen_test]
async fn send_to_oneself_crash_dry_run() {
    send_to_oneself_crash_impl(true).await;
}

#[wasm_bindgen_test]
async fn send_to_oneself_crash_no_dry_run() {
    send_to_oneself_crash_impl(false).await;
}

async fn send_extra_allocations_no_dry_run_impl(donation: bool) {
    let (mut wallet, online) = new_wallet().await;
    fund_and_sync(&mut wallet, &online, "1.0").await;
    create_utxos(&mut wallet, &online, false, Some(1), None).await;
    let asset_a = issue(&wallet, "TSTA", 1000);
    let asset_b = issue(&wallet, "TSTB", 2000);
    let colorable = |w: &mut Wallet| {
        w.list_unspents(None, false, true)
            .unwrap()
            .into_iter()
            .filter(|u| u.utxo.colorable)
            .collect::<Vec<_>>()
    };
    let unspents = colorable(&mut wallet);
    assert_eq!(unspents.len(), 1);
    assert_eq!(unspents[0].rgb_allocations.len(), 2);
    create_utxos(&mut wallet, &online, false, Some(1), None).await;
    assert_eq!(colorable(&mut wallet).len(), 2);

    let (mut rcv, rcv_online) = sender().await;
    let rid = blind(&mut rcv, &rcv_online, 66, PROXY_URL).await;
    let map = HashMap::from([(asset_a.clone(), vec![recipient(&rid, 66, None, PROXY_URL)])]);
    let (res, signed, txid) = begin(&mut wallet, &online, map, donation, false).await;
    let idx = res.batch_transfer_idx.unwrap();
    let saved = sends(&wallet, &txid);
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].status, TransferStatus::Initiated);
    assert!(
        wallet
            .list_transfers(AssetFilter::Id(asset_b.clone()), None)
            .unwrap()
            .iter()
            .all(|t| t.kind != TransferKind::Send)
    );

    assert_eq!(
        end(&mut wallet, &online, &signed).await,
        Ok((txid.clone(), idx))
    );
    let saved = sends(&wallet, &txid);
    let expected = if donation {
        TransferStatus::WaitingConfirmations
    } else {
        TransferStatus::WaitingCounterparty
    };
    assert_eq!(saved[0].status, expected);
    assert!(saved[0].transport_endpoints[0].used);

    refresh(&mut rcv, &rcv_online).await;
    refresh(&mut wallet, &online).await;
    settle(&mut rcv, &rcv_online, &[rid.clone()]).await;
    settle(&mut wallet, &online, &[rid]).await;
    assert_eq!(balance(&wallet, &asset_a), 934);
    assert_eq!(balance(&rcv, &asset_a), 66);
    assert_eq!(balance(&wallet, &asset_b), 2000);
    assert!(colorable(&mut wallet).iter().any(|u| {
        u.rgb_allocations
            .iter()
            .any(|a| a.asset_id.as_deref() == Some(asset_b.as_str()) && a.settled)
    }));
}

#[wasm_bindgen_test]
async fn send_extra_allocations_no_dry_run() {
    send_extra_allocations_no_dry_run_impl(false).await;
}

#[wasm_bindgen_test]
async fn send_extra_allocations_no_dry_run_donation() {
    send_extra_allocations_no_dry_run_impl(true).await;
}

#[wasm_bindgen_test]
async fn blinded_change_send_begin_only() {
    let (mut w1, o1) = new_wallet().await;
    fund_and_sync(&mut w1, &o1, "1.0").await;
    create_utxos(&mut w1, &o1, true, Some(2), Some(487)).await;
    let asset_id = issue(&w1, "TST", 1000);
    let (mut w2, o2) = sender().await;

    // send 1: begin only, to a failed invoice, then fail the send
    let rd = blind_rcv(
        &mut w2,
        &o2,
        None,
        Assignment::Fungible(66),
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap();
    assert!(
        w2.fail_transfers(o2.clone(), Some(rd.batch_transfer_idx), false, false)
            .await
            .unwrap()
    );
    let map = HashMap::from([(
        asset_id.clone(),
        vec![recipient(&rd.recipient_id, 66, None, PROXY_URL)],
    )]);
    let (res, _, _) = begin(&mut w1, &o1, map, false, false).await;
    // the Initiated send reserves the only asset UTXO
    let rid = blind(&mut w2, &o2, 66, PROXY_URL).await;
    let map = HashMap::from([(asset_id.clone(), vec![recipient(&rid, 66, None, PROXY_URL)])]);
    assert!(
        w1.send_begin(
            o1.clone(),
            map,
            false,
            1,
            1,
            default_send_expiration(),
            Some(transport_endpoint(PROXY_URL)),
            None,
            false,
        )
        .await
        .is_err()
    );
    assert!(
        w1.fail_transfers(o1.clone(), res.batch_transfer_idx, false, false)
            .await
            .unwrap()
    );

    // sends 2 and 3 complete
    for round in 0..2 {
        if round == 1 {
            create_utxos(&mut w1, &o1, false, Some(1), Some(487)).await;
        }
        let rid = blind(&mut w2, &o2, 66, PROXY_URL).await;
        let map = HashMap::from([(asset_id.clone(), vec![recipient(&rid, 66, None, PROXY_URL)])]);
        let (_, signed, _) = begin(&mut w1, &o1, map, false, false).await;
        end(&mut w1, &o1, &signed).await.unwrap();
        refresh(&mut w2, &o2).await;
        refresh(&mut w1, &o1).await;
        settle(&mut w2, &o2, &[rid.clone()]).await;
        settle(&mut w1, &o1, &[rid]).await;
    }
    assert_eq!(balance(&w2, &asset_id), 132);
    assert_eq!(balance(&w1, &asset_id), 868);
}

#[wasm_bindgen_test]
async fn batch_donation_success() {
    let (mut wallet, online) = sender().await;
    let asset_a = issue(&wallet, "TSTA", 1000);
    let asset_b = issue(&wallet, "TSTB", 1000);
    let (mut rcv1, rcv1_online) = sender().await;
    let (mut rcv2, rcv2_online) = sender().await;
    let a1 = blind(&mut rcv1, &rcv1_online, 11, PROXY_URL).await;
    let b1 = blind(&mut rcv1, &rcv1_online, 25, PROXY_URL).await;
    let a2 = blind(&mut rcv2, &rcv2_online, 12, PROXY_URL).await;
    let b2 = blind(&mut rcv2, &rcv2_online, 22, PROXY_URL).await;
    let map = HashMap::from([
        (
            asset_a.clone(),
            vec![
                recipient(&a1, 11, None, PROXY_URL),
                recipient(&a2, 12, None, PROXY_URL),
            ],
        ),
        (
            asset_b.clone(),
            vec![
                recipient(&b1, 25, None, PROXY_URL),
                recipient(&b2, 22, None, PROXY_URL),
            ],
        ),
    ]);
    let (_, signed, txid) = begin(&mut wallet, &online, map, true, false).await;
    end(&mut wallet, &online, &signed).await.unwrap();
    // a donation cannot be ended twice
    assert_eq!(
        end(&mut wallet, &online, &signed).await,
        Err(Error::CannotEndTransfer { txid: txid.clone() })
    );
    assert!(
        sends(&wallet, &txid)
            .iter()
            .all(|t| t.status == TransferStatus::WaitingConfirmations)
    );

    // a donation skips WaitingBroadcast
    assert_eq!(
        transfer_of(&rcv1, &a1).status,
        TransferStatus::WaitingCounterparty
    );
    refresh(&mut rcv1, &rcv1_online).await;
    refresh(&mut rcv2, &rcv2_online).await;
    for (w, rid) in [(&rcv1, &a1), (&rcv1, &b1), (&rcv2, &a2), (&rcv2, &b2)] {
        assert_eq!(
            transfer_of(w, rid).status,
            TransferStatus::WaitingConfirmations
        );
    }

    settle(&mut rcv1, &rcv1_online, &[a1.clone(), b1.clone()]).await;
    settle(&mut rcv2, &rcv2_online, &[a2.clone(), b2.clone()]).await;
    settle(&mut wallet, &online, &[a1, a2, b1, b2]).await;
    assert_eq!(balance(&rcv1, &asset_a), 11);
    assert_eq!(balance(&rcv1, &asset_b), 25);
    assert_eq!(balance(&rcv2, &asset_a), 12);
    assert_eq!(balance(&rcv2, &asset_b), 22);
    assert_eq!(balance(&wallet, &asset_a), 977);
    assert_eq!(balance(&wallet, &asset_b), 953);
}

fn now_secs() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64
}

async fn sleep_until(timestamp: u64) {
    let now = js_sys::Date::now() as u64;
    if timestamp * 1000 > now {
        sleep_ms((timestamp * 1000 - now) as u32).await;
    }
}

// The transfer ID of the first transfer registered by the last `ack.prepare`.
fn prepared_transfer_id() -> String {
    let params = rpc_capture("ack.prepare");
    let data = js_sys::Reflect::get(&params, &"transfer_data".into()).unwrap();
    js_sys::Object::keys(&data.into())
        .get(0)
        .as_string()
        .unwrap()
}

// A funded sender, a recipient and the signed PSBT of a send of 66 from one to the other.
async fn pending_send(
    encryption: Option<Encryption>,
) -> (Wallet, Online, Wallet, Online, String, String, String, i32) {
    let (mut wallet, online) = sender().await;
    let asset_id = issue(&wallet, "TST", 1000);
    let (mut rcv, rcv_online) = sender().await;
    let rid = blind_rcv_enc(
        &mut rcv,
        &rcv_online,
        None,
        Assignment::Fungible(66),
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
        encryption,
    )
    .await
    .unwrap()
    .recipient_id;
    let map = HashMap::from([(asset_id.clone(), vec![recipient(&rid, 66, None, PROXY_URL)])]);
    let (res, signed, _) = begin(&mut wallet, &online, map, false, false).await;
    let idx = res.batch_transfer_idx.unwrap();
    (wallet, online, rcv, rcv_online, asset_id, rid, signed, idx)
}

async fn settled_send(encryption: Encryption) {
    let (mut wallet, online, mut rcv, rcv_online, asset_id, rid, signed, _) =
        pending_send(Some(encryption)).await;
    let plaintext = wallet
        .send_consignment_bytes(&txid_of(&signed), &asset_id)
        .unwrap();
    end(&mut wallet, &online, &signed).await.unwrap();
    rpc_hook(
        PROXY_URL,
        "consignment.get",
        "globalThis.__rgbConsignment = res.result.consignment; return res;",
    );
    refresh(&mut rcv, &rcv_online).await;
    clear_rpc_hooks();
    let posted = js_sys::Reflect::get(&js_sys::global(), &"__rgbConsignment".into())
        .unwrap()
        .as_string()
        .unwrap();
    let posted =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, posted).unwrap();
    // only Unsupported sends the consignment in plaintext
    assert_eq!(posted == plaintext, encryption == Encryption::Unsupported);
    // the TX released by the sender proxy is broadcast by the recipient
    assert_eq!(
        transfer_of(&rcv, &rid).status,
        TransferStatus::WaitingConfirmations
    );
    refresh(&mut wallet, &online).await;
    settle(&mut rcv, &rcv_online, std::slice::from_ref(&rid)).await;
    settle(&mut wallet, &online, std::slice::from_ref(&rid)).await;
    assert_eq!(balance(&rcv, &asset_id), 66);
    assert_eq!(balance(&wallet, &asset_id), 934);
}

#[wasm_bindgen_test]
async fn encryption_variants() {
    for encryption in [
        Encryption::Required,
        Encryption::Unsupported,
        Encryption::Optional,
    ] {
        settled_send(encryption).await;
    }
}

#[wasm_bindgen_test]
async fn forged_ack_signature_rejected() {
    spy_rpc();
    clear_rpc_hooks();
    let (mut wallet, online, _rcv, _rcv_online, _, _, signed, idx) = pending_send(None).await;
    let txid = end(&mut wallet, &online, &signed).await.unwrap().0;

    // a third party cannot forge an ACK: the proxy verifies its signature against the recipient
    // pubkey registered by the sender
    let forged = rpc_post(
        PROXY_URL,
        "ack.post",
        serde_json::json!({
            "transfer_id": prepared_transfer_id(),
            "ack": true,
            "signature": "00".repeat(64),
        }),
    )
    .await;
    assert_eq!(forged["error"]["code"], -503);

    // the forged ACK was not stored, so the sender is not tricked into broadcasting
    refresh(&mut wallet, &online).await;
    assert_eq!(
        sends(&wallet, &txid)[0].status,
        TransferStatus::WaitingCounterparty
    );
    assert_eq!(sends(&wallet, &txid)[0].batch_transfer_idx, idx);
}

// A consignment altered on the way fails the decryption, or the plaintext hash check when it is
// not encrypted.
#[wasm_bindgen_test]
async fn recipient_refuses_ciphertext_altered_in_transit() {
    for encryption in [Encryption::Unsupported, Encryption::Required] {
        let (mut wallet, online, mut rcv, rcv_online, _, rid, signed, _) =
            pending_send(Some(encryption)).await;
        let txid = end(&mut wallet, &online, &signed).await.unwrap().0;
        rpc_hook(
            PROXY_URL,
            "consignment.get",
            "const c = res.result.consignment; \
             res.result.consignment = c.slice(0, -8) + (c.endsWith('AAAAAAAA') ? 'BBBBBBBB' : 'AAAAAAAA'); \
             return res;",
        );
        refresh(&mut rcv, &rcv_online).await;
        clear_rpc_hooks();
        assert_eq!(transfer_of(&rcv, &rid).status, TransferStatus::Failed);
        // the sender sees the NACK
        refresh(&mut wallet, &online).await;
        assert_eq!(sends(&wallet, &txid)[0].status, TransferStatus::Failed);
    }
}

// The sender proxy already holds a NACK for the recipient.
#[wasm_bindgen_test]
async fn nack_pre_existing_receiver_fails() {
    let (mut wallet, online, mut rcv, rcv_online, _, rid, signed, _) = pending_send(None).await;
    end(&mut wallet, &online, &signed).await.unwrap();
    rpc_hook(
        PROXY_URL,
        "ack.post",
        "res.error = { code: -100, message: 'cannot change' }; delete res.result; return res;",
    );
    refresh(&mut rcv, &rcv_online).await;
    clear_rpc_hooks();
    assert_eq!(transfer_of(&rcv, &rid).status, TransferStatus::Failed);
}

#[wasm_bindgen_test]
async fn recipient_operation_expired() {
    let (mut wallet, online) = sender().await;
    let asset_id = issue(&wallet, "TST", 1000);
    let (mut rcv1, o1) = sender().await;
    let (mut rcv2, o2) = sender().await;
    let rid1 = blind(&mut rcv1, &o1, 66, PROXY_URL).await;
    let rid2 = blind(&mut rcv2, &o2, 34, PROXY_URL).await;
    let map = HashMap::from([(
        asset_id,
        vec![
            recipient(&rid1, 66, None, PROXY_URL),
            recipient(&rid2, 34, None, PROXY_URL),
        ],
    )]);
    let expiration = now_secs() + 20;
    let (_, signed, _) = begin_with(&mut wallet, &online, map, false, false, expiration).await;
    end(&mut wallet, &online, &signed).await.unwrap();

    // recipient 1 ACKs, recipient 2 never does, so the TX is not released
    refresh(&mut rcv1, &o1).await;
    assert_eq!(
        transfer_of(&rcv1, &rid1).status,
        TransferStatus::WaitingBroadcast
    );
    sleep_until(expiration + 2).await;
    refresh(&mut rcv1, &o1).await;
    assert_eq!(transfer_of(&rcv1, &rid1).status, TransferStatus::Failed);
}

#[wasm_bindgen_test]
async fn send_end_after_expiration() {
    spy_rpc();
    clear_rpc_hooks();
    let (mut wallet, online2) = sender().await;
    let asset_id = issue(&wallet, "TST", 1000);
    let (mut rcv, rcv_online) = sender().await;
    let rid = blind(&mut rcv, &rcv_online, 66, PROXY_URL).await;
    let map = HashMap::from([(asset_id, vec![recipient(&rid, 66, None, PROXY_URL)])]);

    // an operation that expired before its registration cannot start
    let (mut other, other_online) = sender().await;
    let other_asset = issue(&other, "TST", 1000);
    let other_rid = blind(&mut rcv, &rcv_online, 66, PROXY_URL).await;
    let other_map = HashMap::from([(
        other_asset,
        vec![recipient(&other_rid, 66, None, PROXY_URL)],
    )]);
    let other_expiration = now_secs() + 10;
    let (_, other_signed, _) = begin_with(
        &mut other,
        &other_online,
        other_map,
        false,
        false,
        other_expiration,
    )
    .await;
    sleep_until(other_expiration + 1).await;
    assert!(matches!(
        end(&mut other, &other_online, &other_signed).await,
        Err(Error::ExpirationInvalid)
    ));

    let expiration = now_secs() + 10;
    let (_, signed, txid) = begin_with(&mut wallet, &online2, map, false, false, expiration).await;
    // the operation is registered on the sender proxy, then the post fails
    rpc_fail(PROXY_URL, "consignment.post");
    assert!(matches!(
        end(&mut wallet, &online2, &signed).await,
        Err(Error::Proxy { .. })
    ));
    clear_rpc_hooks();

    // once expired, a retry only completes the local state, without posting again
    sleep_until(expiration + 1).await;
    assert!(end(&mut wallet, &online2, &signed).await.is_ok());
    assert_eq!(rpc_count("consignment.post"), 0);
    assert_eq!(
        sends(&wallet, &txid)[0].status,
        TransferStatus::WaitingCounterparty
    );
}

#[wasm_bindgen_test]
async fn fail_before_ack_revokes_operation() {
    let (mut wallet, online, mut rcv, rcv_online, asset_id, rid, signed, idx) =
        pending_send(None).await;
    let txid = end(&mut wallet, &online, &signed).await.unwrap().0;
    assert!(
        wallet
            .fail_transfers(online.clone(), Some(idx), false, false)
            .await
            .unwrap()
    );
    assert_eq!(sends(&wallet, &txid)[0].status, TransferStatus::Failed);
    // the operation is revoked, so the recipient's ACK is refused
    refresh(&mut rcv, &rcv_online).await;
    assert_ne!(
        transfer_of(&rcv, &rid).status,
        TransferStatus::WaitingConfirmations
    );
    // the single-use invoice has had its transfer
    let map = HashMap::from([(asset_id, vec![recipient(&rid, 66, None, PROXY_URL)])]);
    assert!(matches!(
        begin_dry(&mut wallet, &online, map, default_send_expiration()).await,
        Err(Error::InvoiceQuotaExceeded)
    ));
}

#[wasm_bindgen_test]
async fn fail_after_recipient_ack_refused() {
    let (mut wallet, online, mut rcv, rcv_online, _, _, signed, idx) = pending_send(None).await;
    let txid = end(&mut wallet, &online, &signed).await.unwrap().0;
    refresh(&mut rcv, &rcv_online).await;
    assert_eq!(
        wallet
            .fail_transfers(online.clone(), Some(idx), false, false)
            .await,
        Err(Error::CannotFailBatchTransfer)
    );
    assert_ne!(sends(&wallet, &txid)[0].status, TransferStatus::Failed);

    // the sender expires after the recipient ACK: the released TX still settles
    let (mut wallet, online) = sender().await;
    let asset_id = issue(&wallet, "TST", 1000);
    let rid = blind(&mut rcv, &rcv_online, 66, PROXY_URL).await;
    let map = HashMap::from([(asset_id.clone(), vec![recipient(&rid, 66, None, PROXY_URL)])]);
    let expiration = now_secs() + 20;
    let (res, signed, _) = begin_with(&mut wallet, &online, map, false, false, expiration).await;
    let idx = res.batch_transfer_idx.unwrap();
    end(&mut wallet, &online, &signed).await.unwrap();
    refresh(&mut rcv, &rcv_online).await;
    assert_eq!(
        transfer_of(&rcv, &rid).status,
        TransferStatus::WaitingConfirmations
    );
    sleep_until(expiration + 2).await;
    assert_eq!(
        wallet
            .fail_transfers(online.clone(), Some(idx), false, false)
            .await,
        Err(Error::CannotFailBatchTransfer)
    );
    settle(&mut rcv, &rcv_online, std::slice::from_ref(&rid)).await;
    settle(&mut wallet, &online, std::slice::from_ref(&rid)).await;
    assert_eq!(balance(&wallet, &asset_id), 934);
}

// The sender proxy lost the operation after the recipient broadcast its TX.
#[wasm_bindgen_test]
async fn sender_proxy_lost_operation() {
    let (mut wallet, online, mut rcv, rcv_online, asset_id, rid, signed, _) =
        pending_send(None).await;
    end(&mut wallet, &online, &signed).await.unwrap();
    refresh(&mut rcv, &rcv_online).await;
    assert_eq!(
        transfer_of(&rcv, &rid).status,
        TransferStatus::WaitingConfirmations
    );
    rpc_hook(
        PROXY_URL,
        "ack.get",
        "res.error = { code: -104, message: 'not found' }; delete res.result; return res;",
    );
    settle(&mut rcv, &rcv_online, std::slice::from_ref(&rid)).await;
    settle(&mut wallet, &online, std::slice::from_ref(&rid)).await;
    clear_rpc_hooks();
    assert_eq!(balance(&wallet, &asset_id), 934);
}

#[wasm_bindgen_test]
async fn no_change_on_pending_send() {
    let (mut wallet, online, _rcv, _rcv_online, _, _, signed, idx) = pending_send(None).await;
    let txid = end(&mut wallet, &online, &signed).await.unwrap().0;
    // the counterparty has not answered: refresh and fail change nothing
    refresh(&mut wallet, &online).await;
    assert_eq!(
        sends(&wallet, &txid)[0].status,
        TransferStatus::WaitingCounterparty
    );
    assert_eq!(
        wallet
            .fail_transfers(online.clone(), None, false, false)
            .await,
        Ok(false)
    );
    assert_eq!(sends(&wallet, &txid)[0].batch_transfer_idx, idx);
    assert_eq!(
        sends(&wallet, &txid)[0].status,
        TransferStatus::WaitingCounterparty
    );
}

#[wasm_bindgen_test]
async fn invoice_not_reserved_fail() {
    let (mut wallet, online) = sender().await;
    let asset_id = issue(&wallet, "TST", 1000);
    let (mut rcv, rcv_online) = sender().await;
    let rid = blind(&mut rcv, &rcv_online, 66, PROXY_URL).await;
    let exp = default_send_expiration();
    let with = |edit: fn(&mut Recipient)| {
        let mut rcpt = recipient(&rid, 66, None, PROXY_URL);
        edit(&mut rcpt);
        HashMap::from([(asset_id.clone(), vec![rcpt])])
    };

    // invalid recipients
    for map in [HashMap::new(), HashMap::from([(asset_id.clone(), vec![])])] {
        assert!(matches!(
            begin_dry(&mut wallet, &online, map, exp).await,
            Err(Error::InvalidRecipientMap)
        ));
    }
    let map = with(|r| r.public_key = Some("not hex".to_string()));
    assert!(matches!(
        begin_dry(&mut wallet, &online, map, exp).await,
        Err(Error::InvalidPubkey { .. })
    ));
    let map = with(|r| r.expiration_timestamp = None);
    assert!(matches!(
        begin_dry(&mut wallet, &online, map, exp).await,
        Err(Error::InvalidRecipientData { details }) if details == "missing invoice expiration"
    ));
    let map = with(|r| r.expiration_timestamp = Some(1));
    assert!(matches!(
        begin_dry(&mut wallet, &online, map, exp).await,
        Err(Error::InvoiceExpired)
    ));
    let map = with(|r| r.expiration_timestamp = Some(default_send_expiration() + 100));
    assert!(matches!(
        begin_dry(&mut wallet, &online, map, exp).await,
        Err(Error::InvalidExpiration)
    ));

    // an invoice with another pubkey was never reserved on the proxy
    let map = with(|r| {
        r.public_key =
            Some("0738ae88b5bfc7f915b809937573aa51d3bf00cce2edb5795b99f682d4065f07".to_string())
    });
    assert!(matches!(
        begin_dry(&mut wallet, &online, map, exp).await,
        Err(Error::InvoiceNotReserved)
    ));

    // the invoice is full
    rpc_hook(
        PROXY_URL,
        "invoice.check",
        "for (const k in res.result) res.result[k].status = 'full'; return res;",
    );
    let res = begin_dry(&mut wallet, &online, with(|_| {}), exp).await;
    clear_rpc_hooks();
    assert!(matches!(res, Err(Error::InvoiceQuotaExceeded)));

    // an unavailable proxy is skipped for the next one
    let map = with(|r| {
        r.transport_endpoints = vec![
            "rpc://127.0.0.1:1/json-rpc".to_string(),
            transport_endpoint(PROXY_URL),
        ]
    });
    assert!(begin_dry(&mut wallet, &online, map, exp).await.is_ok());
}

#[wasm_bindgen_test]
async fn proxy_auth() {
    let mut wallet = Wallet::new(test_wallet_data(vec![AssetSchema::Nia])).unwrap();
    let endpoint = transport_endpoint(PROXY_AUTH_URL);
    let mut options = go_online_options(false);
    let online = wallet.go_online(options.clone()).await.unwrap();

    // no key
    fund_and_sync(&mut wallet, &online, "1.0").await;
    create_utxos(&mut wallet, &online, true, Some(2), None).await;
    let res = wallet
        .blind_receive(
            Some(online.clone()),
            None,
            Assignment::Any,
            default_rcv_expiration(),
            vec![endpoint.clone()],
            1,
            None,
        )
        .await;
    assert!(res.is_err());

    // wrong key
    options
        .proxy_auth_keys
        .insert(endpoint.clone(), "11".repeat(32));
    let online = wallet.go_online(options.clone()).await.unwrap();
    let res = wallet
        .blind_receive(
            Some(online.clone()),
            None,
            Assignment::Any,
            default_rcv_expiration(),
            vec![endpoint.clone()],
            1,
            None,
        )
        .await;
    assert!(matches!(res, Err(Error::InvalidProxyAuthSignature)));

    // right key
    options
        .proxy_auth_keys
        .insert(endpoint.clone(), PROXY_AUTH_SEED.to_string());
    let online = wallet.go_online(options).await.unwrap();
    wallet
        .blind_receive(
            Some(online),
            None,
            Assignment::Any,
            default_rcv_expiration(),
            vec![endpoint.clone()],
            1,
            None,
        )
        .await
        .unwrap();

    // the sender signs ack.prepare on its own proxy
    let (mut payer, payer_online) = sender().await;
    let asset_id = issue(&payer, "TST", 1000);
    let (mut rcv, rcv_online) = sender().await;
    let rid = blind(&mut rcv, &rcv_online, 66, PROXY_URL).await;
    let map = HashMap::from([(asset_id, vec![recipient(&rid, 66, None, PROXY_URL)])]);
    let res = payer
        .send_begin(
            payer_online.clone(),
            map,
            false,
            1,
            1,
            default_send_expiration(),
            Some(endpoint.clone()),
            None,
            false,
        )
        .await
        .unwrap();
    let signed = payer.sign_psbt(res.psbt, None).unwrap();
    // no key
    assert!(matches!(
        end(&mut payer, &payer_online, &signed).await,
        Err(Error::Proxy { .. })
    ));
    // wrong key
    let mut options = go_online_options(false);
    options
        .proxy_auth_keys
        .insert(endpoint.clone(), "11".repeat(32));
    let payer_online = payer.go_online(options.clone()).await.unwrap();
    assert!(matches!(
        end(&mut payer, &payer_online, &signed).await,
        Err(Error::InvalidProxyAuthSignature)
    ));
    // right key
    options
        .proxy_auth_keys
        .insert(endpoint, PROXY_AUTH_SEED.to_string());
    let payer_online = payer.go_online(options).await.unwrap();
    let txid = end(&mut payer, &payer_online, &signed).await.unwrap().0;
    assert_eq!(
        sends(&payer, &txid)[0].status,
        TransferStatus::WaitingCounterparty
    );
}

#[wasm_bindgen_test]
async fn recipient_nacks_short_retention_grace() {
    let (mut wallet, online, mut rcv, rcv_online, _, rid, signed, _) = pending_send(None).await;
    let txid = end(&mut wallet, &online, &signed).await.unwrap().0;
    rpc_hook(
        PROXY_URL,
        "server.info",
        "res.result.retention_grace_secs = '10'; return res;",
    );
    refresh(&mut rcv, &rcv_online).await;
    clear_rpc_hooks();
    assert_eq!(transfer_of(&rcv, &rid).status, TransferStatus::Failed);
    refresh(&mut wallet, &online).await;
    assert_eq!(sends(&wallet, &txid)[0].status, TransferStatus::Failed);
}

#[wasm_bindgen_test]
async fn proxy_draining() {
    let (mut wallet, online) = sender().await;
    rpc_hook(
        PROXY_URL,
        "invoice.prepare",
        "res.error = { code: -115, message: 'draining' }; delete res.result; return res;",
    );
    let res = blind_rcv(
        &mut wallet,
        &online,
        None,
        Assignment::Any,
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await;
    clear_rpc_hooks();
    assert!(matches!(res, Err(Error::ProxyDraining)));

    let draining =
        "res.error = { code: -115, message: 'draining' }; delete res.result; return res;";
    let asset_id = issue(&wallet, "TST", 1000);
    let (mut rcv, rcv_online) = sender().await;

    // the only recipient proxy drains
    let rid = blind(&mut rcv, &rcv_online, 10, PROXY_URL).await;
    let map = HashMap::from([(asset_id.clone(), vec![recipient(&rid, 10, None, PROXY_URL)])]);
    let (_, signed, _) = begin(&mut wallet, &online, map, false, false).await;
    rpc_hook(PROXY_URL, "consignment.post", draining);
    let res = end(&mut wallet, &online, &signed).await;
    clear_rpc_hooks();
    assert!(matches!(res, Err(Error::ProxyDraining)));

    // the next recipient proxy takes the consignment
    let (mut wallet, online) = sender().await;
    let asset_id = issue(&wallet, "TST", 1000);
    let rid = blind_rcv(
        &mut rcv,
        &rcv_online,
        None,
        Assignment::Fungible(10),
        default_rcv_expiration(),
        vec![
            transport_endpoint(PROXY_URL),
            transport_endpoint(PROXY_B_URL),
        ],
    )
    .await
    .unwrap()
    .recipient_id;
    let mut rcpt = recipient(&rid, 10, None, PROXY_URL);
    rcpt.transport_endpoints = vec![
        transport_endpoint(PROXY_URL),
        transport_endpoint(PROXY_B_URL),
    ];
    let map = HashMap::from([(asset_id.clone(), vec![rcpt])]);
    let (_, signed, txid) = begin(&mut wallet, &online, map, false, false).await;
    rpc_hook(PROXY_URL, "consignment.post", draining);
    let res = end(&mut wallet, &online, &signed).await;
    clear_rpc_hooks();
    assert!(res.is_ok());
    let used: Vec<_> = sends(&wallet, &txid)[0]
        .transport_endpoints
        .iter()
        .filter(|e| e.used)
        .map(|e| e.endpoint.clone())
        .collect();
    assert_eq!(used.len(), 1);
    assert!(used[0].contains("3010"), "{used:?}");

    // the sender proxy drains
    let (mut wallet, online) = sender().await;
    let asset_id = issue(&wallet, "TST", 1000);
    let rid = blind(&mut rcv, &rcv_online, 10, PROXY_URL).await;
    let map = HashMap::from([(asset_id, vec![recipient(&rid, 10, None, PROXY_URL)])]);
    let (_, signed, _) = begin(&mut wallet, &online, map, false, false).await;
    rpc_hook(PROXY_URL, "ack.prepare", draining);
    let res = end(&mut wallet, &online, &signed).await;
    clear_rpc_hooks();
    assert!(matches!(res, Err(Error::ProxyDraining)));
}

#[wasm_bindgen_test]
#[should_panic(
    expected = "TX 0000000000000000000000000000000000000000000000000000000000000000 not confirmed"
)]
async fn mine_until_confirmed_panics_with_txid() {
    mine_until_confirmed("0000000000000000000000000000000000000000000000000000000000000000").await;
}
