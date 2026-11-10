use wasm_bindgen_test::*;

use super::online::{
    MOCK_MAX_UTXO_ASSET_ALLOCATIONS, MOCK_SPAM_HIDDEN_SPENDS, MOCK_SPAM_RECIPIENT_ASSIGNMENTS,
};
use super::test_bfa::utils::*;
use super::test_bfa::{funded_wallet, s, send_and_settle, settle_both, wallet_data};
use crate::wallet::{AssetFilter, Balance, Online, RgbAllocation, Transfer, Wallet};
use crate::{AssetSchema, Assignment, Error, TransferKind, TransferStatus};
use bdk_wallet::bitcoin::psbt::Psbt;
use std::collections::HashMap;
use std::str::FromStr;

const AMOUNT: u64 = 1000;

/// Reset the mocks when a test ends, as all lib tests share one thread.
struct ResetMocks;

impl Drop for ResetMocks {
    fn drop(&mut self) {
        MOCK_SPAM_RECIPIENT_ASSIGNMENTS.set(None);
        MOCK_SPAM_HIDDEN_SPENDS.set(None);
        MOCK_MAX_UTXO_ASSET_ALLOCATIONS.set(None);
    }
}

async fn nia_wallet(utxos: u8) -> (Wallet, Online) {
    funded_wallet(wallet_data(vec![AssetSchema::Nia]), utxos).await
}

fn issue(wallet: &Wallet, amounts: Vec<u64>) -> String {
    wallet
        .issue_asset_nia(s("TST"), s("Test"), 0, amounts)
        .unwrap()
        .asset_id
}

async fn invoice(
    wallet: &mut Wallet,
    online: &Online,
    assignment: Assignment,
    urls: &[&str],
) -> String {
    blind_rcv(
        wallet,
        online,
        None,
        assignment,
        default_rcv_expiration(),
        urls.iter().map(|u| transport_endpoint(u)).collect(),
    )
    .await
    .unwrap()
    .recipient_id
}

/// Begin a send of `amount` to `rid` through `url` and sign it. Return the PSBT and its txid.
async fn begin(
    wallet: &mut Wallet,
    online: &Online,
    asset_id: &str,
    rid: &str,
    amount: u64,
    url: &str,
    donation: bool,
) -> (String, String) {
    let map = HashMap::from([(
        asset_id.to_string(),
        vec![recipient(rid, amount, None, url)],
    )]);
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
    (signed, txid)
}

async fn end(wallet: &mut Wallet, online: &Online, signed: String) {
    wallet
        .send_end(online.clone(), signed, false)
        .await
        .unwrap();
}

async fn send_to(
    wallet: &mut Wallet,
    online: &Online,
    asset_id: &str,
    rid: &str,
    amount: u64,
    url: &str,
    donation: bool,
) -> String {
    let (signed, txid) = begin(wallet, online, asset_id, rid, amount, url, donation).await;
    end(wallet, online, signed).await;
    txid
}

fn sent(wallet: &Wallet, txid: &str) -> Transfer {
    wallet
        .list_transfers(AssetFilter::Any, None)
        .unwrap()
        .into_iter()
        .find(|t| t.kind == TransferKind::Send && t.txid.as_deref() == Some(txid))
        .unwrap()
}

/// Send to `rid` and check that both sides fail the transfer.
async fn send_refused(
    sender: &mut Wallet,
    sender_online: &Online,
    receiver: &mut Wallet,
    receiver_online: &Online,
    asset_id: &str,
    rid: &str,
    amount: u64,
) {
    let txid = send_to(
        sender,
        sender_online,
        asset_id,
        rid,
        amount,
        PROXY_URL,
        false,
    )
    .await;
    refresh(receiver, receiver_online).await;
    assert_eq!(transfer_of(receiver, rid).status, TransferStatus::Failed);
    refresh(sender, sender_online).await;
    assert_eq!(sent(sender, &txid).status, TransferStatus::Failed);
}

/// The allocations of the only colored unspent of `wallet`.
fn colored_allocations(wallet: &mut Wallet) -> Vec<RgbAllocation> {
    let mut colored: Vec<_> = wallet
        .list_unspents(None, false, true)
        .unwrap()
        .into_iter()
        .filter(|u| !u.rgb_allocations.is_empty())
        .collect();
    assert_eq!(colored.len(), 1);
    colored.remove(0).rgb_allocations
}

fn balance(wallet: &Wallet, asset_id: &str) -> Balance {
    wallet.get_asset_balance(asset_id.to_string()).unwrap()
}

