use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use wasm_bindgen_test::*;

use super::test_bfa::utils::*;
use super::test_send::{ResetMocks, issue, nia_wallet};
use crate::api::proxy_v1::{PostConsignmentData, ProxyClient, post_consignment_message};
use crate::encryption::{gen_ed25519, invoice_id, sign_ed25519, transfer_id};
use crate::utils::now;
use crate::wallet::{
    AssetFilter, Online, ReceiveData, Recipient, SyncKeychain, SyncOptions, SyncStrategy, Transfer,
    TransferScope, Wallet,
};
use crate::{Assignment, Encryption, Error, TransferKind, TransferStatus, sha256_digest};

const AMOUNT: u64 = 1000;
const SENT: u64 = 66;

async fn reusable_receive(
    wallet: &mut Wallet,
    online: &Online,
    witness: bool,
    quota: u32,
    encryption: Option<Encryption>,
    expiration: u64,
) -> ReceiveData {
    let endpoints = vec![transport_endpoint(PROXY_URL)];
    let online = Some(online.clone());
    let quota = Some(quota);
    let data = if witness {
        wallet
            .witness_receive(
                online,
                None,
                Assignment::Any,
                expiration,
                endpoints,
                1,
                encryption,
                quota,
            )
            .await
    } else {
        wallet
            .blind_receive(
                online,
                None,
                Assignment::Any,
                expiration,
                endpoints,
                1,
                encryption,
                quota,
            )
            .await
    }
    .unwrap();
    register_receive(&data, encryption.unwrap_or(Encryption::Required));
    data
}

fn reuse_recipient(data: &ReceiveData, witness: bool) -> Recipient {
    recipient(&data.recipient_id, SENT, witness.then_some(1000), PROXY_URL)
}

/// Begin, sign and end a send of `asset_id` to `recipient`. Return the TXID.
async fn send_result(
    sender: &mut Wallet,
    online: &Online,
    asset_id: &str,
    recipient: Recipient,
) -> Result<String, Error> {
    let begin = sender
        .send_begin(
            online.clone(),
            HashMap::from([(asset_id.to_string(), vec![recipient])]),
            false,
            1,
            1,
            default_send_expiration(),
            Some(transport_endpoint(PROXY_URL)),
            None,
            false,
        )
        .await?;
    let signed = sender.sign_psbt(begin.psbt, None).unwrap();
    Ok(sender.send_end(online.clone(), signed).await?.txid)
}

fn all_transfers(wallet: &Wallet) -> Vec<Transfer> {
    wallet
        .list_transfers(AssetFilter::AnyOrNone, TransferScope::All)
        .unwrap()
}

/// The number of incoming transfers of `asset_id` (or with no asset, if `None`), with `status`.
fn incoming(wallet: &Wallet, asset_id: Option<&str>, status: TransferStatus) -> usize {
    let filter = asset_id.map_or(AssetFilter::None, |a| AssetFilter::Id(a.to_string()));
    wallet
        .list_transfers(filter, TransferScope::All)
        .unwrap()
        .into_iter()
        .filter(|t| {
            matches!(
                t.kind,
                TransferKind::ReceiveBlind | TransferKind::ReceiveWitness
            ) && t.status == status
        })
        .count()
}

fn sent_status(wallet: &Wallet, txid: &str) -> TransferStatus {
    all_transfers(wallet)
        .into_iter()
        .find(|t| t.kind == TransferKind::Send && t.txid.as_deref() == Some(txid))
        .unwrap()
        .status
}

/// The consignment hash the sender registered for the operation with `txid`.
fn sent_consignment_hash(wallet: &Wallet, txid: &str) -> String {
    let idx = all_transfers(wallet)
        .into_iter()
        .find(|t| t.kind == TransferKind::Send && t.txid.as_deref() == Some(txid))
        .unwrap()
        .idx;
    wallet
        .database
        .get_transfer(idx)
        .unwrap()
        .unwrap()
        .consignment_hash
        .unwrap()
}

