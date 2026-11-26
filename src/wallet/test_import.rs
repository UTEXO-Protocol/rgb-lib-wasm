use std::collections::{HashMap, HashSet};
use wasm_bindgen_test::*;

use super::offline::MAX_CONTRACT_SIZE;
use super::test_bfa::utils::*;
use super::test_bfa::{
    funded_wallet, s, send, send_and_settle, send_recipients, settle_both, wallet_data,
};
use super::test_burn::inflate_and_settle;
use crate::encryption::sha256_digest;
use crate::no_wallet::RgbRejectListInfo;
use crate::wallet::idb_store::{load_snapshot, save_snapshot};
use crate::wallet::offline::test_wallet::{
    add_fake_txos, assert_no_partial_change, set_media, state,
};
use crate::wallet::{
    AssetFilter, Balance, Invoice, Online, SinglesigKeys, TransferScope, Wallet, WalletData,
};
use crate::{
    AssetSchema, Assignment, BitcoinNetwork, Error, FileContent, OS_ASSET, Operation, RgbTransfer,
    TransferStatus, WitnessVersion, generate_keys,
};
use rgbstd::Opout;
use rgbstd::containers::Contract;

fn zero() -> Balance {
    Balance {
        settled: 0,
        future: 0,
        spendable: 0,
    }
}

/// A wallet with no UTXOs, online.
async fn online_wallet(schemas: Vec<AssetSchema>) -> (Wallet, Online) {
    let (wd, keys) = wallet_data(schemas);
    let mut wallet = Wallet::new(wd, keys).unwrap();
    let online = wallet.go_online(go_online_options(false)).await.unwrap();
    (wallet, online)
}

/// Send 100 of `asset_id` from `a` to `c` and settle. Return the consignment and its TXID.
async fn sent_consignment(
    a: &mut Wallet,
    a_on: &Online,
    asset_id: &str,
    c: &mut Wallet,
    c_on: &Online,
) -> (Vec<u8>, String) {
    let txid = send(a, a_on, c, c_on, asset_id, Assignment::Fungible(100)).await;
    let bytes = a.send_consignment_bytes(&txid, asset_id).unwrap();
    settle_both(a, a_on, c, c_on).await;
    (bytes, txid)
}

async fn nia_consignment() -> (Wallet, Online, String, Vec<u8>, String) {
    let (mut a, a_on) = funded_wallet(wallet_data(vec![AssetSchema::Nia]), 2).await;
    let (mut c, c_on) = funded_wallet(wallet_data(vec![AssetSchema::Nia]), 1).await;
    let asset_id = a
        .issue_asset_nia(s("NIA"), s("Fixed"), 0, vec![1000])
        .unwrap()
        .asset_id;
    let (bytes, txid) = sent_consignment(&mut a, &a_on, &asset_id, &mut c, &c_on).await;
    (a, a_on, asset_id, bytes, txid)
}

fn nia_listed(wallet: &Wallet) -> Vec<(String, Balance)> {
    wallet
        .list_assets(vec![AssetSchema::Nia])
        .unwrap()
        .nia
        .unwrap()
        .into_iter()
        .map(|a| (a.asset_id, a.balance))
        .collect()
}

