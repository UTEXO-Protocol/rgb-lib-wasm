use std::{collections::HashMap, str::FromStr};

use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

mod utils;

use rgb_lib_wasm::bitcoin::psbt::Psbt;
use rgb_lib_wasm::utils::script_buf_from_recipient_id;
use rgb_lib_wasm::wallet::{DatabaseType, Online, Recipient, Wallet, WalletData};
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