/// Post `bytes`, in cleartext, to the invoice of `data` as a sender with a fresh key.
async fn post_test_consignment(data: &ReceiveData, bytes: &[u8], consignment_hash: &str) {
    let sender_sk = gen_ed25519();
    let sender_pk = hex::encode(sender_sk.verifying_key().to_bytes());
    let invoice_id = invoice_id(&data.recipient_id, &data.public_key);
    let transfer_id = transfer_id(&invoice_id, &sender_pk);
    let blob_hash = hex::encode(sha256_digest(bytes));
    let digest = sha256_digest(
        post_consignment_message(&transfer_id, consignment_hash, &blob_hash).as_bytes(),
    );
    let post = PostConsignmentData {
        invoice_id,
        transfer_id,
        sender_transport_endpoint: transport_endpoint(PROXY_URL),
        sender_pk,
        consignment_hash: consignment_hash.to_string(),
        batch: false,
        signature: hex::encode(sign_ed25519(&sender_sk, &digest)),
    };
    let res = ProxyClient::new(PROXY_URL)
        .unwrap()
        .post_consignment(&post, bytes, &now().unix_timestamp().to_string(), "0")
        .await
        .unwrap();
    assert_eq!(res.result, Some(true), "{:?}", res.error);
}

/// The consignment bytes and hash of the send with `txid`, read before the sender broadcasts it.
fn sent_consignment(sender: &Wallet, txid: &str, asset_id: &str) -> (Vec<u8>, String) {
    (
        sender.send_consignment_bytes(txid, asset_id).unwrap(),
        sent_consignment_hash(sender, txid),
    )
}

async fn post_garbage(data: &ReceiveData) {
    let bytes = b"not a consignment";
    post_test_consignment(data, bytes, &hex::encode(sha256_digest(bytes))).await;
}

async fn mine() {
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
}

/// The pending blinded count of the UTXO of the blind receive of `data`.
async fn pending_blinded(wallet: &mut Wallet, data: &ReceiveData) -> u32 {
    let utxo = transfer_of(wallet, &data.recipient_id)
        .receive_utxo
        .unwrap();
    wallet
        .list_unspents(None, false, true)
        .await
        .unwrap()
        .into_iter()
        .find(|u| u.utxo.outpoint == utxo)
        .unwrap()
        .pending_blinded
}