#[wasm_bindgen_test]
async fn import_contract_then_receive() {
    let (mut a, a_on, asset_id, bytes, txid) = nia_consignment().await;
    let (mut b, b_on) = online_wallet(vec![AssetSchema::Nia]).await;
    let endpoints = vec![transport_endpoint(PROXY_URL)];

    let unknown = witness_rcv(
        &mut b,
        &b_on,
        Some(asset_id.clone()),
        Assignment::Fungible(300),
        default_rcv_expiration(),
        endpoints.clone(),
    )
    .await;
    assert!(matches!(unknown, Err(Error::AssetNotFound { .. })));

    assert_no_partial_change(&mut b, "save_new_asset", 0, async |w| {
        w.save_new_asset(b_on.clone(), &bytes, &txid).await
    })
    .await;
    b.save_new_asset(b_on.clone(), &bytes, &txid).await.unwrap();
    assert_eq!(nia_listed(&b), vec![(asset_id.clone(), zero())]);
    // a second import changes nothing
    b.save_new_asset(b_on.clone(), &bytes, &txid).await.unwrap();
    assert_eq!(nia_listed(&b).len(), 1);
    assert_eq!(b.rgb_contract_ids().unwrap().len(), 1);

    // the import is saved: reload with no flush
    let (wd_b, keys_b) = (b.get_wallet_data(), b.get_keys());
    drop(b);
    let mut b = Wallet::restore(wd_b.clone(), keys_b.clone()).await.unwrap();
    let b_on = b.go_online(go_online_options(true)).await.unwrap();
    assert_eq!(nia_listed(&b), vec![(asset_id.clone(), zero())]);

    let backup = b.backup("import").unwrap();
    let mut restored = Wallet::new(
        WalletData {
            data_dir: format!("/tmp/import-{}", js_sys::Math::random()),
            ..wd_b
        },
        keys_b.clone(),
    )
    .unwrap();
    restored.restore_backup(&backup, "import").unwrap();
    assert_eq!(nia_listed(&restored), vec![(asset_id.clone(), zero())]);
    assert!(
        restored
            .rgb_contract_ids()
            .unwrap()
            .iter()
            .any(|c| c.to_string() == asset_id)
    );

    // a witness invoice for the asset needs no BTC
    assert!(b.list_unspents(None, false, true).await.unwrap().is_empty());
    let receive = witness_rcv(
        &mut b,
        &b_on,
        Some(asset_id.clone()),
        Assignment::Fungible(300),
        default_rcv_expiration(),
        endpoints,
    )
    .await
    .unwrap();
    let invoice = Invoice::new(receive.invoice.clone()).unwrap();
    assert_eq!(invoice.invoice_data().asset_id, Some(asset_id.clone()));

    let rid = receive.recipient_id;
    let recipient_map = HashMap::from([(
        asset_id.clone(),
        vec![recipient(&rid, 300, Some(1000), PROXY_URL)],
    )]);
    let begin = a
        .send_begin(
            a_on.clone(),
            recipient_map,
            false,
            1,
            1,
            default_send_expiration(),
            Some(transport_endpoint(PROXY_URL)),
            None,
            false,
        )
        .await
        .unwrap();
    let signed = a.sign_psbt(begin.psbt, None).unwrap();
    a.send_end(a_on.clone(), signed).await.unwrap();
    settle_both(&mut a, &a_on, &mut b, &b_on).await;

    assert_eq!(transfer_of(&b, &rid).status, TransferStatus::Settled);
    assert_eq!(
        b.get_asset_balance(asset_id.clone()).unwrap(),
        Balance {
            settled: 300,
            future: 300,
            spendable: 300,
        }
    );
    assert_eq!(a.get_asset_balance(asset_id).unwrap().settled, 600);
}

