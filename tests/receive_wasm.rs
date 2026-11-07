use std::{collections::HashMap, str::FromStr};

use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

mod utils;

use rgb_lib_wasm::bitcoin::psbt::Psbt;
use rgb_lib_wasm::utils::script_buf_from_recipient_id;
use rgb_lib_wasm::wallet::{
    Balance, DatabaseType, Online, ReceiveData, Recipient, Wallet, WalletData,
};
use rgb_lib_wasm::{
    AssetSchema, Assignment, BitcoinNetwork, Error, FileContent, RgbTransfer, RgbTxid,
    TransferKind, TransferStatus, bitcoin::Amount, generate_keys,
};
use utils::*;

fn test_wallet_data(schemas: Vec<AssetSchema>) -> WalletData {
    let keys = generate_keys(BitcoinNetwork::Regtest);
    WalletData {
        data_dir: "/tmp/rgb_receive".to_string(),
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

async fn create_utxos(wallet: &mut Wallet, online: &Online, num: u8) {
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

async fn new_wallet() -> (Wallet, Online) {
    let mut wallet = Wallet::new(test_wallet_data(vec![AssetSchema::Nia])).unwrap();
    let online = wallet.go_online(go_online_options(false)).await.unwrap();
    (wallet, online)
}

async fn sender() -> (Wallet, Online) {
    let (mut wallet, online) = new_wallet().await;
    fund_and_sync(&mut wallet, &online, "1.0").await;
    create_utxos(&mut wallet, &online, 10).await;
    (wallet, online)
}

fn issue(wallet: &Wallet) -> String {
    wallet
        .issue_asset_nia("TST".to_string(), "Test".to_string(), 0, vec![1000])
        .unwrap()
        .asset_id
}

// Begin, sign and end a send, optionally editing the consignment of each asset in between.
async fn begin_end(
    wallet: &mut Wallet,
    online: &Online,
    map: HashMap<String, Vec<Recipient>>,
    donation: bool,
    edit: Option<fn(&mut RgbTransfer)>,
) -> (String, String) {
    let assets: Vec<String> = map.keys().cloned().collect();
    let unsigned = wallet
        .send_begin(
            online.clone(),
            map,
            donation,
            1,
            1,
            default_send_expiration(),
            Some(transport_endpoint(PROXY_URL)),
            None,
            false,
        )
        .await
        .unwrap()
        .psbt;
    let signed = wallet.sign_psbt(unsigned, None).unwrap();
    let txid = Psbt::from_str(&signed)
        .unwrap()
        .unsigned_tx
        .compute_txid()
        .to_string();
    if let Some(edit) = edit {
        for asset_id in &assets {
            let bytes = wallet.send_consignment_bytes(&txid, asset_id).unwrap();
            let mut consignment = RgbTransfer::load(&bytes[..]).unwrap();
            edit(&mut consignment);
            wallet
                .set_send_consignment_bytes(&txid, asset_id, save(&consignment))
                .unwrap();
        }
    }
    wallet
        .send_end(online.clone(), signed.clone(), false)
        .await
        .unwrap();
    (signed, txid)
}

fn vout_paying(signed: &str, recipient_id: &str) -> u32 {
    let script = script_buf_from_recipient_id(recipient_id.to_string())
        .unwrap()
        .unwrap();
    Psbt::from_str(signed)
        .unwrap()
        .unsigned_tx
        .output
        .iter()
        .position(|o| o.script_pubkey == script)
        .unwrap() as u32
}

fn save(consignment: &RgbTransfer) -> Vec<u8> {
    let mut bytes = vec![];
    consignment.save(&mut bytes).unwrap();
    bytes
}

fn waiting_for_tx(wallet: &Wallet, rid: &str) -> bool {
    matches!(
        transfer_of(wallet, rid).status,
        TransferStatus::WaitingBroadcast | TransferStatus::WaitingConfirmations
    )
}

async fn receive_witness(wallet: &mut Wallet, online: &Online, amount: u64, url: &str) -> String {
    witness_rcv(
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

#[wasm_bindgen_test]
async fn witness_receive_locates_payment() {
    let (mut wallet_a, online_a) = sender().await;
    let asset_id = issue(&wallet_a);
    let (mut wallet_b, online_b) = new_wallet().await;

    let mut rids = vec![];
    for amount in [66, 34, 10] {
        rids.push(receive_witness(&mut wallet_b, &online_b, amount, PROXY_URL).await);
    }
    let map = HashMap::from([(
        asset_id.clone(),
        vec![
            recipient(&rids[0], 66, Some(2000), PROXY_URL),
            recipient(&rids[1], 34, Some(1500), PROXY_URL),
        ],
    )]);
    let (signed, txid) = begin_end(&mut wallet_a, &online_a, map, false, None).await;

    refresh(&mut wallet_b, &online_b).await;
    for rid in &rids[..2] {
        let transfer = transfer_of(&wallet_b, rid);
        assert!(waiting_for_tx(&wallet_b, rid));
        assert_eq!(transfer.kind, TransferKind::ReceiveWitness);
        assert_eq!(transfer.txid, Some(txid.clone()));
    }
    // nothing was sent to the third invoice
    assert_eq!(
        transfer_of(&wallet_b, &rids[2]).status,
        TransferStatus::WaitingCounterparty
    );

    settle(&mut wallet_b, &online_b, &rids[..2]).await;
    for (rid, amount) in rids.iter().zip([66, 34]) {
        let transfer = transfer_of(&wallet_b, rid);
        assert_eq!(transfer.status, TransferStatus::Settled);
        assert_eq!(transfer.assignments, vec![Assignment::Fungible(amount)]);
        let utxo = transfer.receive_utxo.unwrap();
        assert_eq!(utxo.txid, txid);
        assert_eq!(utxo.vout, vout_paying(&signed, rid));
    }
    assert_eq!(wallet_b.get_asset_balance(asset_id).unwrap().settled, 100);
}

#[wasm_bindgen_test]
async fn blind_receive_locates_payment() {
    let (mut wallet_a, online_a) = sender().await;
    let asset_id = issue(&wallet_a);
    let (mut wallet_b, online_b) = new_wallet().await;
    fund_and_sync(&mut wallet_b, &online_b, "1.0").await;
    create_utxos(&mut wallet_b, &online_b, 1).await;

    let rid = blind_rcv(
        &mut wallet_b,
        &online_b,
        None,
        Assignment::Fungible(66),
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap()
    .recipient_id;
    let map = HashMap::from([(asset_id.clone(), vec![recipient(&rid, 66, None, PROXY_URL)])]);
    let (_, txid) = begin_end(&mut wallet_a, &online_a, map, false, None).await;

    refresh(&mut wallet_b, &online_b).await;
    let transfer = transfer_of(&wallet_b, &rid);
    // the TX of a single-recipient send is released with the ACK and broadcast by the recipient
    assert_eq!(transfer.status, TransferStatus::WaitingConfirmations);
    assert_eq!(transfer.kind, TransferKind::ReceiveBlind);
    assert_eq!(transfer.txid, Some(txid));
    let b = wallet_b.get_asset_balance(asset_id.clone()).unwrap();
    assert_eq!((b.settled, b.future), (0, 66));

    settle(&mut wallet_b, &online_b, std::slice::from_ref(&rid)).await;
    let transfer = transfer_of(&wallet_b, &rid);
    assert_eq!(transfer.assignments, vec![Assignment::Fungible(66)]);
    assert_eq!(
        wallet_b
            .get_asset_balance(asset_id.clone())
            .unwrap()
            .settled,
        66
    );
    settle(&mut wallet_a, &online_a, std::slice::from_ref(&rid)).await;
    assert_eq!(wallet_a.get_asset_balance(asset_id).unwrap().settled, 934);
}

#[wasm_bindgen_test]
async fn receive_refuses_consignment_without_terminals() {
    let (mut wallet_a, online_a) = sender().await;
    let asset_id = issue(&wallet_a);
    let (mut wallet_b, online_b) = new_wallet().await;
    fund_and_sync(&mut wallet_b, &online_b, "1.0").await;
    create_utxos(&mut wallet_b, &online_b, 1).await;

    let rid = blind_rcv(
        &mut wallet_b,
        &online_b,
        None,
        Assignment::Fungible(66),
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap()
    .recipient_id;
    let map = HashMap::from([(asset_id, vec![recipient(&rid, 66, None, PROXY_URL)])]);
    let edit: fn(&mut RgbTransfer) = |c| {
        assert!(!c.terminals.is_empty());
        c.terminals = Default::default();
    };
    begin_end(&mut wallet_a, &online_a, map, false, Some(edit)).await;

    refresh(&mut wallet_b, &online_b).await;
    assert_eq!(transfer_of(&wallet_b, &rid).status, TransferStatus::Failed);
}

#[wasm_bindgen_test]
async fn witness_receive_multiple_assets_success() {
    let (mut wallet_a, online_a) = sender().await;
    let asset_1 = issue(&wallet_a);
    let asset_2 = issue(&wallet_a);
    let (mut wallet_b, online_b) = new_wallet().await;

    let mut rids = vec![];
    for (amount, url) in [
        (66, PROXY_URL),
        (132, PROXY_URL),
        (198, PROXY_B_URL),
        (264, PROXY_B_URL),
    ] {
        rids.push(receive_witness(&mut wallet_b, &online_b, amount, url).await);
    }
    let map = HashMap::from([
        (
            asset_1.clone(),
            vec![
                recipient(&rids[0], 66, Some(1600), PROXY_URL),
                recipient(&rids[1], 132, Some(1200), PROXY_URL),
            ],
        ),
        (
            asset_2.clone(),
            vec![
                recipient(&rids[2], 198, Some(1000), PROXY_B_URL),
                recipient(&rids[3], 264, Some(1400), PROXY_B_URL),
            ],
        ),
    ]);
    let (signed, txid) = begin_end(&mut wallet_a, &online_a, map, false, None).await;

    refresh(&mut wallet_b, &online_b).await;
    for rid in &rids {
        let transfer = transfer_of(&wallet_b, rid);
        assert!(waiting_for_tx(&wallet_b, rid));
        assert_eq!(transfer.kind, TransferKind::ReceiveWitness);
        assert_eq!(transfer.txid, Some(txid.clone()));
    }

    settle(&mut wallet_b, &online_b, &rids).await;
    for (rid, amount) in rids.iter().zip([66, 132, 198, 264]) {
        let transfer = transfer_of(&wallet_b, rid);
        assert_eq!(transfer.status, TransferStatus::Settled);
        assert_eq!(transfer.assignments, vec![Assignment::Fungible(amount)]);
        assert_eq!(
            transfer.receive_utxo.unwrap().vout,
            vout_paying(&signed, rid)
        );
    }
    assert_eq!(wallet_b.get_asset_balance(asset_1).unwrap().settled, 198);
    assert_eq!(wallet_b.get_asset_balance(asset_2).unwrap().settled, 462);
}

#[wasm_bindgen_test]
async fn witness_receive_accepts_repeated_terminal_seal() {
    let (mut wallet_a, online_a) = sender().await;
    let asset_id = issue(&wallet_a);
    let (mut wallet_b, online_b) = new_wallet().await;

    let rid = receive_witness(&mut wallet_b, &online_b, 66, PROXY_URL).await;
    let map = HashMap::from([(asset_id, vec![recipient(&rid, 66, Some(2000), PROXY_URL)])]);
    let edit: fn(&mut RgbTransfer) = |c| {
        assert_eq!(c.terminals.len(), 1);
        let seals = c.terminals.values_mut().next().unwrap();
        assert_eq!(seals.len(), 1);
        let seal = seals.iter().next().copied().unwrap();
        seals.push(seal).unwrap();
    };
    begin_end(&mut wallet_a, &online_a, map, false, Some(edit)).await;

    refresh(&mut wallet_b, &online_b).await;
    // the TX of a single-recipient send is released with the ACK and broadcast by the recipient
    assert_eq!(
        transfer_of(&wallet_b, &rid).status,
        TransferStatus::WaitingConfirmations
    );
}

#[wasm_bindgen_test]
async fn donation_receive_broadcasts_signed_tx() {
    let (mut wallet_a, online_a) = sender().await;
    let asset_id = issue(&wallet_a);
    let (mut wallet_b, online_b) = new_wallet().await;
    let rid = receive_witness(&mut wallet_b, &online_b, 66, PROXY_URL).await;
    let map = HashMap::from([(
        asset_id.clone(),
        vec![recipient(&rid, 66, Some(2000), PROXY_URL)],
    )]);
    // a donation embeds the signed TX in the consignment, which the recipient broadcasts
    begin_end(&mut wallet_a, &online_a, map, true, None).await;

    refresh(&mut wallet_b, &online_b).await;
    assert_eq!(
        transfer_of(&wallet_b, &rid).status,
        TransferStatus::WaitingConfirmations
    );
    settle(&mut wallet_b, &online_b, std::slice::from_ref(&rid)).await;
    assert_eq!(wallet_b.get_asset_balance(asset_id).unwrap().settled, 66);
}

// The witness that pays the receiver is on chain, but a witness in its history is not: the
// consignment is refused and the asset is not imported.
#[wasm_bindgen_test]
async fn receive_refuses_history_witness_not_on_chain() {
    let (mut wallet_a, online_a) = sender().await;
    let asset_id = issue(&wallet_a);
    let (mut wallet_b, online_b) = sender().await;
    let rid_b = receive_witness(&mut wallet_b, &online_b, 200, PROXY_URL).await;
    let map = HashMap::from([(
        asset_id.clone(),
        vec![recipient(&rid_b, 200, Some(2000), PROXY_URL)],
    )]);
    let (_, w1) = begin_end(&mut wallet_a, &online_a, map, false, None).await;
    settle(&mut wallet_b, &online_b, std::slice::from_ref(&rid_b)).await;
    assert_eq!(
        wallet_b
            .get_asset_balance(asset_id.clone())
            .unwrap()
            .settled,
        200
    );

    // B sends to C, but changes the A to B witness in the consignment: same commitment and
    // inputs, one sat less, so a txid that is not on chain
    let (mut wallet_c, online_c) = new_wallet().await;
    let rid_c = receive_witness(&mut wallet_c, &online_c, 100, PROXY_URL).await;
    let map = HashMap::from([(
        asset_id.clone(),
        vec![recipient(&rid_c, 100, Some(2000), PROXY_URL)],
    )]);
    let unsigned = wallet_b
        .send_begin(
            online_b.clone(),
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
        .unwrap()
        .psbt;
    let signed = wallet_b.sign_psbt(unsigned, None).unwrap();
    let w2 = Psbt::from_str(&signed)
        .unwrap()
        .unsigned_tx
        .compute_txid()
        .to_string();
    let bytes = wallet_b.send_consignment_bytes(&w2, &asset_id).unwrap();
    let mut consignment = RgbTransfer::load(&bytes[..]).unwrap();
    assert!(
        consignment.modify_bundle(RgbTxid::from_str(&w1).unwrap(), |bw| {
            let out = bw.tx.output.iter_mut().max_by_key(|o| o.value).unwrap();
            out.value = Amount::from_sat(out.value.to_sat() - 1);
        })
    );
    wallet_b
        .set_send_consignment_bytes(&w2, &asset_id, save(&consignment))
        .unwrap();
    wallet_b
        .send_end(online_b.clone(), signed, false)
        .await
        .unwrap();

    refresh(&mut wallet_c, &online_c).await;
    assert_eq!(
        transfer_of(&wallet_c, &rid_c).status,
        TransferStatus::Failed
    );
    assert!(
        wallet_c
            .list_assets(vec![AssetSchema::Nia])
            .unwrap()
            .nia
            .unwrap_or_default()
            .is_empty(),
        "C must not import the asset"
    );
}

// Each refresh fails to post the ACK and leaves nothing behind, so the receive counts once when
// the ACK goes through.
#[wasm_bindgen_test]
async fn ack_failure_no_duplicate_colorings() {
    let (mut wallet, online) = sender().await;
    let asset_id = issue(&wallet);
    let (mut rcv, rcv_online) = sender().await;
    let data = blind_rcv(
        &mut rcv,
        &rcv_online,
        None,
        Assignment::Fungible(66),
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap();
    let rid = data.recipient_id;
    let idx = data.batch_transfer_idx;
    let map = HashMap::from([(asset_id.clone(), vec![recipient(&rid, 66, None, PROXY_URL)])]);
    begin_end(&mut wallet, &online, map, false, None).await;

    clear_rpc_hooks();
    rpc_fail(PROXY_URL, "ack.post");
    for _ in 0..3 {
        let res = rcv
            .refresh(rcv_online.clone(), None, vec![], false)
            .await
            .unwrap();
        assert!(matches!(res[&idx].failure, Some(Error::Proxy { .. })));
        assert_eq!(res[&idx].updated_status, None);
        assert_eq!(
            transfer_of(&rcv, &rid).status,
            TransferStatus::WaitingCounterparty
        );
    }
    assert_eq!(rpc_count("consignment.get"), 1);

    assert!(rcv.rgb_contract_ids().unwrap().is_empty());
    assert!(matches!(
        rcv.get_asset_balance(asset_id.clone()),
        Err(Error::AssetNotFound { .. })
    ));
    assert!(
        rcv.list_unspents(None, false, true)
            .unwrap()
            .iter()
            .all(|u| u.rgb_allocations.is_empty())
    );

    clear_rpc_hooks();
    let res = rcv
        .refresh(rcv_online.clone(), None, vec![], false)
        .await
        .unwrap();
    assert_eq!(res[&idx].failure, None);
    assert_eq!(
        res[&idx].updated_status,
        Some(TransferStatus::WaitingConfirmations)
    );
    assert_eq!(rpc_count("consignment.get"), 0);
    assert_eq!(
        rcv.get_asset_balance(asset_id).unwrap(),
        Balance {
            settled: 0,
            future: 66,
            spendable: 0,
        }
    );

    // A fail goes on after a fee rejection and keeps nothing of the receive.
    let (mut wallet, online, asset_id, mut rcv, rcv_online, data) =
        witness_receive_of_empty_recipient().await;
    let rid = data.recipient_id;
    let txid = donate_to(&mut wallet, &online, &asset_id, &mut rcv, &rcv_online, &rid).await;
    reject_broadcasts("min relay fee not met");
    let res = rcv
        .fail_transfers(
            rcv_online.clone(),
            Some(data.batch_transfer_idx),
            false,
            false,
        )
        .await;
    clear_rpc_hooks();
    assert_eq!(res, Ok(true));
    let transfer = transfer_of(&rcv, &rid);
    assert_eq!(transfer.status, TransferStatus::Failed);
    assert!(transfer.assignments.is_empty());
    assert!(rcv.rgb_contract_ids().unwrap().is_empty());
    assert_only_btc_received(&mut rcv, &txid);
}

// A sender with an asset and a recipient that has a witness receive and no colorable UTXO.
async fn witness_receive_of_empty_recipient()
-> (Wallet, Online, String, Wallet, Online, ReceiveData) {
    let (wallet, online) = sender().await;
    let asset_id = issue(&wallet);
    let (mut rcv, rcv_online) = new_wallet().await;
    // the vanilla funds make the wallet report the lack of allocation slots
    fund_and_sync(&mut rcv, &rcv_online, "0.1").await;
    let data = witness_rcv(
        &mut rcv,
        &rcv_online,
        None,
        Assignment::Fungible(66),
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap();
    (wallet, online, asset_id, rcv, rcv_online, data)
}

// The sender pays the receive with a donation, which it broadcasts at once.
async fn donate_to(
    wallet: &mut Wallet,
    online: &Online,
    asset_id: &str,
    rcv: &mut Wallet,
    rcv_online: &Online,
    rid: &str,
) -> String {
    let map = HashMap::from([(
        asset_id.to_string(),
        vec![recipient(rid, 66, Some(2000), PROXY_URL)],
    )]);
    let (_, txid) = begin_end(wallet, online, map, true, None).await;
    // esplora indexes the mempool with a delay
    for _ in 0..60 {
        rcv.sync(rcv_online.clone()).await.unwrap();
        let unspents = rcv.list_unspents(None, false, true).unwrap();
        if unspents.iter().any(|u| u.utxo.outpoint.txid == txid) {
            return txid;
        }
        sleep_ms(500).await;
    }
    panic!("the wallet did not see the donation");
}

fn assert_only_btc_received(rcv: &mut Wallet, txid: &str) {
    let unspents = rcv.list_unspents(None, false, true).unwrap();
    let unspent = unspents
        .iter()
        .find(|u| u.utxo.outpoint.txid == txid)
        .unwrap();
    assert!(unspent.rgb_allocations.is_empty());
    assert!(
        rcv.list_assets(vec![])
            .unwrap()
            .nia
            .unwrap_or_default()
            .is_empty()
    );
}

// A blind receive needs a colorable UTXO that is not flagged as pending witness.
async fn blind_receive_result(rcv: &mut Wallet, online: &Online) -> Result<ReceiveData, Error> {
    blind_rcv(
        rcv,
        online,
        None,
        Assignment::Any,
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
}

#[wasm_bindgen_test]
async fn witness_receive_cleanup_payment_after_fail() {
    let (mut wallet, online, asset_id, mut rcv, rcv_online, data) =
        witness_receive_of_empty_recipient().await;
    let rid = data.recipient_id;
    let idx = data.batch_transfer_idx;
    assert_eq!(
        rcv.fail_transfers(rcv_online.clone(), Some(idx), false, false)
            .await,
        Ok(true)
    );

    let txid = donate_to(&mut wallet, &online, &asset_id, &mut rcv, &rcv_online, &rid).await;

    assert_only_btc_received(&mut rcv, &txid);
    refresh(&mut rcv, &rcv_online).await;
    assert_eq!(transfer_of(&rcv, &rid).status, TransferStatus::Failed);
    blind_receive_result(&mut rcv, &rcv_online).await.unwrap();
}

#[wasm_bindgen_test]
async fn witness_receive_cleanup_payment_before_refusal() {
    let (mut wallet, online, asset_id, mut rcv, rcv_online, data) =
        witness_receive_of_empty_recipient().await;
    let rid = data.recipient_id;

    let txid = donate_to(&mut wallet, &online, &asset_id, &mut rcv, &rcv_online, &rid).await;
    assert!(matches!(
        blind_receive_result(&mut rcv, &rcv_online).await,
        Err(Error::InsufficientAllocationSlots)
    ));

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
    assert_only_btc_received(&mut rcv, &txid);
    blind_receive_result(&mut rcv, &rcv_online).await.unwrap();
}