// a consignment that does not pay the recipient is refused
#[wasm_bindgen_test]
async fn recipient_refuses_consignment_not_paying_it() {
    let _reset = ResetMocks;
    let (mut a, a_on) = nia_wallet(5).await;
    let (mut r1, r1_on) = nia_wallet(1).await;
    let (mut r2, r2_on) = nia_wallet(1).await;
    let asset_id = issue(&a, vec![AMOUNT]);

    let rid1 = invoice(&mut r1, &r1_on, Assignment::Fungible(66), &[PROXY_URL]).await;
    let (signed, tx1) = begin(&mut a, &a_on, &asset_id, &rid1, 66, PROXY_URL, false).await;
    let bytes_1 = a.send_consignment_bytes(&tx1, &asset_id).unwrap();
    end(&mut a, &a_on, signed).await;
    settle_both(&mut a, &a_on, &mut r1, &r1_on).await;

    // the second send posts the consignment of the first one
    let rid2 = invoice(&mut r2, &r2_on, Assignment::Any, &[PROXY_URL]).await;
    let (signed, tx2) = begin(&mut a, &a_on, &asset_id, &rid2, 66, PROXY_URL, false).await;
    a.set_send_consignment_bytes(&tx2, &asset_id, bytes_1)
        .unwrap();
    end(&mut a, &a_on, signed).await;
    refresh(&mut r2, &r2_on).await;

    assert_eq!(transfer_of(&r2, &rid2).status, TransferStatus::Failed);
    assert!(
        r2.list_unspents(None, false, true)
            .unwrap()
            .iter()
            .flat_map(|u| &u.rgb_allocations)
            .all(|a| a.asset_id.as_deref() != Some(asset_id.as_str()))
    );
}

// a consignment that pays the recipient in two bundles is refused
#[wasm_bindgen_test]
async fn receive_paying_two_destinations_refused() {
    let _reset = ResetMocks;
    let (mut a, a_on) = nia_wallet(5).await;
    let (mut r, r_on) = nia_wallet(1).await;
    let asset_id = issue(&a, vec![AMOUNT]);
    let rid = invoice(
        &mut r,
        &r_on,
        Assignment::Fungible(100),
        &[PROXY_URL, PROXY_B_URL],
    )
    .await;

    // the first payment goes to proxy B only and settles on the sender side
    let tx1 = send_to(&mut a, &a_on, &asset_id, &rid, 100, PROXY_B_URL, true).await;
    for _ in 0..3 {
        mine_blocks(1).await;
        wait_for_esplora_sync().await;
        a.sync(a_on.clone()).await.unwrap();
        refresh(&mut a, &a_on).await;
    }
    assert_eq!(sent(&a, &tx1).status, TransferStatus::Settled);

    // the second payment spends the change of the first one, so its consignment pays R twice
    send_to(&mut a, &a_on, &asset_id, &rid, 100, PROXY_URL, true).await;
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    r.sync(r_on.clone()).await.unwrap();
    refresh(&mut r, &r_on).await;

    assert_eq!(transfer_of(&r, &rid).status, TransferStatus::Failed);
    assert!(
        r.list_assets(vec![AssetSchema::Nia])
            .unwrap()
            .nia
            .unwrap()
            .is_empty()
    );
}

// a consignment that pays the recipient more than one allocation is refused
#[wasm_bindgen_test]
async fn receive_multiple_allocations_refused() {
    let _reset = ResetMocks;
    let (mut a, a_on) = nia_wallet(5).await;
    let (mut r, r_on) = nia_wallet(1).await;
    let (mut r2, r2_on) = nia_wallet(1).await;
    let asset_id = issue(&a, vec![AMOUNT]);
    send_and_settle(&mut a, &a_on, &mut r, &r_on, &asset_id, 100).await;

    let rid = invoice(&mut r, &r_on, Assignment::Fungible(50), &[PROXY_URL]).await;
    MOCK_SPAM_RECIPIENT_ASSIGNMENTS.set(Some((1, 1)));
    send_refused(&mut a, &a_on, &mut r, &r_on, &asset_id, &rid, 50).await;
    assert!(MOCK_SPAM_RECIPIENT_ASSIGNMENTS.with_borrow(|m| m.is_none()));

    assert_eq!(balance(&a, &asset_id).spendable, 900);
    assert_eq!(balance(&r, &asset_id).spendable, 100);
    assert_eq!(colored_allocations(&mut r).len(), 1);
    send_and_settle(&mut r, &r_on, &mut r2, &r2_on, &asset_id, 100).await;
    assert_eq!(balance(&r2, &asset_id).settled, 100);
}