#[wasm_bindgen_test]
async fn import_contract_checks_and_blind_invoice() {
    let (_a, _a_on, asset_id, bytes, txid) = nia_consignment().await;

    let (d, d_on) = online_wallet(vec![AssetSchema::Ifa]).await;
    assert!(matches!(
        d.save_new_asset(d_on, &bytes, &txid).await,
        Err(Error::UnsupportedSchema {
            asset_schema: AssetSchema::Nia
        })
    ));
    assert!(d.rgb_contract_ids().unwrap().is_empty());

    let mut tampered = RgbTransfer::load(&bytes[..]).unwrap();
    tampered.genesis.timestamp += 1;
    let mut tampered_bytes = vec![];
    tampered.save(&mut tampered_bytes).unwrap();
    let (mut e, e_on) = funded_wallet(wallet_data(vec![AssetSchema::Nia]), 1).await;
    assert!(matches!(
        e.save_new_asset(e_on.clone(), &tampered_bytes, &txid).await,
        Err(Error::ConsignmentValidationFailed { .. })
    ));
    assert!(nia_listed(&e).is_empty());
    assert!(e.rgb_contract_ids().unwrap().is_empty());

    e.save_new_asset(e_on.clone(), &bytes, &txid).await.unwrap();
    let receive = blind_rcv(
        &mut e,
        &e_on,
        Some(asset_id.clone()),
        Assignment::Fungible(50),
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap();
    let invoice = Invoice::new(receive.invoice).unwrap();
    assert_eq!(invoice.invoice_data().asset_id, Some(asset_id));
}

#[wasm_bindgen_test]
async fn import_contract_reject_list() {
    let url = format!("{REGTEST_HELPER_URL}/reject-list/test/import");
    serve_reject_list(&url, "");
    let schemas = vec![AssetSchema::Nia, AssetSchema::Ifa];
    let (mut a, a_on) = funded_wallet(wallet_data(schemas.clone()), 2).await;
    let (mut c, c_on) = funded_wallet(wallet_data(schemas.clone()), 1).await;
    let issued = a
        .issue_asset_ifa(
            s("IFA"),
            s("Inflatable"),
            0,
            vec![1000],
            vec![100],
            Some(url.clone()),
            false,
            None,
            false,
        )
        .unwrap();
    let asset_id = issued.asset_id.clone();
    let expected = Some(RgbRejectListInfo::Url(url.clone()));
    assert_eq!(issued.reject_list, expected);
    let ifa = |w: &Wallet| w.list_assets(vec![AssetSchema::Ifa]).unwrap().ifa.unwrap();
    let reported = |w: &Wallet| {
        let metadata = w.get_asset_metadata(asset_id.clone()).unwrap();
        (
            ifa(w)[0].reject_list.clone(),
            metadata.reject_list,
            metadata.bridge_location,
        )
    };
    let (bytes, txid) = sent_consignment(&mut a, &a_on, &asset_id, &mut c, &c_on).await;
    assert_eq!(reported(&a), (expected.clone(), expected.clone(), None));

    let terminal = RgbTransfer::load(&bytes[..]).unwrap().terminal_ops();
    assert_eq!(terminal.len(), 1);
    let opid = *terminal.iter().next().unwrap();
    let rejected = Opout::new(opid, OS_ASSET, 0).to_string();
    serve_reject_list(&url, &rejected);
    let (d, d_on) = online_wallet(schemas).await;
    match d.save_new_asset(d_on.clone(), &bytes, &txid).await {
        Err(Error::ConsignmentRejected { opouts }) => assert_eq!(opouts, vec![rejected]),
        res => panic!("unexpected result: {res:?}"),
    }
    assert!(ifa(&d).is_empty());
    assert!(d.rgb_contract_ids().unwrap().is_empty());

    serve_reject_list(&url, "");
    d.save_new_asset(d_on, &bytes, &txid).await.unwrap();
    let listed = ifa(&d);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].asset_id, asset_id);
    assert_eq!(listed[0].balance, zero());
    assert_eq!(reported(&d), (expected.clone(), expected, None));
}

/// A wallet that issues with no indexer.
fn offline_issuer(schemas: Vec<AssetSchema>) -> Wallet {
    let (wd, keys) = wallet_data(schemas);
    let wallet = Wallet::new(wd, keys).unwrap();
    add_fake_txos(&wallet);
    wallet
}

fn media_digests(wallet: &Wallet, asset_id: &str) -> HashSet<String> {
    wallet
        .list_asset_media(asset_id.to_string())
        .unwrap()
        .into_iter()
        .map(|m| m.digest)
        .collect()
}

fn last_operation(wallet: &Wallet) -> i128 {
    wallet
        .database
        .get_backup_info()
        .unwrap()
        .map_or(0, |i| i.last_operation_timestamp.parse().unwrap())
}

fn assert_same_metadata(a: &Wallet, b: &Wallet, asset_id: &str) {
    assert_eq!(
        b.get_asset_metadata(asset_id.to_string()).unwrap(),
        a.get_asset_metadata(asset_id.to_string()).unwrap()
    );
}

fn assert_unknown(wallet: &Wallet, asset_id: &str) {
    assert!(matches!(
        wallet.get_asset_metadata(asset_id.to_string()),
        Err(Error::AssetNotFound { .. })
    ));
}

