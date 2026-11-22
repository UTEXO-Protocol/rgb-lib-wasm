use std::collections::HashMap;
use wasm_bindgen_test::*;

use super::test_bfa::utils::*;
use super::test_bfa::{funded_wallet, s, send, settle_both, wallet_data};
use crate::no_wallet::RgbRejectListInfo;
use crate::wallet::offline::address_reuse_tests::assert_no_partial_change;
use crate::wallet::{Balance, Invoice, Online, Wallet, WalletData};
use crate::{AssetSchema, Assignment, Error, FileContent, OS_ASSET, RgbTransfer, TransferStatus};
use rgbstd::Opout;

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
        (
            ifa(w)[0].reject_list.clone(),
            w.get_asset_metadata(asset_id.clone()).unwrap().reject_list,
        )
    };
    let (bytes, txid) = sent_consignment(&mut a, &a_on, &asset_id, &mut c, &c_on).await;
    assert_eq!(reported(&a), (expected.clone(), expected.clone()));

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
    assert_eq!(reported(&d), (expected.clone(), expected));
}