/// Send twice to a reusable invoice with quota 2: a third send is refused and both settle.
async fn send_twice_to_reusable_invoice(witness: bool) {
    let _reset = ResetMocks;
    let (mut s, s_on) = nia_wallet(5).await;
    let (mut r, r_on) = nia_wallet(3).await;
    // an allocation per transfer, as the change of a pending transfer is not spendable yet
    let asset_id = issue(&s, vec![AMOUNT, AMOUNT * 2, AMOUNT * 3]);

    let data = reusable_receive(&mut r, &r_on, witness, 2, None, default_rcv_expiration()).await;
    if !witness {
        assert_eq!(pending_blinded(&mut r, &data).await, 2);
    }

    let rcp = reuse_recipient(&data, witness);
    let txid_1 = send_result(&mut s, &s_on, &asset_id, rcp.clone())
        .await
        .unwrap();
    refresh(&mut r, &r_on).await;
    refresh(&mut s, &s_on).await;
    assert_eq!(
        sent_status(&s, &txid_1),
        TransferStatus::WaitingConfirmations
    );
    let txid_2 = send_result(&mut s, &s_on, &asset_id, rcp.clone())
        .await
        .unwrap();

    // the invoice is now full
    assert!(matches!(
        send_result(&mut s, &s_on, &asset_id, rcp).await,
        Err(Error::InvoiceQuotaExceeded)
    ));

    // the recipient discovers the 2nd transfer, which gets a transfer of its own
    refresh(&mut r, &r_on).await;
    assert_eq!(
        incoming(&r, Some(&asset_id), TransferStatus::WaitingConfirmations),
        2
    );
    refresh(&mut s, &s_on).await;
    assert_eq!(
        sent_status(&s, &txid_2),
        TransferStatus::WaitingConfirmations
    );

    mine().await;
    refresh(&mut r, &r_on).await;
    refresh(&mut s, &s_on).await;
    assert_eq!(incoming(&r, Some(&asset_id), TransferStatus::Settled), 2);
    assert_eq!(sent_status(&s, &txid_1), TransferStatus::Settled);
    assert_eq!(sent_status(&s, &txid_2), TransferStatus::Settled);
    // each transfer keeps its own consignment, in a directory of its own
    let (consignments, dirs): (HashSet<_>, HashSet<_>) = all_transfers(&r)
        .into_iter()
        .map(|t| {
            let sent = all_transfers(&s)
                .into_iter()
                .find(|st| st.kind == TransferKind::Send && st.txid == t.txid)
                .unwrap();
            let consignment = r.get_consignment(t.idx).unwrap().unwrap();
            assert_eq!(s.get_consignment(sent.idx), Ok(Some(consignment.clone())));
            let path = PathBuf::from(t.consignment_path.unwrap());
            (consignment, path.parent().unwrap().to_path_buf())
        })
        .unzip();
    assert_eq!((consignments.len(), dirs.len()), (2, 2));
    let balance = r.get_asset_balance(asset_id).unwrap();
    assert_eq!(balance.settled, SENT * 2);
    assert_eq!(balance.spendable, SENT * 2);
}

#[wasm_bindgen_test]
async fn reusable_blind_invoice() {
    send_twice_to_reusable_invoice(false).await;
}

#[wasm_bindgen_test]
async fn reusable_witness_invoice() {
    send_twice_to_reusable_invoice(true).await;
}

/// Transfers that cannot pay a reusable invoice are refused, with no local transfer.
/// Their deletion from the proxy frees the quota.
#[wasm_bindgen_test]
async fn reusable_invoice_spam_refused() {
    let _reset = ResetMocks;
    let (mut s, s_on) = nia_wallet(5).await;
    let (mut r, r_on) = nia_wallet(3).await;
    let asset_id = issue(&s, vec![AMOUNT]);

    // The recipient requires encryption, so it cannot decrypt a cleartext consignment.
    let data = reusable_receive(
        &mut r,
        &r_on,
        false,
        2,
        Some(Encryption::Required),
        default_rcv_expiration(),
    )
    .await;
    let rcp = reuse_recipient(&data, false);
    post_garbage(&data).await;
    post_garbage(&data).await;
    assert!(matches!(
        send_result(&mut s, &s_on, &asset_id, rcp.clone()).await,
        Err(Error::InvoiceQuotaExceeded)
    ));

    // discovery refuses and deletes them
    refresh(&mut r, &r_on).await;
    assert_eq!(all_transfers(&r).len(), 1);

    // the payment gets the transfer created with the invoice
    send_result(&mut s, &s_on, &asset_id, rcp).await.unwrap();
    refresh(&mut r, &r_on).await;
    let transfers = all_transfers(&r);
    assert_eq!(transfers.len(), 1);
    assert_eq!(transfers[0].status, TransferStatus::WaitingConfirmations);
}

