use wasm_bindgen_test::*;

use super::offline::{MOCK_ISSUE_MEDIA, MOCK_MAX_UTXO_ASSET_ALLOCATIONS};
use super::online::{
    MOCK_ACCEPT_MULTIPLE_ALLOCATIONS, MOCK_ACK_PENDING, MOCK_SPAM_HIDDEN_SPENDS,
    MOCK_SPAM_RECIPIENT_ASSIGNMENTS, RECEIVED_CONSIGNMENT_VALIDATIONS,
};
use super::test_bfa::utils::*;
use super::test_bfa::{funded_wallet, s, send, send_and_settle, settle_both, wallet_data};
use crate::utils::TRANSITION_BUILDERS_LOADED;
use crate::wallet::offline::address_reuse_tests::assert_no_partial_change;
use crate::wallet::rust_only::{AssetColoringInfo, ColoringInfo};
use crate::wallet::{
    AssetFilter, Balance, Online, Outpoint, ReceiveData, Recipient, RefreshFilter,
    RefreshTransferStatus, RgbAllocation, Transfer, TransferScope, Unspent, Wallet, WalletData,
};
use crate::{
    AssetSchema, Assignment, Attachment, Bytes32, Encryption, Error, MediaType, RgbTransfer,
    TransferKind, TransferStatus, sha256_digest,
};
use amplify::ByteArray;
use amplify::confinement::Confined;
use bdk_wallet::bitcoin::psbt::Psbt;
use bdk_wallet::bitcoin::psbt::raw::ProprietaryKey;
use bdk_wallet::bitcoin::{
    Amount as BdkAmount, FeeRate, OutPoint, ScriptBuf, TxMerkleNode, hashes::Hash,
};
use bdk_wallet::{KeychainKind, TxOrdering};
use rgbstd::ContractId;
use rgbstd::Txid as RgbTxid;
use rgbstd::containers::FileContent;
use rgbstd::persistence::RgbStore;
use rgbstd::validation::{ResolveWitness, WitnessStatus};
use rgbstd::vm::WitnessOrd;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::num::NonZeroU32;
use std::str::FromStr;

const AMOUNT: u64 = 1000;

/// Reset the mocks when a test ends, as all lib tests share one thread.
pub(super) struct ResetMocks;

impl Drop for ResetMocks {
    fn drop(&mut self) {
        MOCK_SPAM_RECIPIENT_ASSIGNMENTS.set(None);
        MOCK_SPAM_HIDDEN_SPENDS.set(None);
        MOCK_MAX_UTXO_ASSET_ALLOCATIONS.set(None);
        MOCK_ACCEPT_MULTIPLE_ALLOCATIONS.set(false);
        MOCK_ACK_PENDING.set(false);
        MOCK_ISSUE_MEDIA.set(None);
    }
}

pub(super) fn validations() -> usize {
    RECEIVED_CONSIGNMENT_VALIDATIONS.with_borrow(|n| *n)
}

/// A text attachment whose content is `media`.
pub(super) fn test_media() -> Attachment {
    Attachment {
        ty: MediaType::with("text/plain"),
        digest: Bytes32::from_byte_array(sha256_digest(b"media")),
    }
}