#[wasm_bindgen_test]
async fn import_contract_success() {
    let (mut a, a_on) = funded_wallet(wallet_data(vec![AssetSchema::Nia]), 2).await;
    let (mut b, b_on) = funded_wallet(wallet_data(vec![AssetSchema::Nia]), 1).await;
    let asset_id = a
        .issue_asset_nia(s("NIA"), s("Fixed"), 0, vec![1000])
        .unwrap()
        .asset_id;
    let exported = a.export_contract(asset_id.clone()).unwrap();

    assert_unknown(&b, &asset_id);
    let before = last_operation(&b);
    b.import_contract(&exported.contract, exported.media)
        .unwrap();
    assert!(last_operation(&b) > before);
    assert_same_metadata(&a, &b, &asset_id);
    assert_eq!(nia_listed(&b), vec![(asset_id.clone(), zero())]);
    assert!(
        b.list_transfers(AssetFilter::Id(asset_id.clone()), TransferScope::All)
            .unwrap()
            .is_empty()
    );

    let rid = blind_rcv(
        &mut b,
        &b_on,
        Some(asset_id.clone()),
        Assignment::Fungible(100),
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap()
    .recipient_id;
    send_recipients(
        &mut a,
        &a_on,
        &asset_id,
        vec![recipient(&rid, 100, None, PROXY_URL)],
    )
    .await;
    settle_both(&mut a, &a_on, &mut b, &b_on).await;
    assert_eq!(transfer_of(&b, &rid).status, TransferStatus::Settled);
    assert_eq!(
        b.get_asset_balance(asset_id).unwrap(),
        Balance {
            settled: 100,
            future: 100,
            spendable: 100,
        }
    );
}

#[wasm_bindgen_test]
async fn import_contract_all_schemas() {
    let schemas = vec![AssetSchema::Nia, AssetSchema::Ifa, AssetSchema::Bfa];
    let a = offline_issuer(schemas.clone());
    let (wd, keys) = wallet_data(schemas);
    let mut b = Wallet::new(wd, keys).unwrap();
    for schema in AssetSchema::VALUES {
        // an exhaustive match: a new schema does not compile until it is added here
        let asset_id = match schema {
            AssetSchema::Nia => {
                a.issue_asset_nia(s("NIA"), s("Nia"), 0, vec![100])
                    .unwrap()
                    .asset_id
            }
            AssetSchema::Ifa => {
                a.issue_asset_ifa(
                    s("IFA"),
                    s("Ifa"),
                    0,
                    vec![100],
                    vec![],
                    None,
                    false,
                    None,
                    false,
                )
                .unwrap()
                .asset_id
            }
            AssetSchema::Bfa => {
                a.issue_asset_bfa(
                    s("BFA"),
                    s("Bfa"),
                    0,
                    1,
                    31337,
                    s("0x5FbDB2315678afecb367f032d93F642f64180aa3"),
                    None,
                    false,
                    None,
                    false,
                )
                .unwrap()
                .asset_id
            }
        };
        let exported = a.export_contract(asset_id.clone()).unwrap();
        b.import_contract(&exported.contract, exported.media)
            .unwrap();
        assert_same_metadata(&a, &b, &asset_id);
    }
}

#[wasm_bindgen_test]
async fn import_contract_media_success() {
    let mut a = offline_issuer(vec![AssetSchema::Nia]);
    let digest = set_media(&mut a, b"nia media");
    let x = a
        .issue_asset_nia(s("X"), s("X"), 0, vec![100])
        .unwrap()
        .asset_id;
    set_media(&mut a, b"nia media");
    let y = a
        .issue_asset_nia(s("Y"), s("Y"), 0, vec![100])
        .unwrap()
        .asset_id;
    let x_contract = a.export_contract(x.clone()).unwrap().contract;
    let y_contract = a.export_contract(y.clone()).unwrap().contract;
    let (wd, keys) = wallet_data(vec![AssetSchema::Nia]);
    let mut b = Wallet::new(wd, keys).unwrap();

    // without its media, the import is refused and no media stays
    for media in [vec![], vec![b"other".to_vec()]] {
        let before = state(&b);
        assert!(matches!(
            b.import_contract(&x_contract, media),
            Err(Error::InvalidAttachments { .. })
        ));
        assert_eq!(state(&b), before);
        assert_unknown(&b, &x);
    }
    assert_no_partial_change(&mut b, "import_contract", 0, async |w| {
        w.import_contract(&x_contract, vec![b"nia media".to_vec()])
    })
    .await;

    // the wallet ignores the extra blob
    b.import_contract(&x_contract, vec![b"extra".to_vec(), b"nia media".to_vec()])
        .unwrap();
    assert_eq!(b.media[&digest], b"nia media");
    assert!(!b.media.contains_key(&hex::encode(sha256_digest(b"extra"))));
    assert_eq!(media_digests(&b, &x), HashSet::from([digest.clone()]));
    assert_same_metadata(&a, &b, &x);

    // media already present is not needed
    b.import_contract(&y_contract, vec![]).unwrap();
    assert_eq!(media_digests(&b, &y), HashSet::from([digest]));
}

#[wasm_bindgen_test]
async fn import_contract_fail() {
    let mut a = offline_issuer(vec![AssetSchema::Nia]);
    let asset_id = a
        .issue_asset_nia(s("NIA"), s("Nia"), 0, vec![100])
        .unwrap()
        .asset_id;
    let nia = a.export_contract(asset_id.clone()).unwrap().contract;
    let transfer_bytes = include_bytes!("../../tests/fixtures/inspection/send.rgbc");
    let (wd, keys) = wallet_data(vec![AssetSchema::Nia]);
    let mut b = Wallet::new(wd.clone(), keys).unwrap();
    // a refused import changes nothing
    let refused = |w: &mut Wallet, bytes: &[u8]| {
        let before = state(w);
        let res = w.import_contract(bytes, vec![]);
        assert_eq!(state(w), before);
        res.unwrap_err()
    };

    let too_large = vec![0u8; MAX_CONTRACT_SIZE as usize + 1];
    assert!(matches!(
        refused(&mut b, &too_large),
        Error::FileTooLarge { max_size } if max_size == MAX_CONTRACT_SIZE
    ));
    assert_unknown(&b, &asset_id);
    assert_eq!(refused(&mut b, b"not a contract"), Error::InvalidContract);
    assert_eq!(refused(&mut b, transfer_bytes), Error::InvalidContract);
    assert_unknown(&b, &asset_id);

    let mut tampered = Contract::load(&nia[..]).unwrap();
    tampered.genesis.globals = Default::default();
    let tampered_id = tampered.genesis.contract_id().to_string();
    let mut tampered_bytes = vec![];
    tampered.save(&mut tampered_bytes).unwrap();
    assert!(matches!(
        refused(&mut b, &tampered_bytes),
        Error::ConsignmentValidationFailed { .. }
    ));
    assert_unknown(&b, &tampered_id);

    assert_eq!(
        refused(&mut a, &nia),
        Error::AssetAlreadyKnown {
            asset_id: asset_id.clone()
        }
    );
    b.import_contract(&nia, vec![]).unwrap();
    assert_eq!(
        refused(&mut b, &nia),
        Error::AssetAlreadyKnown {
            asset_id: asset_id.clone()
        }
    );

    let (wd_ifa, keys_ifa) = wallet_data(vec![AssetSchema::Ifa]);
    let mut c = Wallet::new(wd_ifa, keys_ifa).unwrap();
    assert_eq!(
        refused(&mut c, &nia),
        Error::UnsupportedSchema {
            asset_schema: AssetSchema::Nia
        }
    );
    assert_unknown(&c, &asset_id);

    let testnet_keys = generate_keys(BitcoinNetwork::Testnet, WitnessVersion::Taproot);
    let mut d = Wallet::new(
        WalletData {
            bitcoin_network: BitcoinNetwork::Testnet,
            ..wd
        },
        SinglesigKeys::from_keys(&testnet_keys, None),
    )
    .unwrap();
    assert!(matches!(
        refused(&mut d, &nia),
        Error::ConsignmentValidationFailed { .. }
    ));
    assert_unknown(&d, &asset_id);
}

#[wasm_bindgen_test]
async fn import_contract_ifa_inflated() {
    let (mut a, a_on) = funded_wallet(wallet_data(vec![AssetSchema::Ifa]), 3).await;
    let asset_id = a
        .issue_asset_ifa(
            s("IFA"),
            s("Inflatable"),
            0,
            vec![1000],
            vec![100],
            None,
            false,
            None,
            false,
        )
        .unwrap()
        .asset_id;
    inflate_and_settle(&mut a, &a_on, &asset_id, 100).await;
    let supply = |w: &Wallet| {
        let metadata = w.get_asset_metadata(asset_id.clone()).unwrap();
        (metadata.initial_supply, metadata.known_circulating_supply)
    };
    assert_eq!(supply(&a), (1000, 1100));

    let (mut b, b_on) = funded_wallet(wallet_data(vec![AssetSchema::Ifa]), 1).await;
    let exported = a.export_contract(asset_id.clone()).unwrap();
    b.import_contract(&exported.contract, exported.media)
        .unwrap();
    assert_eq!(supply(&b), (1000, 1000));

    send_and_settle(&mut a, &a_on, &mut b, &b_on, &asset_id, 1001).await;
    assert_eq!(supply(&b), (1000, 1100));
}

#[wasm_bindgen_test]
async fn import_contract_persisted() {
    let mut a = offline_issuer(vec![AssetSchema::Nia]);
    let digest = set_media(&mut a, b"nia media");
    let asset_id = a
        .issue_asset_nia(s("NIA"), s("Nia"), 0, vec![100])
        .unwrap()
        .asset_id;
    let exported = a.export_contract(asset_id.clone()).unwrap();
    let (wd, keys) = wallet_data(vec![AssetSchema::Nia]);
    let mut b = Wallet::new(wd.clone(), keys.clone()).unwrap();
    b.import_contract(&exported.contract, exported.media)
        .unwrap();
    b.flush().await.unwrap();
    drop(b);

    let b = Wallet::restore(wd, keys).await.unwrap();
    assert_same_metadata(&a, &b, &asset_id);
    assert_eq!(media_digests(&b, &asset_id), HashSet::from([digest]));
    assert_eq!(
        b.export_contract(asset_id).unwrap().media,
        vec![b"nia media".to_vec()]
    );
}

#[wasm_bindgen_test]
async fn import_contract_after_failed_save() {
    let a = offline_issuer(vec![AssetSchema::Nia]);
    let asset_id = a
        .issue_asset_nia(s("NIA"), s("Nia"), 0, vec![100])
        .unwrap()
        .asset_id;
    let exported = a.export_contract(asset_id.clone()).unwrap();
    let (wd, keys) = wallet_data(vec![AssetSchema::Nia]);
    let mut b = Wallet::new(wd.clone(), keys.clone()).unwrap();
    b.flush().await.unwrap();
    let key = b.idb_key();
    let mut own = load_snapshot(&key).await.unwrap().unwrap();
    // another wallet saves, so IndexedDB refuses the writes of b
    Wallet::restore(wd.clone(), keys.clone())
        .await
        .unwrap()
        .flush()
        .await
        .unwrap();
    assert!(matches!(b.flush().await, Err(Error::Persistence { .. })));

    assert!(matches!(
        b.import_contract(&exported.contract, exported.media),
        Err(Error::Persistence { .. })
    ));
    assert_same_metadata(&a, &b, &asset_id);

    // put back a snapshot of b, so its next save commits
    let stored = load_snapshot(&key).await.unwrap().unwrap();
    own.sequence = stored.sequence + 1;
    save_snapshot(&key, &own, Some((stored.writer, stored.sequence)))
        .await
        .unwrap();
    b.flush().await.unwrap();
    assert_eq!(b.last_save_error(), None);
    drop(b);

    let b = Wallet::restore(wd, keys).await.unwrap();
    assert_same_metadata(&a, &b, &asset_id);
}