/// A copy of a transfer is refused, so the payment counts once.
/// The failed stored transfer is deleted from the proxy. A failed deletion is retried.
#[wasm_bindgen_test]
async fn reusable_invoice_failed_transfer_delete_retried() {
    let _reset = ResetMocks;
    let (mut s, s_on) = nia_wallet(5).await;
    let (mut r, r_on) = nia_wallet(3).await;
    // an allocation per transfer, as the change of a pending transfer is not spendable yet
    let asset_id = issue(&s, vec![AMOUNT, AMOUNT * 2]);

    let data = reusable_receive(
        &mut r,
        &r_on,
        false,
        2,
        Some(Encryption::Optional),
        default_rcv_expiration(),
    )
    .await;
    let rcp = reuse_recipient(&data, false);
    let txid = send_result(&mut s, &s_on, &asset_id, rcp.clone())
        .await
        .unwrap();
    let (bytes, hash) = sent_consignment(&s, &txid, &asset_id);
    refresh(&mut r, &r_on).await;
    refresh(&mut s, &s_on).await;

    // The copy passes discovery but fails in processing: its payment is already credited.
    // The recipient accepts cleartext consignments, so the test can post the copy unchanged.
    post_test_consignment(&data, &bytes, &hash).await;
    rpc_fail(PROXY_URL, "transfer.delete");
    refresh(&mut r, &r_on).await;
    assert_eq!(incoming(&r, None, TransferStatus::Failed), 1);
    assert!(matches!(
        send_result(&mut s, &s_on, &asset_id, rcp.clone()).await,
        Err(Error::InvoiceQuotaExceeded)
    ));

    // The retried deletion frees the slot for another payment.
    clear_rpc_hooks();
    refresh(&mut r, &r_on).await;
    send_result(&mut s, &s_on, &asset_id, rcp).await.unwrap();
    refresh(&mut r, &r_on).await;
    assert_eq!(
        incoming(&r, Some(&asset_id), TransferStatus::WaitingConfirmations),
        2
    );

    // Both payments settle. The copy does not count.
    refresh(&mut s, &s_on).await;
    mine().await;
    refresh(&mut r, &r_on).await;
    refresh(&mut s, &s_on).await;
    assert_eq!(sent_status(&s, &txid), TransferStatus::Settled);
    assert_eq!(r.get_asset_balance(asset_id).unwrap().settled, SENT * 2);
}

/// A failed consignment download creates no local transfer.
/// A later refresh discovers the transfer again.
#[wasm_bindgen_test]
async fn reusable_invoice_download_fail() {
    let _reset = ResetMocks;
    let (mut s, s_on) = nia_wallet(5).await;
    let (mut r, r_on) = nia_wallet(3).await;
    let asset_id = issue(&s, vec![AMOUNT]);

    let data = reusable_receive(&mut r, &r_on, false, 2, None, default_rcv_expiration()).await;
    send_result(&mut s, &s_on, &asset_id, reuse_recipient(&data, false))
        .await
        .unwrap();

    rpc_fail(PROXY_URL, "consignment.get");
    refresh(&mut r, &r_on).await;
    let transfers = all_transfers(&r);
    assert_eq!(transfers.len(), 1);
    assert_eq!(transfers[0].status, TransferStatus::WaitingCounterparty);

    clear_rpc_hooks();
    refresh(&mut r, &r_on).await;
    let transfers = all_transfers(&r);
    assert_eq!(transfers.len(), 1);
    assert_eq!(transfers[0].status, TransferStatus::WaitingConfirmations);
}