// extra allocations listed as spent by a transition the consignment does not carry are refused
#[wasm_bindgen_test]
async fn receive_hidden_multiple_allocations_refused() {
    let _reset = ResetMocks;
    let (mut a, a_on) = nia_wallet(5).await;
    let (mut r, r_on) = nia_wallet(1).await;
    let asset_id = issue(&a, vec![AMOUNT]);

    MOCK_SPAM_RECIPIENT_ASSIGNMENTS.set(Some((2, 1)));
    MOCK_SPAM_HIDDEN_SPENDS.set(Some(()));
    let rid = invoice(&mut r, &r_on, Assignment::Fungible(50), &[PROXY_URL]).await;
    send_refused(&mut a, &a_on, &mut r, &r_on, &asset_id, &rid, 50).await;
    assert!(MOCK_SPAM_RECIPIENT_ASSIGNMENTS.with_borrow(|m| m.is_none()));
    assert!(MOCK_SPAM_HIDDEN_SPENDS.with_borrow(|m| m.is_none()));

    assert_eq!(balance(&a, &asset_id).spendable, AMOUNT);
    assert!(matches!(
        r.get_asset_balance(asset_id),
        Err(Error::AssetNotFound { .. })
    ));
}

// a receive that puts more allocations on the UTXO than the limit is refused
#[wasm_bindgen_test]
async fn utxo_freeze_limit() {
    let _reset = ResetMocks;
    MOCK_MAX_UTXO_ASSET_ALLOCATIONS.set(Some(2));
    let (mut a, a_on) = nia_wallet(5).await;
    let (mut r, r_on) = nia_wallet(1).await;
    let (mut r2, r2_on) = nia_wallet(1).await;
    let asset_id = issue(&a, vec![AMOUNT]);
    send_and_settle(&mut a, &a_on, &mut r, &r_on, &asset_id, 100).await;
    send_and_settle(&mut a, &a_on, &mut r, &r_on, &asset_id, 100).await;
    assert_eq!(colored_allocations(&mut r).len(), 2);

    let rid = invoice(&mut r, &r_on, Assignment::Fungible(100), &[PROXY_URL]).await;
    send_refused(&mut a, &a_on, &mut r, &r_on, &asset_id, &rid, 100).await;
    assert_eq!(colored_allocations(&mut r).len(), 2);

    assert_eq!(balance(&r, &asset_id).spendable, 200);
    send_and_settle(&mut r, &r_on, &mut r2, &r2_on, &asset_id, 200).await;
    assert_eq!(balance(&r2, &asset_id).settled, 200);
}

// allocations of a pending receive count toward the limit
#[wasm_bindgen_test]
async fn utxo_freeze_concurrent_receives() {
    let _reset = ResetMocks;
    MOCK_MAX_UTXO_ASSET_ALLOCATIONS.set(Some(1));
    let (mut a, a_on) = nia_wallet(5).await;
    let (mut r, r_on) = nia_wallet(1).await;
    let asset_id = issue(&a, vec![AMOUNT, AMOUNT]);

    let rid1 = invoice(&mut r, &r_on, Assignment::Fungible(50), &[PROXY_URL]).await;
    let rid2 = invoice(&mut r, &r_on, Assignment::Fungible(50), &[PROXY_URL]).await;
    assert_eq!(
        transfer_of(&r, &rid1).receive_utxo,
        transfer_of(&r, &rid2).receive_utxo
    );
    let tx1 = send_to(&mut a, &a_on, &asset_id, &rid1, 50, PROXY_URL, false).await;
    let tx2 = send_to(&mut a, &a_on, &asset_id, &rid2, 50, PROXY_URL, false).await;
    assert_ne!(tx1, tx2);

    refresh(&mut r, &r_on).await;
    let statuses = [transfer_of(&r, &rid1).status, transfer_of(&r, &rid2).status];
    assert!(statuses.contains(&TransferStatus::WaitingConfirmations));
    assert!(statuses.contains(&TransferStatus::Failed));
    let (accepted, refused) = if statuses[0] == TransferStatus::Failed {
        (tx2, tx1)
    } else {
        (tx1, tx2)
    };

    settle_both(&mut a, &a_on, &mut r, &r_on).await;
    assert_eq!(sent(&a, &accepted).status, TransferStatus::Settled);
    assert_eq!(sent(&a, &refused).status, TransferStatus::Failed);
    assert_eq!(colored_allocations(&mut r).len(), 1);
    assert_eq!(balance(&r, &asset_id).settled, 50);
}