/// Make the proxy answer every `media.get` with the base64 of `content`.
pub(super) fn serve_media(content: &str) {
    clear_rpc_hooks();
    rpc_hook(
        PROXY_URL,
        "media.get",
        &format!("return {{ jsonrpc: '2.0', id: res.id, result: btoa('{content}') }};"),
    );
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

/// Begin the send of `map` and return its PSBT.
async fn begin_map(
    wallet: &mut Wallet,
    online: &Online,
    map: HashMap<String, Vec<Recipient>>,
    donation: bool,
) -> Result<String, Error> {
    wallet
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
        .map(|r| r.psbt)
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
    let unsigned = begin_map(wallet, online, map, donation).await.unwrap();
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
        .list_transfers(AssetFilter::AnyOrNone, TransferScope::All)
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

pub(super) fn witness_ords(wallet: &Wallet) -> BTreeMap<RgbTxid, WitnessOrd> {
    wallet
        .rgb_runtime()
        .unwrap()
        .stock
        .as_store()
        .all_witness_ords()
        .unwrap()
}

pub(super) fn witness_ord(wallet: &Wallet, txid: &str) -> Option<WitnessOrd> {
    witness_ords(wallet)
        .get(&RgbTxid::from_str(txid).unwrap())
        .copied()
}

pub(super) fn assert_no_rgb_data(psbt: &Psbt) {
    assert!(psbt.proprietary.is_empty());
    assert!(psbt.inputs.iter().all(|i| i.proprietary.is_empty()));
    assert!(psbt.outputs.iter().all(|o| o.proprietary.is_empty()));
}

/// A PSBT that spends the colored UTXO and pays 500 sat to the wallet.
fn psbt_to_color(wallet: &mut Wallet, op_return: bool) -> Psbt {
    let outpoint: OutPoint = wallet
        .list_unspents(None, false, true)
        .unwrap()
        .into_iter()
        .find(|u| !u.rgb_allocations.is_empty())
        .unwrap()
        .utxo
        .outpoint
        .into();
    let script = wallet
        .bdk_wallet
        .peek_address(KeychainKind::External, 0)
        .script_pubkey();
    let mut builder = wallet.bdk_wallet.build_tx();
    builder
        .add_utxo(outpoint)
        .unwrap()
        .manually_selected_only()
        .ordering(TxOrdering::Untouched)
        .fee_rate(FeeRate::from_sat_per_vb_u32(1));
    if op_return {
        builder.add_recipient(ScriptBuf::new_op_return([]), BdkAmount::ZERO);
    }
    builder.add_recipient(script, BdkAmount::from_sat(500));
    builder.finish().unwrap()
}

/// Send the whole asset to the 500 sat output, as color_psbt supports no change.
fn coloring_info(asset_id: &str) -> ColoringInfo {
    ColoringInfo {
        asset_info_map: HashMap::from([(
            ContractId::from_str(asset_id).unwrap(),
            AssetColoringInfo {
                output_map: HashMap::from([(0, AMOUNT)]),
                static_blinding: Some(777),
            },
        )]),
        static_blinding: Some(777),
        nonce: None,
    }
}

#[wasm_bindgen_test]
async fn color_psbt_strips_rgb_data() {
    let _reset = ResetMocks;
    let (mut a, _a_on) = nia_wallet(1).await;
    let asset_id = issue(&a, vec![AMOUNT]);
    let mut psbt = psbt_to_color(&mut a, false);

    a.color_psbt(&mut psbt, coloring_info(&asset_id)).unwrap();
    assert_no_rgb_data(&psbt);
}

#[wasm_bindgen_test]
async fn color_psbt_checks_the_psbt_parses_back() {
    let _reset = ResetMocks;
    let (mut a, _a_on) = nia_wallet(1).await;
    let asset_id = issue(&a, vec![AMOUNT]);
    // with an OP_RETURN output, color_psbt keeps the given PSBT
    let mut psbt = psbt_to_color(&mut a, true);
    // a non-RGB value over the 4 MB global map limit survives the strip
    psbt.proprietary.insert(
        ProprietaryKey {
            prefix: b"OTHER".to_vec(),
            subtype: 0,
            key: vec![],
        },
        vec![0; 5_000_000],
    );

    let err = a
        .color_psbt(&mut psbt, coloring_info(&asset_id))
        .unwrap_err();
    assert!(
        matches!(&err, Error::Internal { details } if details.starts_with("PSBT can't be parsed back")),
        "unexpected error: {err:?}"
    );
}

// a send enters the stock at its broadcast, and a dry run never does
#[wasm_bindgen_test]
async fn send_enters_the_stock_at_broadcast() {
    let _reset = ResetMocks;
    let (mut a, a_on) = nia_wallet(3).await;
    let (mut r, r_on) = nia_wallet(1).await;
    let asset_id = issue(&a, vec![AMOUNT]);
    let ords = witness_ords(&a);

    let rid = invoice(&mut r, &r_on, Assignment::Fungible(100), &[PROXY_URL]).await;
    let map = HashMap::from([(
        asset_id.clone(),
        vec![recipient(&rid, 100, None, PROXY_URL)],
    )]);
    a.send_begin(
        a_on.clone(),
        map,
        false,
        1,
        1,
        default_send_expiration(),
        Some(transport_endpoint(PROXY_URL)),
        None,
        true,
    )
    .await
    .unwrap();
    assert_eq!(witness_ords(&a), ords);

    let (signed, txid) = begin(&mut a, &a_on, &asset_id, &rid, 100, PROXY_URL, false).await;
    assert_no_rgb_data(&Psbt::from_str(&signed).unwrap());
    assert_eq!(witness_ords(&a), ords);
    assert!(a.send_consignment_bytes(&txid, &asset_id).is_none());
    end(&mut a, &a_on, signed).await;
    assert_eq!(witness_ord(&a, &txid), None);
    settle_both(&mut a, &a_on, &mut r, &r_on).await;
    assert!(witness_ord(&a, &txid).is_some());

    // a donation enters the stock at its end
    let rid = invoice(&mut r, &r_on, Assignment::Fungible(50), &[PROXY_URL]).await;
    let (signed, txid) = begin(&mut a, &a_on, &asset_id, &rid, 50, PROXY_URL, true).await;
    assert_eq!(witness_ord(&a, &txid), None);
    end(&mut a, &a_on, signed).await;
    assert!(witness_ord(&a, &txid).is_some());
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
    end(&mut a, &a_on, signed).await;
    let bytes_1 = a.send_consignment_bytes(&tx1, &asset_id).unwrap();
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
    let v = validations();
    send_refused(&mut a, &a_on, &mut r, &r_on, &asset_id, &rid, 50).await;
    // the consignment is validated before its payment is checked
    assert_eq!(validations(), v + 1);
    assert!(MOCK_SPAM_RECIPIENT_ASSIGNMENTS.with_borrow(|m| m.is_none()));

    assert_eq!(balance(&a, &asset_id).spendable, 900);
    assert_eq!(balance(&r, &asset_id).spendable, 100);
    assert_eq!(colored_allocations(&mut r).len(), 1);
    send_and_settle(&mut r, &r_on, &mut r2, &r2_on, &asset_id, 100).await;
    assert_eq!(balance(&r2, &asset_id).settled, 100);
}

// a consignment processed again replaces the colorings of the first processing
#[wasm_bindgen_test]
async fn receive_processed_again_counted_once() {
    let _reset = ResetMocks;
    let (mut a, a_on) = nia_wallet(5).await;
    let (mut r, r_on) = nia_wallet(1).await;
    r.wallet_data.max_allocations_per_utxo = 2;
    let asset_id = issue(&a, vec![AMOUNT]);
    send_and_settle(&mut a, &a_on, &mut r, &r_on, &asset_id, 100).await;

    let rid = invoice(&mut r, &r_on, Assignment::Fungible(100), &[PROXY_URL]).await;
    send_to(&mut a, &a_on, &asset_id, &rid, 100, PROXY_URL, false).await;
    MOCK_ACK_PENDING.set(true);
    let res = r.refresh(r_on.clone(), None, vec![], false).await.unwrap();
    assert!(res.values().all(|t| t.failure.is_none()), "{res:?}");
    assert!(!MOCK_ACK_PENDING.take());
    assert_eq!(
        transfer_of(&r, &rid).status,
        TransferStatus::WaitingCounterparty
    );

    settle_both(&mut a, &a_on, &mut r, &r_on).await;
    assert_eq!(transfer_of(&r, &rid).status, TransferStatus::Settled);
    assert_eq!(colored_allocations(&mut r).len(), 2);
    assert_eq!(balance(&r, &asset_id).settled, 200);
}

// a media that does not match the contract fails the refresh and a later refresh tries again
#[wasm_bindgen_test]
async fn receive_media_mismatch_retried() {
    let _reset = ResetMocks;
    let (mut a, a_on) = nia_wallet(5).await;
    let (mut r, r_on) = nia_wallet(1).await;
    MOCK_ISSUE_MEDIA.set(Some(test_media()));
    let asset_id = issue(&a, vec![AMOUNT]);
    assert!(MOCK_ISSUE_MEDIA.with_borrow(|m| m.is_none()));
    let rcv = blind_rcv_enc(
        &mut r,
        &r_on,
        None,
        Assignment::Fungible(100),
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
        Some(Encryption::Optional),
    )
    .await
    .unwrap();
    send_to(
        &mut a,
        &a_on,
        &asset_id,
        &rcv.recipient_id,
        100,
        PROXY_URL,
        false,
    )
    .await;

    for _ in 0..2 {
        serve_media("other");
        let res = r.refresh(r_on.clone(), None, vec![], false).await.unwrap();
        assert!(
            matches!(
                res[&rcv.batch_transfer_idx].failure,
                Some(Error::HashMismatch)
            ),
            "{res:?}"
        );
        assert_eq!(
            transfer_of(&r, &rcv.recipient_id).status,
            TransferStatus::WaitingCounterparty
        );
        assert!(r.database.check_asset_exists(asset_id.clone()).is_err());
        assert!(
            r.list_unspents(None, false, true)
                .unwrap()
                .iter()
                .all(|u| u.rgb_allocations.is_empty())
        );
    }

    serve_media("media");
    let res = r.refresh(r_on.clone(), None, vec![], false).await.unwrap();
    clear_rpc_hooks();
    let refreshed = &res[&rcv.batch_transfer_idx];
    assert!(refreshed.failure.is_none(), "{res:?}");
    assert_eq!(
        refreshed.updated_status,
        Some(TransferStatus::WaitingConfirmations)
    );
    assert!(
        r.database
            .check_asset_exists(asset_id.clone())
            .unwrap()
            .media_idx
            .is_some()
    );
    settle_both(&mut a, &a_on, &mut r, &r_on).await;
    assert_eq!(
        transfer_of(&r, &rcv.recipient_id).status,
        TransferStatus::Settled
    );
    assert_eq!(balance(&r, &asset_id).settled, 100);
}

// with no validated copy in memory, settlement validates the consignment again
#[wasm_bindgen_test]
async fn receive_settles_after_reload() {
    let (mut a, a_on) = nia_wallet(5).await;
    let (mut r, r_on) = nia_wallet(1).await;
    let asset_id = issue(&a, vec![AMOUNT]);
    let rid = invoice(&mut r, &r_on, Assignment::Fungible(100), &[PROXY_URL]).await;
    send_to(&mut a, &a_on, &asset_id, &rid, 100, PROXY_URL, false).await;
    refresh(&mut r, &r_on).await;
    assert_eq!(
        transfer_of(&r, &rid).status,
        TransferStatus::WaitingConfirmations
    );
    // A NIA consignment has no reject list, so no DAG is built.
    assert!(
        r.received_valid_consignments[&rid]
            .validation_status()
            .ops_dag
            .is_none()
    );

    let mut r = Wallet::restore(r.get_wallet_data()).await.unwrap();
    let r_on = r.go_online(go_online_options(false)).await.unwrap();
    let v = validations();
    settle_both(&mut a, &a_on, &mut r, &r_on).await;
    assert_eq!(transfer_of(&r, &rid).status, TransferStatus::Settled);
    assert_eq!(balance(&r, &asset_id).settled, 100);
    // with no copy in memory, settlement validates once
    assert_eq!(validations(), v + 1);
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

    // the UTXO is full, so the wallet does not pick it for an invoice
    assert!(matches!(
        blind_rcv(
            &mut r,
            &r_on,
            None,
            Assignment::Fungible(100),
            default_rcv_expiration(),
            vec![transport_endpoint(PROXY_URL)],
        )
        .await,
        Err(Error::InsufficientAllocationSlots)
    ));
    MOCK_MAX_UTXO_ASSET_ALLOCATIONS.set(Some(3));
    let rid = invoice(&mut r, &r_on, Assignment::Fungible(100), &[PROXY_URL]).await;
    MOCK_MAX_UTXO_ASSET_ALLOCATIONS.set(Some(2));
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
    MOCK_MAX_UTXO_ASSET_ALLOCATIONS.set(Some(2));
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

    MOCK_MAX_UTXO_ASSET_ALLOCATIONS.set(Some(1));
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

// a receive that puts more allocations of any asset on the UTXO than the wallet setting allows
// is refused
#[wasm_bindgen_test]
async fn receive_max_allocations_per_utxo() {
    let _reset = ResetMocks;
    let (mut a, a_on) = nia_wallet(5).await;
    let (mut r, r_on) = nia_wallet(1).await;
    r.wallet_data.max_allocations_per_utxo = 3;
    let asset_id = issue(&a, vec![AMOUNT]);
    let other = issue(&a, vec![AMOUNT + 1]);
    send_and_settle(&mut a, &a_on, &mut r, &r_on, &other, 100).await;
    send_and_settle(&mut a, &a_on, &mut r, &r_on, &asset_id, 100).await;

    let rid = invoice(&mut r, &r_on, Assignment::Fungible(100), &[PROXY_URL]).await;
    r.wallet_data.max_allocations_per_utxo = 2;
    send_refused(&mut a, &a_on, &mut r, &r_on, &asset_id, &rid, 100).await;
    assert_eq!(colored_allocations(&mut r).len(), 2);

    r.wallet_data.max_allocations_per_utxo = 3;
    send_and_settle(&mut a, &a_on, &mut r, &r_on, &asset_id, 100).await;
    assert_eq!(colored_allocations(&mut r).len(), 3);
    assert_eq!(balance(&r, &asset_id).settled, 200);
    assert_eq!(balance(&r, &other).settled, 100);
}

// each other unpaid invoice on the UTXO counts for one allocation
#[wasm_bindgen_test]
async fn receive_counts_unpaid_invoices() {
    let _reset = ResetMocks;
    let (mut a, a_on) = nia_wallet(5).await;
    let (mut r, r_on) = nia_wallet(1).await;
    r.wallet_data.max_allocations_per_utxo = 3;
    let asset_id = issue(&a, vec![AMOUNT]);
    send_and_settle(&mut a, &a_on, &mut r, &r_on, &asset_id, 100).await;

    let rid1 = invoice(&mut r, &r_on, Assignment::Fungible(100), &[PROXY_URL]).await;
    let rid2 = invoice(&mut r, &r_on, Assignment::Fungible(100), &[PROXY_URL]).await;
    assert_eq!(
        transfer_of(&r, &rid1).receive_utxo,
        transfer_of(&r, &rid2).receive_utxo
    );
    r.wallet_data.max_allocations_per_utxo = 2;
    send_refused(&mut a, &a_on, &mut r, &r_on, &asset_id, &rid2, 100).await;

    send_to(&mut a, &a_on, &asset_id, &rid1, 100, PROXY_URL, false).await;
    settle_both(&mut a, &a_on, &mut r, &r_on).await;
    assert_eq!(transfer_of(&r, &rid1).status, TransferStatus::Settled);
    assert_eq!(colored_allocations(&mut r).len(), 2);

    // the invoice also counts toward the allocations a bundle can spend
    r.wallet_data.max_allocations_per_utxo = 10;
    let rid3 = invoice(&mut r, &r_on, Assignment::Fungible(100), &[PROXY_URL]).await;
    let rid4 = invoice(&mut r, &r_on, Assignment::Fungible(100), &[PROXY_URL]).await;
    assert_eq!(
        transfer_of(&r, &rid3).receive_utxo,
        transfer_of(&r, &rid4).receive_utxo
    );
    MOCK_MAX_UTXO_ASSET_ALLOCATIONS.set(Some(3));
    send_refused(&mut a, &a_on, &mut r, &r_on, &asset_id, &rid4, 100).await;

    send_to(&mut a, &a_on, &asset_id, &rid3, 100, PROXY_URL, false).await;
    settle_both(&mut a, &a_on, &mut r, &r_on).await;
    assert_eq!(transfer_of(&r, &rid3).status, TransferStatus::Settled);
    assert_eq!(colored_allocations(&mut r).len(), 3);
}

/// The RGB unspent of `wallet` at `outpoint`.
fn unspent_at(wallet: &mut Wallet, outpoint: &Outpoint) -> Unspent {
    wallet
        .list_unspents(None, false, true)
        .unwrap()
        .into_iter()
        .find(|u| &u.utxo.outpoint == outpoint)
        .unwrap()
}

// a send skips a UTXO that takes its inputs past the allocations a bundle can spend
#[wasm_bindgen_test]
async fn utxo_allocations_limit_across_inputs() {
    let _reset = ResetMocks;
    MOCK_ACCEPT_MULTIPLE_ALLOCATIONS.set(true);
    MOCK_MAX_UTXO_ASSET_ALLOCATIONS.set(Some(10));
    let (mut a, a_on) = nia_wallet(5).await;
    let mut wd = wallet_data(vec![AssetSchema::Nia]);
    wd.max_allocations_per_utxo = u32::MAX;
    let (mut r, r_on) = funded_wallet(wd, 3).await;
    let (mut r2, r2_on) = nia_wallet(1).await;
    let asset_id = issue(&a, vec![33_000 + 34_000 + 50_000 + AMOUNT]);

    // A and B get 6 allocations each, C gets 1, each on its own UTXO
    let mut receive_utxos = vec![];
    for (amount, spam) in [
        (33_000, Some((5, 1))),
        (34_000, Some((5, 1))),
        (50_000, None),
    ] {
        let rid = invoice(&mut r, &r_on, Assignment::Fungible(amount), &[PROXY_URL]).await;
        receive_utxos.push(transfer_of(&r, &rid).receive_utxo.unwrap());
        MOCK_SPAM_RECIPIENT_ASSIGNMENTS.set(spam);
        send_to(&mut a, &a_on, &asset_id, &rid, amount, PROXY_URL, false).await;
        settle_both(&mut a, &a_on, &mut r, &r_on).await;
        assert!(MOCK_SPAM_RECIPIENT_ASSIGNMENTS.with_borrow(|m| m.is_none()));
    }
    assert_eq!(receive_utxos.iter().collect::<HashSet<_>>().len(), 3);
    let mut allocations: Vec<(u64, usize)> = r
        .list_unspents(None, false, true)
        .unwrap()
        .iter()
        .filter(|u| !u.rgb_allocations.is_empty())
        .map(|u| {
            let sum = u
                .rgb_allocations
                .iter()
                .map(|a| a.assignment.main_amount())
                .sum();
            (sum, u.rgb_allocations.len())
        })
        .collect();
    allocations.sort();
    assert_eq!(allocations, vec![(33_000, 6), (34_000, 6), (50_000, 1)]);

    // A comes first, B is skipped and C is selected
    send_and_settle(&mut r, &r_on, &mut r2, &r2_on, &asset_id, 40_000).await;
    assert_eq!(balance(&r2, &asset_id).settled, 40_000);
    assert_eq!(
        unspent_at(&mut r, &receive_utxos[1]).rgb_allocations.len(),
        6
    );
}

/// Send assets X and Y together from a wallet with two UTXOs. UTXO 1 holds X and 6 allocations
/// of Y. UTXO 2 holds 6 allocations of Y. Only UTXO 1 can be spent.
async fn utxo_allocations_limit_across_assets(x_first: bool) {
    let _reset = ResetMocks;
    MOCK_ACCEPT_MULTIPLE_ALLOCATIONS.set(true);
    MOCK_MAX_UTXO_ASSET_ALLOCATIONS.set(Some(10));
    let (mut a, a_on) = nia_wallet(5).await;
    let mut wd = wallet_data(vec![AssetSchema::Nia]);
    wd.max_allocations_per_utxo = u32::MAX;
    let (mut r, r_on) = funded_wallet(wd, 1).await;
    let (mut r2, r2_on) = nia_wallet(2).await;

    // the send selects inputs in asset ID order
    let mut ids = [issue(&a, vec![AMOUNT]), issue(&a, vec![AMOUNT + 1])];
    ids.sort();
    let [first, second] = ids;
    let (x, y) = if x_first {
        (first, second)
    } else {
        (second, first)
    };

    let rid = invoice(&mut r, &r_on, Assignment::Fungible(100), &[PROXY_URL]).await;
    send_to(&mut a, &a_on, &x, &rid, 100, PROXY_URL, false).await;
    settle_both(&mut a, &a_on, &mut r, &r_on).await;
    let rid = invoice(&mut r, &r_on, Assignment::Fungible(60), &[PROXY_URL]).await;
    MOCK_SPAM_RECIPIENT_ASSIGNMENTS.set(Some((5, 10)));
    send_to(&mut a, &a_on, &y, &rid, 60, PROXY_URL, false).await;
    settle_both(&mut a, &a_on, &mut r, &r_on).await;
    assert_eq!(colored_allocations(&mut r).len(), 7);

    let unsigned = r
        .create_utxos_begin(r_on.clone(), false, Some(1), None, 1, true, false)
        .await
        .unwrap();
    let signed = r.sign_psbt(unsigned, None).unwrap();
    r.create_utxos_end(r_on.clone(), signed).await.unwrap();
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    r.sync(r_on.clone()).await.unwrap();
    let rid = invoice(&mut r, &r_on, Assignment::Fungible(30), &[PROXY_URL]).await;
    let utxo_2 = transfer_of(&r, &rid).receive_utxo.unwrap();
    MOCK_SPAM_RECIPIENT_ASSIGNMENTS.set(Some((5, 5)));
    send_to(&mut a, &a_on, &y, &rid, 30, PROXY_URL, false).await;
    settle_both(&mut a, &a_on, &mut r, &r_on).await;
    assert!(MOCK_SPAM_RECIPIENT_ASSIGNMENTS.with_borrow(|m| m.is_none()));

    let rid_x = invoice(&mut r2, &r2_on, Assignment::Fungible(100), &[PROXY_URL]).await;
    // with X first, UTXO 2 alone covers Y but takes Y past the limit
    let y_amount = if x_first { 20 } else { 50 };
    let rid_y = invoice(
        &mut r2,
        &r2_on,
        Assignment::Fungible(y_amount),
        &[PROXY_URL],
    )
    .await;
    let map = HashMap::from([
        (x.clone(), vec![recipient(&rid_x, 100, None, PROXY_URL)]),
        (
            y.clone(),
            vec![recipient(&rid_y, y_amount, None, PROXY_URL)],
        ),
    ]);
    let unsigned = r
        .send_begin(
            r_on.clone(),
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
    let signed = r.sign_psbt(unsigned, None).unwrap();
    let txid = r.send_end(r_on.clone(), signed, false).await.unwrap().txid;
    settle_both(&mut r, &r_on, &mut r2, &r2_on).await;
    assert_eq!(sent(&r, &txid).status, TransferStatus::Settled);
    assert_eq!(balance(&r2, &x).settled, 100);
    assert_eq!(balance(&r2, &y).settled, y_amount);
    assert_eq!(unspent_at(&mut r, &utxo_2).rgb_allocations.len(), 6);
}

#[wasm_bindgen_test]
async fn utxo_allocations_limit_across_assets_x_first() {
    utxo_allocations_limit_across_assets(true).await;
}

#[wasm_bindgen_test]
async fn utxo_allocations_limit_across_assets_y_first() {
    utxo_allocations_limit_across_assets(false).await;
}

/// A send loads one transition builder for all the extra allocations of an asset.
#[wasm_bindgen_test]
async fn extra_allocations_share_one_builder() {
    let _reset = ResetMocks;
    MOCK_ACCEPT_MULTIPLE_ALLOCATIONS.set(true);
    let (mut a, a_on) = nia_wallet(5).await;
    let mut wd = wallet_data(vec![AssetSchema::Nia]);
    wd.max_allocations_per_utxo = u32::MAX;
    let (mut r, r_on) = funded_wallet(wd, 1).await;
    let (mut r2, r2_on) = nia_wallet(1).await;
    let x = issue(&a, vec![AMOUNT]);
    let y = issue(&a, vec![AMOUNT + 1]);

    let rid = invoice(&mut r, &r_on, Assignment::Fungible(100), &[PROXY_URL]).await;
    send_to(&mut a, &a_on, &x, &rid, 100, PROXY_URL, false).await;
    settle_both(&mut a, &a_on, &mut r, &r_on).await;
    let rid = invoice(&mut r, &r_on, Assignment::Fungible(60), &[PROXY_URL]).await;
    MOCK_SPAM_RECIPIENT_ASSIGNMENTS.set(Some((5, 10)));
    send_to(&mut a, &a_on, &y, &rid, 60, PROXY_URL, false).await;
    settle_both(&mut a, &a_on, &mut r, &r_on).await;
    // the only colored UTXO holds 1 allocation of X and 6 of Y
    assert_eq!(colored_allocations(&mut r).len(), 7);

    let rid = invoice(&mut r2, &r2_on, Assignment::Fungible(50), &[PROXY_URL]).await;
    TRANSITION_BUILDERS_LOADED.set(0);
    let (signed, txid) = begin(&mut r, &r_on, &x, &rid, 50, PROXY_URL, false).await;
    // the 6 allocations of Y are extra and share one builder
    assert_eq!(TRANSITION_BUILDERS_LOADED.get(), 1);
    end(&mut r, &r_on, signed).await;
    settle_both(&mut r, &r_on, &mut r2, &r2_on).await;

    assert_eq!(sent(&r, &txid).status, TransferStatus::Settled);
    assert_eq!(balance(&r2, &x).settled, 50);
    assert_eq!(balance(&r, &x).settled, 50);
    assert_eq!(balance(&r, &y).settled, 60);
    let y_allocations = r
        .list_unspents(None, false, true)
        .unwrap()
        .into_iter()
        .flat_map(|u| u.rgb_allocations)
        .filter(|a| a.asset_id == Some(y.clone()))
        .count();
    assert_eq!(y_allocations, 6);
}

/// A funded wallet with no UTXOs.
async fn noutxo_wallet(wd: WalletData) -> (Wallet, Online) {
    let mut wallet = Wallet::new(wd).unwrap();
    let online = wallet.go_online(go_online_options(false)).await.unwrap();
    fund_and_sync(&mut wallet, &online, "1.0").await;
    (wallet, online)
}

async fn create_utxos(wallet: &mut Wallet, online: &Online, num: u8, size: Option<u32>) {
    let unsigned = wallet
        .create_utxos_begin(online.clone(), false, Some(num), size, 1, true, false)
        .await
        .unwrap();
    let signed = wallet.sign_psbt(unsigned, None).unwrap();
    wallet
        .create_utxos_end(online.clone(), signed)
        .await
        .unwrap();
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();
}

fn utxo_num_allocations(wallet: &mut Wallet, outpoint: &Outpoint) -> Option<usize> {
    wallet
        .list_unspents(None, true, true)
        .unwrap()
        .into_iter()
        .find(|u| &u.utxo.outpoint == outpoint)
        .map(|u| u.rgb_allocations.len())
}

/// Begin a send of `amount` to `rid` and return the result.
async fn begin_result(
    wallet: &mut Wallet,
    online: &Online,
    asset_id: &str,
    rid: &str,
    amount: u64,
) -> Result<(), Error> {
    let map = HashMap::from([(
        asset_id.to_string(),
        vec![recipient(rid, amount, None, PROXY_URL)],
    )]);
    begin_map(wallet, online, map, false).await.map(|_| ())
}

// the RGB change does not take a UTXO past the limit with the slots of its unpaid invoices
#[wasm_bindgen_test]
async fn change_respects_invoice_slots() {
    let _reset = ResetMocks;
    let (mut a, a_on) = nia_wallet(5).await;
    let mut wd = wallet_data(vec![AssetSchema::Nia]);
    wd.max_allocations_per_utxo = 3;
    let (mut r, r_on) = noutxo_wallet(wd).await;
    let (mut r2, r2_on) = nia_wallet(1).await;
    // tiny UTXOs, so that a send at 1 sat/vB has no BTC change
    create_utxos(&mut r, &r_on, 2, Some(400)).await;
    let asset_id = issue(&a, vec![AMOUNT]);
    for _ in 0..3 {
        send_and_settle(&mut a, &a_on, &mut r, &r_on, &asset_id, 100).await;
    }

    // the invoice goes on the UTXO with 1 allocation (U), the send spends the other one
    let rid = invoice(&mut r, &r_on, Assignment::Any, &[PROXY_URL]).await;
    let utxo_u = transfer_of(&r, &rid).receive_utxo.unwrap();
    assert_eq!(utxo_num_allocations(&mut r, &utxo_u), Some(1));
    let rid2 = invoice(&mut r2, &r2_on, Assignment::Any, &[PROXY_URL]).await;
    let res = begin_result(&mut r, &r_on, &asset_id, &rid2, 1).await;
    assert!(
        matches!(res, Err(Error::InsufficientAllocationSlots)),
        "{res:?}"
    );

    // with another UTXO the RGB change goes there
    create_utxos(&mut r, &r_on, 1, None).await;
    let txid = send_to(&mut r, &r_on, &asset_id, &rid2, 1, PROXY_URL, false).await;
    let change_utxos = sent(&r, &txid).change_utxos;
    assert_eq!(change_utxos.len(), 1);
    assert_ne!(change_utxos[0], utxo_u);
    settle_both(&mut r, &r_on, &mut r2, &r2_on).await;
    assert_eq!(sent(&r, &txid).status, TransferStatus::Settled);

    // the invoice can still be paid
    send_to(&mut a, &a_on, &asset_id, &rid, 100, PROXY_URL, false).await;
    settle_both(&mut a, &a_on, &mut r, &r_on).await;
    assert_eq!(transfer_of(&r, &rid).status, TransferStatus::Settled);
    assert_eq!(utxo_num_allocations(&mut r, &utxo_u), Some(2));
}

// the RGB change does not take a UTXO past the allocations a bundle can spend
#[wasm_bindgen_test]
async fn change_utxo_allocations_limit() {
    let _reset = ResetMocks;
    MOCK_ACCEPT_MULTIPLE_ALLOCATIONS.set(true);
    MOCK_MAX_UTXO_ASSET_ALLOCATIONS.set(Some(10));
    let (mut a, a_on) = nia_wallet(5).await;
    let mut wd = wallet_data(vec![AssetSchema::Nia]);
    wd.max_allocations_per_utxo = u32::MAX;
    let (mut r, r_on) = noutxo_wallet(wd).await;
    let (mut r2, r2_on) = nia_wallet(1).await;
    // tiny UTXOs, so that a send at 1 sat/vB has no BTC change
    create_utxos(&mut r, &r_on, 2, Some(400)).await;
    let asset_id = issue(&a, vec![1_000_000 + 118]);

    // D gets 1 allocation, S gets 10
    let rid_d = invoice(&mut r, &r_on, Assignment::Any, &[PROXY_URL]).await;
    send_to(
        &mut a, &a_on, &asset_id, &rid_d, 1_000_000, PROXY_URL, false,
    )
    .await;
    settle_both(&mut a, &a_on, &mut r, &r_on).await;
    let rid_s = invoice(&mut r, &r_on, Assignment::Any, &[PROXY_URL]).await;
    MOCK_SPAM_RECIPIENT_ASSIGNMENTS.set(Some((9, 2)));
    send_to(&mut a, &a_on, &asset_id, &rid_s, 118, PROXY_URL, false).await;
    settle_both(&mut a, &a_on, &mut r, &r_on).await;
    assert!(MOCK_SPAM_RECIPIENT_ASSIGNMENTS.with_borrow(|m| m.is_none()));
    let utxo_d = transfer_of(&r, &rid_d).receive_utxo.unwrap();
    let utxo_s = transfer_of(&r, &rid_s).receive_utxo.unwrap();
    assert_ne!(utxo_d, utxo_s);
    assert_eq!(utxo_num_allocations(&mut r, &utxo_d), Some(1));
    assert_eq!(utxo_num_allocations(&mut r, &utxo_s), Some(10));

    // the send spends S and its 10 allocations go to the change, which D cannot take
    let rid2 = invoice(&mut r2, &r2_on, Assignment::Any, &[PROXY_URL]).await;
    let res = begin_result(&mut r, &r_on, &asset_id, &rid2, 1).await;
    assert!(
        matches!(res, Err(Error::InsufficientAllocationSlots)),
        "{res:?}"
    );

    // with another UTXO the RGB change goes there
    create_utxos(&mut r, &r_on, 1, None).await;
    let txid = send_to(&mut r, &r_on, &asset_id, &rid2, 1, PROXY_URL, false).await;
    let change_utxos = sent(&r, &txid).change_utxos;
    assert_eq!(change_utxos.len(), 1);
    let utxo_change = change_utxos[0].clone();
    assert_ne!(utxo_change, utxo_d);
    assert_ne!(utxo_change, utxo_s);
    settle_both(&mut r, &r_on, &mut r2, &r2_on).await;
    assert_eq!(sent(&r, &txid).status, TransferStatus::Settled);
    assert_eq!(utxo_num_allocations(&mut r, &utxo_d), Some(1));
    assert_eq!(utxo_num_allocations(&mut r, &utxo_change), Some(10));
}

// the RGB change goes to the BTC change output and to other UTXOs, as many as needed
#[wasm_bindgen_test]
async fn change_spread_over_utxos() {
    let _reset = ResetMocks;
    let (mut a, a_on) = nia_wallet(5).await;
    let mut wd = wallet_data(vec![AssetSchema::Nia]);
    wd.max_allocations_per_utxo = 4;
    let (mut r, r_on) = noutxo_wallet(wd).await;
    let (mut r2, r2_on) = nia_wallet(1).await;
    let ids: Vec<String> = (0..5).map(|_| issue(&a, vec![AMOUNT])).collect();

    // S gets 1 allocation of A, B, C and D
    create_utxos(&mut r, &r_on, 1, None).await;
    for asset_id in &ids[..4] {
        send_and_settle(&mut a, &a_on, &mut r, &r_on, asset_id, 100).await;
    }
    let utxo_s = r
        .list_unspents(None, false, true)
        .unwrap()
        .into_iter()
        .find(|u| !u.rgb_allocations.is_empty())
        .unwrap()
        .utxo
        .outpoint;
    assert_eq!(utxo_num_allocations(&mut r, &utxo_s), Some(4));

    // the 4 change allocations do not fit on the BTC change output alone
    r.wallet_data.max_allocations_per_utxo = 2;
    let rid2 = invoice(&mut r2, &r2_on, Assignment::Any, &[PROXY_URL]).await;
    let res = begin_result(&mut r, &r_on, &ids[0], &rid2, 10).await;
    assert!(
        matches!(res, Err(Error::InsufficientAllocationSlots)),
        "{res:?}"
    );
    r.wallet_data.max_allocations_per_utxo = 4;

    // U1 and U2 get 1 allocation of E each
    create_utxos(&mut r, &r_on, 2, None).await;
    let mut utxos_u = vec![];
    for _ in 0..2 {
        let rid = invoice(&mut r, &r_on, Assignment::Any, &[PROXY_URL]).await;
        utxos_u.push(transfer_of(&r, &rid).receive_utxo.unwrap());
        send_to(&mut a, &a_on, &ids[4], &rid, 100, PROXY_URL, false).await;
        settle_both(&mut a, &a_on, &mut r, &r_on).await;
    }
    assert_ne!(utxos_u[0], utxos_u[1]);
    assert!(!utxos_u.contains(&utxo_s));

    // the BTC change output gets 2 of the 4 change allocations, U1 and U2 get 1 each
    r.wallet_data.max_allocations_per_utxo = 2;
    let txid = send_to(&mut r, &r_on, &ids[0], &rid2, 10, PROXY_URL, false).await;
    let change_utxos = sent(&r, &txid).change_utxos;
    assert_eq!(change_utxos.len(), 1);
    assert_eq!(change_utxos[0].txid, txid);
    settle_both(&mut r, &r_on, &mut r2, &r2_on).await;
    assert_eq!(sent(&r, &txid).status, TransferStatus::Settled);
    assert_eq!(utxo_num_allocations(&mut r, &utxo_s), None);
    for utxo in [&change_utxos[0], &utxos_u[0], &utxos_u[1]] {
        assert_eq!(utxo_num_allocations(&mut r, utxo), Some(2));
    }
    assert_eq!(balance(&r, &ids[0]).spendable, 90);
    for asset_id in &ids[1..4] {
        assert_eq!(balance(&r, asset_id).spendable, 100);
    }
}

// each change allocation takes its own slot
#[wasm_bindgen_test]
async fn change_allocations_placed_separately() {
    let _reset = ResetMocks;
    let (mut a, a_on) =
        funded_wallet(wallet_data(vec![AssetSchema::Nia, AssetSchema::Ifa]), 5).await;
    let (mut r, r_on) =
        funded_wallet(wallet_data(vec![AssetSchema::Nia, AssetSchema::Ifa]), 1).await;
    let asset_id = a
        .issue_asset_ifa(
            s("IFA"),
            s("Inflatable"),
            0,
            vec![AMOUNT],
            vec![AMOUNT],
            None,
            false,
            None,
            false,
        )
        .unwrap()
        .asset_id;

    a.wallet_data.max_allocations_per_utxo = 1;
    let rid_f = invoice(&mut r, &r_on, Assignment::Any, &[PROXY_URL]).await;
    let rid_i = invoice(&mut r, &r_on, Assignment::Any, &[PROXY_URL]).await;
    let rec_f = recipient(&rid_f, 66, None, PROXY_URL);
    let rec_i = Recipient {
        assignment: Assignment::InflationRight(42),
        ..recipient(&rid_i, 42, None, PROXY_URL)
    };
    let map = HashMap::from([(asset_id.clone(), vec![rec_f, rec_i])]);
    let unsigned = begin_map(&mut a, &a_on, map, false).await.unwrap();
    let signed = a.sign_psbt(unsigned, None).unwrap();
    let txid = a.send_end(a_on.clone(), signed, false).await.unwrap().txid;
    settle_both(&mut a, &a_on, &mut r, &r_on).await;
    let t = sent(&a, &txid);
    assert_eq!(t.status, TransferStatus::Settled);

    // the BTC change output and a pre-existing UTXO get 1 allocation each
    assert_eq!(t.change_utxos.len(), 2);
    assert_eq!(t.change_utxos.iter().filter(|o| o.txid == txid).count(), 1);
    let unspents = a.list_unspents(None, false, true).unwrap();
    let mut change: Vec<Assignment> = t
        .change_utxos
        .iter()
        .flat_map(|o| {
            let unspent = unspents.iter().find(|u| &u.utxo.outpoint == o).unwrap();
            assert_eq!(unspent.rgb_allocations.len(), 1);
            unspent.rgb_allocations.iter().map(|a| a.assignment.clone())
        })
        .collect();
    change.sort();
    let mut expected = vec![
        Assignment::Fungible(AMOUNT - 66),
        Assignment::InflationRight(AMOUNT - 42),
    ];
    expected.sort();
    assert_eq!(change, expected);
}

pub(super) fn consignment(wallet: &Wallet, asset_id: &str, txid: &str) -> RgbTransfer {
    let bytes = &wallet.transfer_artifacts[txid].consignment_bytes[asset_id];
    RgbTransfer::load(&bytes[..]).unwrap()
}

pub(super) fn proven(consignment: &RgbTransfer) -> HashMap<String, bool> {
    consignment
        .bundles
        .iter()
        .map(|bw| (bw.witness_id().to_string(), bw.spv_proof.is_some()))
        .collect()
}

fn stored_proof(wallet: &Wallet, txid: &str) -> bool {
    let runtime = wallet.rgb_runtime().unwrap();
    let txid = RgbTxid::from_str(txid).unwrap();
    runtime
        .stock
        .as_store()
        .witness_spv_proof(txid)
        .unwrap()
        .is_some()
}

/// Stop refusing transaction lookups when a test ends.
struct AllowTxLookups;

impl Drop for AllowTxLookups {
    fn drop(&mut self) {
        refuse_tx_lookups(&[]);
    }
}

// a consignment carries the SPV proof of each mined history witness, and the receiver checks it
// without fetching the witness
#[wasm_bindgen_test]
async fn spv_proofs() {
    let _allow = AllowTxLookups;
    let (mut a, a_on) = nia_wallet(2).await;
    let (mut b, b_on) = nia_wallet(2).await;
    let (mut c, c_on) = nia_wallet(2).await;
    let asset_id = issue(&a, vec![AMOUNT]);

    let txid_1 = send(
        &mut a,
        &a_on,
        &mut b,
        &b_on,
        &asset_id,
        Assignment::Fungible(100),
    )
    .await;
    settle_both(&mut a, &a_on, &mut b, &b_on).await;

    // own send: A spends its change from txid_1
    let txid_2 = send(
        &mut a,
        &a_on,
        &mut c,
        &c_on,
        &asset_id,
        Assignment::Fungible(50),
    )
    .await;
    let ac = consignment(&a, &asset_id, &txid_2);
    let proofs = proven(&ac);
    assert!(proofs[&txid_1]);
    assert!(!proofs[&txid_2]);
    assert!(stored_proof(&a, &txid_1));

    // C settles the transfer with the lookups of txid_1 refused
    for _ in 0..3 {
        refuse_tx_lookups(&[&txid_1]);
        c.sync(c_on.clone()).await.unwrap();
        for (idx, refreshed) in c.refresh(c_on.clone(), None, vec![], false).await.unwrap() {
            assert!(
                refreshed.failure.is_none(),
                "transfer {idx}: {:?}",
                refreshed.failure
            );
        }
        assert_eq!(refused_tx_lookups(), 0);
        refuse_tx_lookups(&[]);
        a.sync(a_on.clone()).await.unwrap();
        a.refresh(a_on.clone(), None, vec![], false).await.unwrap();
        mine_blocks(1).await;
        wait_for_esplora_sync().await;
    }
    let received: Vec<Transfer> = c
        .list_transfers(AssetFilter::Id(asset_id.clone()), TransferScope::All)
        .unwrap()
        .into_iter()
        .filter(|t| t.kind == TransferKind::ReceiveBlind)
        .collect();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].status, TransferStatus::Settled);
    assert_eq!(balance(&c, &asset_id).settled, 50);
    assert!(stored_proof(&c, &txid_1));

    // a carried proof replaces the lookup of its witness
    let witness_1 = RgbTxid::from_str(&txid_1).unwrap();
    let height = ac
        .bundles
        .iter()
        .find(|bw| bw.witness_id() == witness_1)
        .and_then(|bw| bw.spv_proof.as_ref())
        .unwrap()
        .block_height;
    refuse_tx_lookups(&[&txid_1]);
    let resolver = c.prefetch_consignment_witnesses(&ac).await.unwrap();
    refuse_tx_lookups(&[]);
    assert!(resolver.resolve_witness(witness_1).is_err());
    assert!(resolver.get_block_header(height).is_ok());

    // a proof that does not verify makes the receiver fetch the witness
    let mut broken = ac.clone();
    for bw in broken.bundles.iter_mut() {
        if bw.witness_id() == witness_1 {
            let proof = bw.spv_proof.as_mut().unwrap();
            let mut nodes: Vec<TxMerkleNode> = proof.merkle.iter().copied().collect();
            assert!(!nodes.is_empty());
            nodes[0] = TxMerkleNode::from_byte_array([0xab; 32]);
            proof.merkle = Confined::try_from(nodes).unwrap();
        }
    }
    let resolver = c.prefetch_consignment_witnesses(&broken).await.unwrap();
    assert!(matches!(
        resolver.resolve_witness(witness_1),
        Ok(WitnessStatus::Resolved(_, WitnessOrd::Mined(_)))
    ));

    // a proof above the chain tip has no header, so the receiver fetches the witness
    let mut above_tip = ac.clone();
    for bw in above_tip.bundles.iter_mut() {
        if bw.witness_id() == witness_1 {
            bw.spv_proof.as_mut().unwrap().block_height = NonZeroU32::MAX;
        }
    }
    let resolver = c.prefetch_consignment_witnesses(&above_tip).await.unwrap();
    assert!(resolver.get_block_header(NonZeroU32::MAX).is_err());
    assert!(matches!(
        resolver.resolve_witness(witness_1),
        Ok(WitnessStatus::Resolved(_, WitnessOrd::Mined(_)))
    ));
}

#[wasm_bindgen_test]
async fn send_and_refresh_are_atomic() {
    let (mut a, a_on) = nia_wallet(3).await;
    let (mut b, b_on) = nia_wallet(1).await;
    assert_no_partial_change(&mut b, "go_online", 0, async |w| {
        w.go_online(go_online_options(false)).await
    })
    .await;
    assert_no_partial_change(&mut a, "issue_asset_nia", 0, async |w| {
        w.issue_asset_nia(s("NIA"), s("Fixed"), 0, vec![1000, 1000])
    })
    .await;
    let asset_id = a
        .issue_asset_nia(s("NIA"), s("Fixed"), 0, vec![1000, 1000])
        .unwrap()
        .asset_id;
    let endpoints = vec![transport_endpoint(PROXY_URL)];
    assert_no_partial_change(&mut b, "blind_receive", 0, async |w| {
        let exp = default_rcv_expiration();
        blind_rcv(w, &b_on, None, Assignment::Any, exp, endpoints.clone()).await
    })
    .await;
    assert_no_partial_change(&mut b, "witness_receive", 0, async |w| {
        let exp = default_rcv_expiration();
        witness_rcv(w, &b_on, None, Assignment::Any, exp, endpoints.clone()).await
    })
    .await;
    let exp = default_rcv_expiration();
    let blind = blind_rcv(&mut b, &b_on, None, Assignment::Any, exp, endpoints.clone())
        .await
        .unwrap()
        .recipient_id;
    let witness = witness_rcv(&mut b, &b_on, None, Assignment::Any, exp, endpoints)
        .await
        .unwrap()
        .recipient_id;

    let send_begin = async |w: &mut Wallet, recipient: Recipient| {
        w.send_begin(
            a_on.clone(),
            HashMap::from([(asset_id.clone(), vec![recipient])]),
            false,
            1,
            1,
            default_send_expiration(),
            Some(transport_endpoint(PROXY_URL)),
            None,
            false,
        )
        .await
    };
    assert_no_partial_change(&mut a, "send_begin", 0, async |w| {
        send_begin(w, recipient(&blind, 100, None, PROXY_URL)).await
    })
    .await;
    let (signed, txid) = begin(&mut a, &a_on, &asset_id, &blind, 100, PROXY_URL, false).await;
    end(&mut a, &a_on, signed).await;
    b.refresh(b_on.clone(), None, vec![], false).await.unwrap();
    let txids = assert_no_partial_change(&mut a, "refresh", 1, async |w| {
        w.refresh(a_on.clone(), None, vec![], true).await
    })
    .await;
    assert_eq!(txids, vec![txid]);
    a.refresh(a_on.clone(), None, vec![], true).await.unwrap();
    assert_eq!(
        transfer_of(&a, &blind).status,
        TransferStatus::WaitingConfirmations
    );

    let unsigned = send_begin(&mut a, recipient(&witness, 100, Some(1000), PROXY_URL))
        .await
        .unwrap()
        .psbt;
    let signed = a.sign_psbt(unsigned, None).unwrap();
    assert_no_partial_change(&mut a, "send_end", 0, async |w| {
        w.send_end(a_on.clone(), signed.clone(), false).await
    })
    .await;
}

const WAITING_SAFE_HEIGHT: RefreshFilter = RefreshFilter {
    status: RefreshTransferStatus::WaitingSafeHeight,
    incoming: true,
};

/// Receive a send whose history TX has 1 confirmation, with `min_confirmations` 2.
async fn park_on_safe_height(
    donation: bool,
) -> (Wallet, Online, Wallet, Online, String, ReceiveData, String) {
    let (mut a, a_on) = nia_wallet(3).await;
    let (mut r, r_on) = nia_wallet(2).await;
    let asset_id = issue(&a, vec![AMOUNT]);

    let rid1 = invoice(&mut r, &r_on, Assignment::Fungible(66), &[PROXY_URL]).await;
    let txid1 = send_to(&mut a, &a_on, &asset_id, &rid1, 66, PROXY_URL, false).await;
    refresh(&mut r, &r_on).await;
    refresh(&mut a, &a_on).await;
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    refresh(&mut r, &r_on).await;
    refresh(&mut a, &a_on).await;
    assert_eq!(transfer_of(&r, &rid1).status, TransferStatus::Settled);
    assert_eq!(sent(&a, &txid1).status, TransferStatus::Settled);

    let rcv2 = r
        .blind_receive(
            Some(r_on.clone()),
            None,
            Assignment::Any,
            default_rcv_expiration(),
            vec![transport_endpoint(PROXY_URL)],
            2,
            None,
        )
        .await
        .unwrap();
    register_receive(&rcv2, Encryption::Required);
    let txid2 = send_to(
        &mut a,
        &a_on,
        &asset_id,
        &rcv2.recipient_id,
        33,
        PROXY_URL,
        donation,
    )
    .await;
    let res = r.refresh(r_on.clone(), None, vec![], false).await.unwrap();
    assert_eq!(
        res[&rcv2.batch_transfer_idx].updated_status,
        Some(TransferStatus::WaitingSafeHeight)
    );
    assert_eq!(
        transfer_of(&r, &rcv2.recipient_id).status,
        TransferStatus::WaitingSafeHeight
    );
    (a, a_on, r, r_on, asset_id, rcv2, txid2)
}

// a receive with unsafe history waits for the safe height before the ACK
#[wasm_bindgen_test]
async fn unsafe_history_waits_for_safe_height() {
    let _reset = ResetMocks;
    let (mut a, a_on, mut r, r_on, asset_id, rcv2, txid2) = park_on_safe_height(false).await;
    refresh(&mut a, &a_on).await;
    assert_eq!(sent(&a, &txid2).status, TransferStatus::WaitingCounterparty);

    let res = r
        .refresh(r_on.clone(), None, vec![WAITING_SAFE_HEIGHT], false)
        .await
        .unwrap();
    assert!(
        res.values()
            .all(|t| t.updated_status.is_none() && t.failure.is_none())
    );
    assert_eq!(
        transfer_of(&r, &rcv2.recipient_id).status,
        TransferStatus::WaitingSafeHeight
    );

    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    let res = r
        .refresh(r_on.clone(), None, vec![WAITING_SAFE_HEIGHT], false)
        .await
        .unwrap();
    let refreshed = &res[&rcv2.batch_transfer_idx];
    assert!(matches!(
        refreshed.updated_status,
        Some(TransferStatus::WaitingBroadcast | TransferStatus::WaitingConfirmations)
    ));
    assert!(refreshed.failure.is_none());

    settle_both(&mut a, &a_on, &mut r, &r_on).await;
    assert_eq!(
        transfer_of(&r, &rcv2.recipient_id).status,
        TransferStatus::Settled
    );
    assert_eq!(sent(&a, &txid2).status, TransferStatus::Settled);
    assert_eq!(balance(&r, &asset_id).settled, 99);
}

// a donation that waits for the safe height is broadcast when the sender proxy has no operation
#[wasm_bindgen_test]
async fn unsafe_history_donation_resumes() {
    let _reset = ResetMocks;
    let (mut a, a_on, mut r, r_on, asset_id, rcv2, _) = park_on_safe_height(true).await;
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    rpc_hook(
        PROXY_URL,
        "ack.post",
        "res.error = { code: -104, message: 'not found' }; delete res.result; return res;",
    );
    let res = r
        .refresh(r_on.clone(), None, vec![WAITING_SAFE_HEIGHT], false)
        .await;
    clear_rpc_hooks();
    let refreshed = &res.unwrap()[&rcv2.batch_transfer_idx];
    assert_eq!(
        refreshed.updated_status,
        Some(TransferStatus::WaitingConfirmations)
    );
    assert!(refreshed.failure.is_none());

    settle_both(&mut a, &a_on, &mut r, &r_on).await;
    assert_eq!(
        transfer_of(&r, &rcv2.recipient_id).status,
        TransferStatus::Settled
    );
    assert_eq!(balance(&r, &asset_id).settled, 99);
}