/// The script of a reusable witness invoice stays watched after its first transfer settles.
#[wasm_bindgen_test]
async fn reusable_witness_invoice_watched_after_settle() {
    let _reset = ResetMocks;
    let (mut s, s_on) = nia_wallet(5).await;
    let (mut r, r_on) = nia_wallet(3).await;
    let asset_id = issue(&s, vec![AMOUNT, AMOUNT * 2]);

    let data = reusable_receive(&mut r, &r_on, true, 2, None, default_rcv_expiration()).await;
    let rcp = reuse_recipient(&data, true);
    let txid = send_result(&mut s, &s_on, &asset_id, rcp.clone())
        .await
        .unwrap();
    refresh(&mut r, &r_on).await;
    refresh(&mut s, &s_on).await;
    mine().await;
    refresh(&mut r, &r_on).await;
    refresh(&mut s, &s_on).await;
    assert_eq!(sent_status(&s, &txid), TransferStatus::Settled);

    // A sync sees the 2nd payment, a donation broadcast at once, before discovery.
    let txid = s
        .send(
            s_on.clone(),
            HashMap::from([(asset_id.clone(), vec![rcp])]),
            true,
            1,
            1,
            default_send_expiration(),
            Some(transport_endpoint(PROXY_URL)),
        )
        .await
        .unwrap()
        .txid;
    wait_for_esplora_sync().await;
    r.sync(
        r_on.clone(),
        SyncOptions {
            keychain: SyncKeychain::Colored,
            strategy: SyncStrategy::FullSync,
        },
    )
    .await
    .unwrap();
    let txos: Vec<_> = r
        .database
        .iter_txos()
        .unwrap()
        .into_iter()
        .filter(|t| t.txid == txid)
        .collect();
    assert_eq!(txos.len(), 1);
    assert!(txos[0].pending_witness);

    refresh(&mut r, &r_on).await;
    assert_eq!(
        incoming(&r, Some(&asset_id), TransferStatus::WaitingConfirmations),
        1
    );
}

/// The failed transfers of a live reusable invoice are not deleted, so it keeps receiving.
#[wasm_bindgen_test]
async fn reusable_invoice_transfers_not_deleted_while_live() {
    let _reset = ResetMocks;
    let (mut s, s_on) = nia_wallet(5).await;
    let (mut r, r_on) = nia_wallet(3).await;
    let asset_id = issue(&s, vec![AMOUNT]);

    // the receive created with the invoice is failed and deleting all skips it
    let data = reusable_receive(&mut r, &r_on, false, 2, None, default_rcv_expiration()).await;
    let invoice_bt = transfer_of(&r, &data.recipient_id).batch_transfer_idx;
    assert!(
        r.fail_transfers(r_on.clone(), Some(invoice_bt), false, false)
            .await
            .unwrap()
    );
    assert!(!r.delete_transfers(None, false).unwrap());

    // a payment to the invoice is still discovered
    send_result(&mut s, &s_on, &asset_id, reuse_recipient(&data, false))
        .await
        .unwrap();
    refresh(&mut r, &r_on).await;
    assert_eq!(
        incoming(&r, Some(&asset_id), TransferStatus::WaitingConfirmations),
        1
    );
}

/// A reusable invoice with no asset receives a different asset with each transfer.
async fn send_assets_to_reusable_invoice(witness: bool) {
    let _reset = ResetMocks;
    let (mut s, s_on) = nia_wallet(5).await;
    let (mut r, r_on) = nia_wallet(3).await;
    let asset_1 = issue(&s, vec![AMOUNT]);
    let asset_2 = issue(&s, vec![AMOUNT]);

    let data = reusable_receive(&mut r, &r_on, witness, 2, None, default_rcv_expiration()).await;
    for asset_id in [&asset_1, &asset_2] {
        let txid = send_result(&mut s, &s_on, asset_id, reuse_recipient(&data, witness))
            .await
            .unwrap();
        refresh(&mut r, &r_on).await;
        refresh(&mut s, &s_on).await;
        mine().await;
        refresh(&mut r, &r_on).await;
        refresh(&mut s, &s_on).await;
        assert_eq!(sent_status(&s, &txid), TransferStatus::Settled);
    }

    for asset_id in [asset_1, asset_2] {
        assert_eq!(incoming(&r, Some(&asset_id), TransferStatus::Settled), 1);
        assert_eq!(r.get_asset_balance(asset_id).unwrap().settled, SENT);
    }
}

#[wasm_bindgen_test]
async fn reusable_blind_invoice_any_asset() {
    send_assets_to_reusable_invoice(false).await;
}

#[wasm_bindgen_test]
async fn reusable_witness_invoice_any_asset() {
    send_assets_to_reusable_invoice(true).await;
}
