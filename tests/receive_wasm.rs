use std::{collections::HashMap, str::FromStr};

use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

mod utils;

use rgb_lib_wasm::bitcoin::{consensus::encode::serialize_hex, psbt::Psbt};
use rgb_lib_wasm::utils::script_buf_from_recipient_id;
use rgb_lib_wasm::wallet::{
    AssetFilter, DatabaseType, Online, Recipient, Transfer, Wallet, WalletData, WitnessData,
};
use rgb_lib_wasm::{
    AssetSchema, Assignment, BitcoinNetwork, FileContent, RgbTransfer, TransferKind,
    TransferStatus, generate_keys,
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

async fn fund_and_sync(wallet: &mut Wallet, online: &Online, amount: &str) {
    let addr = wallet.get_address().unwrap();
    fund_address(&addr, amount).await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();
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

fn transport_endpoint(url: &str) -> String {
    format!("rpc://{}", url.trim_start_matches("http://"))
}

fn recipient(id: &str, amount: u64, sat: Option<u64>, url: &str) -> Recipient {
    Recipient {
        recipient_id: id.to_string(),
        witness_data: sat.map(|amount_sat| WitnessData {
            amount_sat,
            blinding: None,
        }),
        assignment: Assignment::Fungible(amount),
        transport_endpoints: vec![transport_endpoint(url)],
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
    create_utxos(&mut wallet, &online, 10).await;
    (wallet, online)
}

fn issue(wallet: &Wallet) -> String {
    wallet
        .issue_asset_nia("TST".to_string(), "Test".to_string(), 0, vec![1000])
        .unwrap()
        .asset_id
}

async fn begin_send(
    wallet: &mut Wallet,
    online: &Online,
    map: HashMap<String, Vec<Recipient>>,
) -> (String, String) {
    let unsigned = wallet
        .send_begin(
            online.clone(),
            map,
            false,
            1,
            1,
            default_send_expiration(),
            None,
        )
        .await
        .unwrap();
    let signed = wallet.sign_psbt(unsigned, None).unwrap();
    let txid = Psbt::from_str(&signed)
        .unwrap()
        .unsigned_tx
        .compute_txid()
        .to_string();
    wallet.post_consignment_by_txid(txid.clone()).await.unwrap();
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

async fn broadcast(signed: &str) {
    let tx = Psbt::from_str(signed).unwrap().extract_tx().unwrap();
    reqwest::Client::new()
        .post(format!("{}/tx", ESPLORA_URL))
        .body(serialize_hex(&tx))
        .send()
        .await
        .expect("broadcast request failed");
}

fn transfer_of(wallet: &Wallet, recipient_id: &str) -> Transfer {
    wallet
        .list_transfers(AssetFilter::Any, None)
        .unwrap()
        .into_iter()
        .find(|t| t.recipient_id.as_deref() == Some(recipient_id))
        .unwrap()
}

async fn refresh(wallet: &mut Wallet, online: &Online) {
    wallet
        .refresh(online.clone(), None, vec![], false)
        .await
        .unwrap();
}

async fn settle(wallet: &mut Wallet, online: &Online, recipient_ids: &[String]) {
    for _ in 0..5 {
        mine_blocks(1).await;
        wait_for_esplora_sync().await;
        wallet.sync(online.clone()).await.unwrap();
        refresh(wallet, online).await;
        if recipient_ids
            .iter()
            .all(|r| transfer_of(wallet, r).status == TransferStatus::Settled)
        {
            return;
        }
        sleep_ms(1000).await;
    }
    let statuses: Vec<_> = recipient_ids
        .iter()
        .map(|r| transfer_of(wallet, r).status)
        .collect();
    panic!("transfers did not settle: {statuses:?}");
}

fn save(consignment: &RgbTransfer) -> Vec<u8> {
    let mut bytes = vec![];
    consignment.save(&mut bytes).unwrap();
    bytes
}

#[wasm_bindgen_test]
async fn witness_receive_settles_without_proxy_txid_and_vout() {
    let (mut wallet_a, online_a) = sender().await;
    let asset_id = issue(&wallet_a);
    let (mut wallet_b, online_b) = new_wallet().await;
    let decoy = fund_address(&wallet_b.get_address().unwrap(), "0.001").await;

    let mut rids = vec![];
    for amount in [66, 34, 10] {
        let rid = wallet_b
            .witness_receive(
                None,
                Assignment::Fungible(amount),
                default_rcv_expiration(),
                vec![transport_endpoint(PROXY_URL)],
                1,
            )
            .unwrap()
            .recipient_id;
        rids.push(rid);
    }
    let map = HashMap::from([(
        asset_id.clone(),
        vec![
            recipient(&rids[0], 66, Some(2000), PROXY_URL),
            recipient(&rids[1], 34, Some(1500), PROXY_URL),
        ],
    )]);
    let (signed, txid) = begin_send(&mut wallet_a, &online_a, map).await;
    let bytes = get_consignment_from(PROXY_URL, &txid).await.0;
    for rid in &rids {
        wallet_a
            .post_consignment(PROXY_URL, rid.clone(), &bytes, decoy.clone(), None)
            .await
            .unwrap();
    }
    wallet_a
        .post_pending_consignments(txid.clone())
        .await
        .unwrap();

    refresh(&mut wallet_b, &online_b).await;
    for rid in &rids[..2] {
        let transfer = transfer_of(&wallet_b, rid);
        assert_eq!(transfer.status, TransferStatus::WaitingConfirmations);
        assert_eq!(transfer.kind, TransferKind::ReceiveWitness);
        assert_eq!(transfer.txid, Some(txid.clone()));
    }
    assert_eq!(
        transfer_of(&wallet_b, &rids[2]).status,
        TransferStatus::Failed
    );

    broadcast(&signed).await;
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
async fn blind_receive_settles_without_proxy_txid() {
    let (mut wallet_a, online_a) = sender().await;
    let asset_id = issue(&wallet_a);
    let (mut wallet_b, online_b) = new_wallet().await;
    fund_and_sync(&mut wallet_b, &online_b, "1.0").await;
    create_utxos(&mut wallet_b, &online_b, 1).await;
    let decoy = fund_address(&wallet_b.get_address().unwrap(), "0.001").await;

    let rid = wallet_b
        .blind_receive(
            None,
            Assignment::Fungible(66),
            default_rcv_expiration(),
            vec![transport_endpoint(PROXY_URL)],
            1,
        )
        .unwrap()
        .recipient_id;
    let map = HashMap::from([(asset_id.clone(), vec![recipient(&rid, 66, None, PROXY_URL)])]);
    let (signed, txid) = begin_send(&mut wallet_a, &online_a, map).await;
    let bytes = get_consignment_from(PROXY_URL, &txid).await.0;
    wallet_a
        .post_consignment(PROXY_URL, rid.clone(), &bytes, decoy, None)
        .await
        .unwrap();

    refresh(&mut wallet_b, &online_b).await;
    let transfer = transfer_of(&wallet_b, &rid);
    assert_eq!(transfer.status, TransferStatus::WaitingConfirmations);
    assert_eq!(transfer.kind, TransferKind::ReceiveBlind);
    assert_eq!(transfer.txid, Some(txid));

    broadcast(&signed).await;
    settle(&mut wallet_b, &online_b, std::slice::from_ref(&rid)).await;
    let transfer = transfer_of(&wallet_b, &rid);
    assert_eq!(transfer.assignments, vec![Assignment::Fungible(66)]);
    assert_eq!(wallet_b.get_asset_balance(asset_id).unwrap().settled, 66);
}

#[wasm_bindgen_test]
async fn receive_refuses_consignment_without_terminals() {
    let (mut wallet_a, online_a) = sender().await;
    let asset_id = issue(&wallet_a);
    let (mut wallet_b, online_b) = new_wallet().await;
    fund_and_sync(&mut wallet_b, &online_b, "1.0").await;
    create_utxos(&mut wallet_b, &online_b, 1).await;

    let rid = wallet_b
        .blind_receive(
            None,
            Assignment::Fungible(66),
            default_rcv_expiration(),
            vec![transport_endpoint(PROXY_URL)],
            1,
        )
        .unwrap()
        .recipient_id;
    let map = HashMap::from([(asset_id, vec![recipient(&rid, 66, None, PROXY_URL)])]);
    let (_, txid) = begin_send(&mut wallet_a, &online_a, map).await;
    let bytes = get_consignment_from(PROXY_URL, &txid).await.0;
    let mut consignment = RgbTransfer::load(&bytes[..]).unwrap();
    assert!(!consignment.terminals.is_empty());
    consignment.terminals = Default::default();
    wallet_a
        .post_consignment(PROXY_URL, rid.clone(), &save(&consignment), txid, None)
        .await
        .unwrap();

    refresh(&mut wallet_b, &online_b).await;
    assert_eq!(transfer_of(&wallet_b, &rid).status, TransferStatus::Failed);
}

#[wasm_bindgen_test]
async fn witness_receive_multiple_assets_success() {
    let (mut wallet_a, online_a) = sender().await;
    let asset_1 = issue(&wallet_a);
    let asset_2 = issue(&wallet_a);
    let (mut wallet_b, online_b) = new_wallet().await;
    let decoy = fund_address(&wallet_b.get_address().unwrap(), "0.001").await;

    let mut rids = vec![];
    for (amount, url) in [
        (66, PROXY_URL),
        (132, PROXY_URL),
        (198, PROXY_B_URL),
        (264, PROXY_B_URL),
    ] {
        let rid = wallet_b
            .witness_receive(
                None,
                Assignment::Fungible(amount),
                default_rcv_expiration(),
                vec![transport_endpoint(url)],
                1,
            )
            .unwrap()
            .recipient_id;
        rids.push(rid);
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
    let (signed, txid) = begin_send(&mut wallet_a, &online_a, map).await;
    let bytes_1 = get_consignment_from(PROXY_URL, &txid).await.0;
    let bytes_2 = get_consignment_from(PROXY_B_URL, &txid).await.0;
    assert_ne!(bytes_1, bytes_2);
    for (rid, url, bytes) in [
        (&rids[0], PROXY_URL, &bytes_1),
        (&rids[1], PROXY_URL, &bytes_1),
        (&rids[2], PROXY_B_URL, &bytes_2),
        (&rids[3], PROXY_B_URL, &bytes_2),
    ] {
        wallet_a
            .post_consignment(url, rid.clone(), bytes, decoy.clone(), None)
            .await
            .unwrap();
    }

    refresh(&mut wallet_b, &online_b).await;
    for rid in &rids {
        let transfer = transfer_of(&wallet_b, rid);
        assert_eq!(transfer.status, TransferStatus::WaitingConfirmations);
        assert_eq!(transfer.kind, TransferKind::ReceiveWitness);
        assert_eq!(transfer.txid, Some(txid.clone()));
    }

    broadcast(&signed).await;
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

    let rid = wallet_b
        .witness_receive(
            None,
            Assignment::Fungible(66),
            default_rcv_expiration(),
            vec![transport_endpoint(PROXY_URL)],
            1,
        )
        .unwrap()
        .recipient_id;
    let map = HashMap::from([(asset_id, vec![recipient(&rid, 66, Some(2000), PROXY_URL)])]);
    let (signed, txid) = begin_send(&mut wallet_a, &online_a, map).await;
    let bytes = get_consignment_from(PROXY_URL, &txid).await.0;
    let mut consignment = RgbTransfer::load(&bytes[..]).unwrap();
    assert_eq!(consignment.terminals.len(), 1);
    let seals = consignment.terminals.values_mut().next().unwrap();
    assert_eq!(seals.len(), 1);
    let seal = seals.iter().next().copied().unwrap();
    seals.push(seal).unwrap();
    wallet_a
        .post_consignment(
            PROXY_URL,
            rid.clone(),
            &save(&consignment),
            txid,
            Some(vout_paying(&signed, &rid)),
        )
        .await
        .unwrap();

    refresh(&mut wallet_b, &online_b).await;
    assert_eq!(
        transfer_of(&wallet_b, &rid).status,
        TransferStatus::WaitingConfirmations
    );
}
