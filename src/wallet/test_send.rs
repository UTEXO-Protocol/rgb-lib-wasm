use wasm_bindgen_test::*;

use super::online::{
    MOCK_MAX_UTXO_ASSET_ALLOCATIONS, MOCK_SPAM_HIDDEN_SPENDS, MOCK_SPAM_RECIPIENT_ASSIGNMENTS,
};
use super::test_bfa::utils::*;
use super::test_bfa::{funded_wallet, s, send, send_and_settle, settle_both, wallet_data};
use crate::wallet::offline::address_reuse_tests::assert_no_partial_change;
use crate::wallet::{AssetFilter, Balance, Online, Recipient, RgbAllocation, Transfer, Wallet};
use crate::{AssetSchema, Assignment, Error, RgbTransfer, TransferKind, TransferStatus};
use amplify::confinement::Confined;
use bdk_wallet::bitcoin::psbt::Psbt;
use bdk_wallet::bitcoin::{TxMerkleNode, hashes::Hash};
use rgbstd::Txid as RgbTxid;
use rgbstd::containers::FileContent;
use rgbstd::persistence::RgbStore;
use rgbstd::validation::{ResolveWitness, WitnessStatus};
use rgbstd::vm::WitnessOrd;
use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroU32;
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
        .list_transfers(AssetFilter::Id(asset_id.clone()), None)
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
