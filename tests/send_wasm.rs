use std::{collections::HashMap, str::FromStr};

use wasm_bindgen::JsValue;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

mod utils;

use rgb_lib_wasm::bitcoin::psbt::Psbt;
use rgb_lib_wasm::wallet::{
    AssetFilter, DatabaseType, Online, Recipient, SendBeginResult, Transfer, Wallet, WalletData,
};
use rgb_lib_wasm::{
    AssetSchema, Assignment, BitcoinNetwork, Error, FileContent, RgbTransfer, RgbTxid,
    TransferKind, TransferStatus, generate_keys,
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

async fn create_utxos(
    wallet: &mut Wallet,
    online: &Online,
    up_to: bool,
    num: Option<u8>,
    size: Option<u32>,
) {
    let unsigned = wallet
        .create_utxos_begin(online.clone(), up_to, num, size, 1, true)
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

fn blind(wallet: &Wallet, amount: u64, url: &str) -> String {
    wallet
        .blind_receive(
            None,
            Assignment::Fungible(amount),
            default_rcv_expiration(),
            vec![transport_endpoint(url)],
            1,
        )
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
    let res = wallet
        .send_begin(
            online.clone(),
            map,
            donation,
            1,
            1,
            default_send_expiration(),
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

// Make every consignment post for `recipient_id` fail, or stop the failures.
fn fail_posts(recipient_id: &str, fail: bool) {
    js_sys::Function::new_no_args(
        "if (!globalThis.__rgbFetch) {
            globalThis.__rgbFetch = globalThis.fetch;
            globalThis.__rgbFailIds = new Set();
            globalThis.fetch = function (input, init) {
                const self = this;
                const orig = () => globalThis.__rgbFetch.call(self, input, init);
                const req = input instanceof Request ? input.clone() : null;
                const body = init && init.body instanceof FormData ? init.body : null;
                if (globalThis.__rgbFailIds.size === 0 || (!body && (!req || req.method !== 'POST'))) {
                    return orig();
                }
                const form = body ? Promise.resolve(body) : req.formData();
                return form.then((fd) => {
                    const params = String(fd.get('params') || '');
                    for (const id of globalThis.__rgbFailIds) {
                        if (params.includes(id)) throw new TypeError('injected failure');
                    }
                    return orig();
                }, orig);
            };
        }",
    )
    .call0(&JsValue::NULL)
    .unwrap();
    let ids = js_sys::Reflect::get(&js_sys::global(), &"__rgbFailIds".into()).unwrap();
    let method = if fail { "add" } else { "delete" };
    let f: js_sys::Function = js_sys::Reflect::get(&ids, &method.into()).unwrap().into();
    f.call1(&ids, &recipient_id.into()).unwrap();
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
    let (rcv, _) = sender().await;
    let rid = blind(&rcv, 66, PROXY_URL);
    let map = HashMap::from([(asset_id.clone(), vec![recipient(&rid, 66, None, PROXY_URL)])]);
    wallet.backup("pw").unwrap();
    assert!(!wallet.backup_info().unwrap());

    // a dry run saves nothing
    let (res, _, txid) = begin(&mut wallet, &online, map.clone(), false, true).await;
    assert_eq!(res.batch_transfer_idx, None);
    assert!(sends(&wallet, &txid).is_empty());
    assert!(!wallet.backup_info().unwrap());

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
    let ra = blind(&rcv, 66, PROXY_URL);
    let rb = blind(&rcv, 34, PROXY_B_URL);
    let map = HashMap::from([(
        asset_id.clone(),
        vec![
            recipient(&ra, 66, None, PROXY_URL),
            recipient(&rb, 34, None, PROXY_B_URL),
        ],
    )]);
    let (res, signed, txid) = begin(&mut wallet, &online, map, false, false).await;
    let idx = res.batch_transfer_idx.unwrap();
    fail_posts(&rb, true);
    assert_eq!(
        end(&mut wallet, &online, &signed).await,
        Err(Error::NoValidTransportEndpoint)
    );
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
    fail_posts(&rb, false);
    assert_eq!(
        end(&mut wallet, &online, &signed).await,
        Err(Error::CannotEndTransfer { txid })
    );

    // a send whose second post fails can be retried
    let ra = blind(&rcv, 66, PROXY_URL);
    let rb = blind(&rcv, 34, PROXY_B_URL);
    let map = HashMap::from([(
        asset_id.clone(),
        vec![
            recipient(&ra, 66, None, PROXY_URL),
            recipient(&rb, 34, None, PROXY_B_URL),
        ],
    )]);
    let (res, signed, txid) = begin(&mut wallet, &online, map, false, false).await;
    let idx = res.batch_transfer_idx.unwrap();
    fail_posts(&rb, true);
    assert_eq!(
        end(&mut wallet, &online, &signed).await,
        Err(Error::NoValidTransportEndpoint)
    );
    let saved = sends(&wallet, &txid);
    assert_eq!(saved.len(), 2);
    assert!(
        saved
            .iter()
            .all(|t| t.status == TransferStatus::Initiated && t.batch_transfer_idx == idx)
    );
    fail_posts(&rb, false);
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
    let r1 = blind(&wallet, 66, PROXY_URL);
    let r2 = wallet
        .witness_receive(
            None,
            Assignment::Fungible(33),
            default_rcv_expiration(),
            vec![transport_endpoint(PROXY_B_URL)],
            1,
        )
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
    fail_posts(&r2, true);
    assert_eq!(
        end(&mut wallet, &online, &signed).await,
        Err(Error::NoValidTransportEndpoint)
    );
    let saved = sends(&wallet, &txid);
    if dry_run {
        assert!(saved.is_empty());
    } else {
        assert_eq!(saved.len(), 2);
        assert!(saved.iter().all(|t| t.status == TransferStatus::Initiated));
    }
    refresh(&mut wallet, &online).await;
    assert_eq!(transfer_of(&wallet, &r1).txid, Some(txid.clone()));

    fail_posts(&r2, false);
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
    let rid = blind(&rcv, 66, PROXY_URL);
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
    let rd = w2
        .blind_receive(
            None,
            Assignment::Fungible(66),
            default_rcv_expiration(),
            vec![transport_endpoint(PROXY_URL)],
            1,
        )
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
    let rid = blind(&w2, 66, PROXY_URL);
    let map = HashMap::from([(asset_id.clone(), vec![recipient(&rid, 66, None, PROXY_URL)])]);
    assert!(
        w1.send_begin(
            o1.clone(),
            map,
            false,
            1,
            1,
            default_send_expiration(),
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
        let rid = blind(&w2, 66, PROXY_URL);
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
    let a1 = blind(&rcv1, 11, PROXY_URL);
    let b1 = blind(&rcv1, 25, PROXY_URL);
    let a2 = blind(&rcv2, 12, PROXY_URL);
    let b2 = blind(&rcv2, 22, PROXY_URL);
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
    assert!(
        sends(&wallet, &txid)
            .iter()
            .all(|t| t.status == TransferStatus::WaitingConfirmations)
    );

    // the consignments carry the signed TX
    let signed_tx = Psbt::from_str(&signed).unwrap().extract_tx().unwrap();
    let witness_id = RgbTxid::from_str(&txid).unwrap();
    for rid in [&a1, &b1, &a2, &b2] {
        let bytes = get_consignment_from(PROXY_URL, rid).await.0;
        let consignment = RgbTransfer::load(&bytes[..]).unwrap();
        let bundle = consignment
            .bundles
            .iter()
            .find(|b| b.witness_id() == witness_id)
            .unwrap();
        assert_eq!(bundle.tx, signed_tx);
    }

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

#[wasm_bindgen_test]
async fn donation_posted_before_end_waits_for_broadcast() {
    let (mut wallet, online) = sender().await;
    let asset_id = issue(&wallet, "TST", 1000);
    let (mut rcv, rcv_online) = sender().await;
    let rid = blind(&rcv, 66, PROXY_URL);
    let map = HashMap::from([(asset_id.clone(), vec![recipient(&rid, 66, None, PROXY_URL)])]);
    let (_, signed, txid) = begin(&mut wallet, &online, map, true, false).await;
    wallet
        .post_pending_consignments(txid.clone())
        .await
        .unwrap();

    refresh(&mut rcv, &rcv_online).await;
    assert_eq!(
        transfer_of(&rcv, &rid).status,
        TransferStatus::WaitingBroadcast
    );

    end(&mut wallet, &online, &signed).await.unwrap();
    assert_eq!(
        sends(&wallet, &txid)[0].status,
        TransferStatus::WaitingConfirmations
    );
    settle(&mut rcv, &rcv_online, std::slice::from_ref(&rid)).await;
    assert_eq!(balance(&rcv, &asset_id), 66);
}
