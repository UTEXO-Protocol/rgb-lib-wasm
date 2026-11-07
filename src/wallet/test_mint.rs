use std::collections::HashMap;
use wasm_bindgen_test::*;

use super::test_bfa::utils::*;
use super::test_bfa::*;
use crate::wallet::online::{MINT_AFTER_BLOCK_GRACE_SECS, MINT_FINALITY_GRACE_SECS};
use crate::wallet::rust_only::validate_consignment_offchain;
use crate::wallet::{
    AssetFilter, MintBeginResult, Online, OnlineOptions, OperationResult, ReceiveData, Recipient,
    Wallet,
};
use crate::{
    AssetSchema, Assignment, BitcoinNetwork, Error, OS_ASSET, OS_MINT, RgbOutExt, TransferKind,
    TransferStatus, script_buf_from_recipient_id,
};
use bdk_wallet::bitcoin::{OutPoint, Psbt};
use rgbstd::{ContractId, OpId, Opout};
use serde_json::json;
use std::str::FromStr;

const MINT_AMOUNT: u64 = 100;

/// A blind receive of `MINT_AMOUNT`.
async fn mint_rcv(wallet: &mut Wallet, online: &Online, expiration: u64) -> ReceiveData {
    blind_rcv(
        wallet,
        online,
        None,
        Assignment::Fungible(MINT_AMOUNT),
        expiration,
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap()
}

fn mint_recipient(receive: &ReceiveData, amount: u64) -> Recipient {
    recipient(&receive.recipient_id, amount, None, PROXY_URL)
}

async fn mint_begin(
    minter: &mut Wallet,
    minter_online: &Online,
    asset_id: &str,
    recipient: Recipient,
) -> Result<MintBeginResult, Error> {
    minter
        .mint_begin(
            minter_online.clone(),
            asset_id.to_string(),
            recipient,
            1,
            1,
            transport_endpoint(PROXY_URL),
        )
        .await
}

/// Lock the mint on the bridge and finalize the lock.
async fn lock_mint(bridge: &str, res: &MintBeginResult, amount: u64) {
    emit_funds_in(bridge, OpId::from_str(&res.details.opid).unwrap(), amount).await;
    finalize_evm().await;
}

async fn mint_end(
    minter: &mut Wallet,
    minter_online: &Online,
    res: &MintBeginResult,
) -> Result<OperationResult, Error> {
    let signed = minter.sign_psbt(res.psbt.clone(), None).unwrap();
    minter.mint_end(minter_online.clone(), signed).await
}

/// Mint `amount` of `asset_id` to a blinded UTXO of `receiver` and settle it. Return the
/// recipient ID and the mint opid.
async fn mint(
    minter: &mut Wallet,
    minter_online: &Online,
    receiver: &mut Wallet,
    receiver_online: &Online,
    asset_id: &str,
    bridge: &str,
    amount: u64,
) -> (String, OpId) {
    let receive = blind_rcv(
        receiver,
        receiver_online,
        None,
        Assignment::Fungible(amount),
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap();
    let res = mint_begin(
        minter,
        minter_online,
        asset_id,
        mint_recipient(&receive, amount),
    )
    .await
    .unwrap();
    receiver
        .refresh(receiver_online.clone(), None, vec![], false)
        .await
        .unwrap();
    lock_mint(bridge, &res, amount).await;
    mint_end(minter, minter_online, &res).await.unwrap();
    let statuses = settle_both(minter, minter_online, receiver, receiver_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
    (
        receive.recipient_id,
        OpId::from_str(&res.details.opid).unwrap(),
    )
}

#[wasm_bindgen_test]
async fn bfa_mint_settles_and_is_spendable() {
    let wd = wallet_data(vec![AssetSchema::Bfa]);
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 5).await;
    let (mut b, b_online) = funded_wallet(wd.clone(), 3).await;
    let bridge = new_evm_address();
    deploy_bridge(&bridge).await;
    let asset = a
        .issue_asset_bfa(s("BFA"), s("Bridged"), 0, 3, CHAIN_ID, s(&bridge), None)
        .unwrap();
    let asset_id = asset.asset_id.clone();

    let receive = mint_rcv(&mut b, &b_online, default_rcv_expiration()).await;
    spy_rpc();
    clear_rpc_hooks();
    let after_block = finalized_block().await;
    let res = mint_begin(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&receive, MINT_AMOUNT),
    )
    .await
    .unwrap();
    assert_eq!(res.details.opid.len(), 64);
    assert_eq!(res.details.min_confirmations, 1);
    let psbt = Psbt::from_str(&res.psbt).unwrap();
    let opret = psbt
        .unsigned_tx
        .output
        .iter()
        .position(|o| o.script_pubkey.is_op_return())
        .unwrap();
    assert_eq!(psbt.outputs[opret].mpc_entropy(), Some(res.details.entropy));

    // the receiver checks the mint before the lock and tells what to lock
    assert!(
        b.get_received_mint(receive.recipient_id.clone())
            .unwrap()
            .is_none()
    );
    b.refresh(b_online.clone(), None, vec![], false)
        .await
        .unwrap();
    let received = b
        .get_received_mint(receive.recipient_id.clone())
        .unwrap()
        .unwrap();
    assert_eq!(received.opid, res.details.opid);
    assert_eq!(received.amount, MINT_AMOUNT);
    assert_eq!(received.after_block, after_block);
    assert_eq!(received.evm_chain_id, CHAIN_ID);
    assert_eq!(received.bridge_contract.to_lowercase(), bridge);

    // no lock yet, then a lock not finalized yet
    let opid = OpId::from_str(&res.details.opid).unwrap();
    for lock in [false, true] {
        if lock {
            emit_funds_in(&bridge, opid, MINT_AMOUNT).await;
        }
        let result = mint_end(&mut a, &a_online, &res).await;
        assert!(
            matches!(result, Err(Error::MintEventNotFound { ref opid }) if *opid == res.details.opid),
            "{result:?}"
        );
    }

    finalize_evm().await;
    let minted = mint_end(&mut a, &a_online, &res).await.unwrap();
    assert_eq!(minted.batch_transfer_idx, res.batch_transfer_idx);
    assert_eq!(minted.entropy, res.details.entropy);
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));

    // a mint posts its consignment and makes no ACK call
    let calls = (
        rpc_count("consignment.post"),
        rpc_count("ack.prepare"),
        rpc_count("ack.post"),
    );
    clear_rpc_hooks();
    assert_eq!(calls, (1, 0, 0));

    // only an online check can accept a consignment that holds a mint
    let bytes = &a.transfer_artifacts[&minted.txid].consignment_bytes[&asset_id];
    let validated =
        validate_consignment_offchain(bytes, &minted.txid, BitcoinNetwork::Regtest).unwrap();
    assert!(!validated.valid);
    assert_eq!(validated.error.as_deref(), Some("external_anchors"));

    let transfers = a.list_transfers(AssetFilter::Any, None).unwrap();
    let transfer = transfers.last().unwrap();
    assert_eq!(transfer.kind, TransferKind::Mint);
    assert_eq!(transfer.status, TransferStatus::Settled);
    assert_eq!(transfer.recipient_id, Some(receive.recipient_id.clone()));
    assert_eq!(
        transfer.requested_assignment,
        Some(Assignment::Fungible(MINT_AMOUNT))
    );
    assert_eq!(transfer.txid, Some(minted.txid));
    assert!(transfer.consignment_path.is_some());
    assert_eq!(mint_rights(&mut a, &asset_id), 2);
    let b_balance = b.get_asset_balance(asset_id.clone()).unwrap();
    assert_eq!(
        (b_balance.settled, b_balance.spendable),
        (MINT_AMOUNT, MINT_AMOUNT)
    );

    // the details stay after the settlement and in a restored wallet
    assert_eq!(
        b.get_received_mint(receive.recipient_id.clone()).unwrap(),
        Some(received.clone())
    );
    let backup = b.backup("password").unwrap();
    let mut restored = Wallet::new(wd).unwrap();
    restored.restore_backup(&backup, "password").unwrap();
    assert_eq!(
        restored
            .get_received_mint(receive.recipient_id.clone())
            .unwrap(),
        Some(received)
    );

    // the minted units are spendable
    send_and_settle(&mut b, &b_online, &mut a, &a_online, &asset_id, 30).await;
    let b_balance = b.get_asset_balance(asset_id.clone()).unwrap();
    assert_eq!((b_balance.settled, b_balance.spendable), (70, 70));
    assert_eq!(a.get_asset_balance(asset_id.clone()).unwrap().settled, 30);

    // a mint to a witness invoice pays the script of the recipient
    let receive = witness_rcv(
        &mut b,
        &b_online,
        None,
        Assignment::Fungible(MINT_AMOUNT),
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap();
    let recipient = recipient(&receive.recipient_id, MINT_AMOUNT, Some(1000), PROXY_URL);
    let res = mint_begin(&mut a, &a_online, &asset_id, recipient)
        .await
        .unwrap();
    let script = script_buf_from_recipient_id(receive.recipient_id.clone())
        .unwrap()
        .unwrap();
    let psbt = Psbt::from_str(&res.psbt).unwrap();
    assert!(
        psbt.unsigned_tx
            .output
            .iter()
            .any(|o| o.script_pubkey == script && o.value.to_sat() == 1000)
    );
    b.refresh(b_online.clone(), None, vec![], false)
        .await
        .unwrap();
    lock_mint(&bridge, &res, MINT_AMOUNT).await;
    mint_end(&mut a, &a_online, &res).await.unwrap();
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
    assert_eq!(
        b.get_asset_balance(asset_id).unwrap().settled,
        70 + MINT_AMOUNT
    );
}

#[wasm_bindgen_test]
async fn bfa_mint_begin_refusals() {
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 5).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let bridge = new_evm_address();
    let rcv = mint_rcv(&mut b, &b_online, default_rcv_expiration()).await;

    // the last mint right is never spent
    let single = a
        .issue_asset_bfa(s("BFA"), s("Bridged"), 0, 1, CHAIN_ID, s(&bridge), None)
        .unwrap();
    let result = mint_begin(
        &mut a,
        &a_online,
        &single.asset_id,
        mint_recipient(&rcv, 100),
    )
    .await;
    assert!(matches!(result, Err(Error::LastMintRight)), "{result:?}");

    // the invoice has to outlast the lock window and carry its expiration
    let bfa = a
        .issue_asset_bfa(s("BFA"), s("Bridged"), 0, 2, CHAIN_ID, s(&bridge), None)
        .unwrap();
    let id = &bfa.asset_id;
    let short_expiration = default_rcv_expiration() - 86400 + 1800;
    let short = mint_rcv(&mut b, &b_online, short_expiration).await;
    let result = mint_begin(&mut a, &a_online, id, mint_recipient(&short, 100)).await;
    assert!(matches!(result, Err(Error::InvoiceExpired)), "{result:?}");
    let mut recipient = mint_recipient(&rcv, 100);
    recipient.expiration_timestamp = None;
    let result = mint_begin(&mut a, &a_online, id, recipient).await;
    assert!(
        matches!(result, Err(Error::InvalidRecipientData { ref details }) if details.contains("expiration")),
        "{result:?}"
    );

    // the invoice has to carry the public key of the recipient
    let mut recipient = mint_recipient(&rcv, 100);
    recipient.public_key = None;
    let result = mint_begin(&mut a, &a_online, id, recipient).await;
    assert!(
        matches!(result, Err(Error::InvalidPubkey { .. })),
        "{result:?}"
    );
}

#[wasm_bindgen_test]
async fn bfa_mint_refused_out_of_scan_window() {
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 4).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let bridge = new_evm_address();
    deploy_bridge(&bridge).await;
    let asset = a
        .issue_asset_bfa(s("BFA"), s("Bridged"), 0, 2, CHAIN_ID, s(&bridge), None)
        .unwrap();
    let receive = mint_rcv(&mut b, &b_online, default_rcv_expiration()).await;
    let res = mint_begin(
        &mut a,
        &a_online,
        &asset.asset_id,
        mint_recipient(&receive, MINT_AMOUNT),
    )
    .await
    .unwrap();

    // the lock is finalized, but the receiver scans no block after the one the mint commits to
    lock_mint(&bridge, &res, MINT_AMOUNT).await;
    crate::api::evm::MOCK_MAX_MINT_SCAN_BLOCKS.replace(Some(0));
    let refreshed = b.refresh(b_online.clone(), None, vec![], false).await;
    crate::api::evm::MOCK_MAX_MINT_SCAN_BLOCKS.replace(None);
    let refreshed = refreshed.unwrap();
    let refreshed = refreshed.values().next().unwrap();
    assert_eq!(
        refreshed.updated_status,
        Some(TransferStatus::Failed),
        "{refreshed:?}"
    );
    assert!(b.get_received_mint(receive.recipient_id).unwrap().is_none());
    assert!(b.list_assets(vec![]).unwrap().bfa.unwrap().is_empty());
}

#[wasm_bindgen_test]
async fn bfa_mint_history_refused_by_reject_list() {
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 4).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 3).await;
    let (mut c, c_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let bridge = new_evm_address();
    let reject_list = new_evm_address();
    deploy_bridge(&bridge).await;
    deploy_reject_list(&reject_list, &[]).await;
    let asset = a
        .issue_asset_bfa(
            s("BFA"),
            s("Bridged"),
            0,
            3,
            CHAIN_ID,
            s(&bridge),
            Some(reject_list.clone()),
        )
        .unwrap();
    let asset_id = asset.asset_id.clone();
    let (_, opid) = mint(
        &mut a,
        &a_online,
        &mut b,
        &b_online,
        &asset_id,
        &bridge,
        MINT_AMOUNT,
    )
    .await;

    // the list rejects the minted allocation: the owner cannot spend it
    deploy_reject_list(&reject_list, &[(opid, 0)]).await;
    finalize_evm().await;
    let receive = blind_rcv(
        &mut c,
        &c_online,
        None,
        Assignment::Fungible(30),
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap();
    let recipient_map = HashMap::from([(
        asset_id.clone(),
        vec![recipient(&receive.recipient_id, 30, None, PROXY_URL)],
    )]);
    let result = b
        .send_begin(
            b_online.clone(),
            recipient_map,
            false,
            1,
            1,
            default_send_expiration(),
            Some(transport_endpoint(PROXY_URL)),
            None,
            false,
        )
        .await;
    assert!(
        matches!(result, Err(Error::InsufficientAssignments { asset_id: ref id, .. }) if *id == asset_id),
        "{result:?}"
    );

    // the list rejects all the mint rights left: the minter cannot mint anymore
    let mut rejected: Vec<Opout> = mint_right_opouts(&mut a, &asset_id)
        .into_iter()
        .flatten()
        .collect();
    assert_eq!(rejected.len(), 2);
    rejected.push(Opout::new(opid, OS_ASSET, 0));
    deploy_reject_list_opouts(&reject_list, &rejected).await;
    finalize_evm().await;
    let receive = mint_rcv(&mut c, &c_online, default_rcv_expiration()).await;
    let result = mint_begin(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&receive, MINT_AMOUNT),
    )
    .await;
    assert!(
        matches!(result, Err(Error::InsufficientAssignments { asset_id: ref id, .. }) if *id == asset_id),
        "{result:?}"
    );
}

/// The mint right opouts of `asset_id`, by UTXO, in the order the wallet selects the UTXOs.
fn mint_right_opouts(wallet: &mut Wallet, asset_id: &str) -> Vec<Vec<Opout>> {
    let unspents = wallet.list_unspents(None, false, true).unwrap();
    let contract_id = ContractId::from_str(asset_id).unwrap();
    let runtime = wallet.rgb_runtime().unwrap();
    unspents
        .into_iter()
        .filter(|u| {
            u.rgb_allocations.iter().any(|a| {
                a.asset_id.as_deref() == Some(asset_id) && a.assignment == Assignment::MintRight
            })
        })
        .map(|u| {
            runtime
                .contract_assignments_for(contract_id, [OutPoint::from(u.utxo.outpoint)])
                .unwrap()
                .into_values()
                .flat_map(|state| state.into_keys())
                .filter(|opout| opout.ty == OS_MINT)
                .collect()
        })
        .collect()
}

#[wasm_bindgen_test]
async fn bfa_mint_multi_right_utxo() {
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 5).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 2).await;
    let bridge = new_evm_address();
    let reject_list = new_evm_address();
    deploy_bridge(&bridge).await;
    deploy_reject_list(&reject_list, &[]).await;
    let asset_id = a
        .issue_asset_bfa(
            s("BFA"),
            s("Bridged"),
            0,
            4,
            CHAIN_ID,
            s(&bridge),
            Some(reject_list.clone()),
        )
        .unwrap()
        .asset_id;

    // each mint right is on its own UTXO
    let rights = mint_right_opouts(&mut a, &asset_id);
    assert_eq!(rights.len(), 4);
    assert!(rights.iter().all(|opouts| opouts.len() == 1));

    // the list rejects the first 2 mint rights to be selected: the mint retries with the next one
    // and moves the rejected rights to its change UTXO
    deploy_reject_list_opouts(&reject_list, &[rights[0][0], rights[1][0]]).await;
    finalize_evm().await;
    mint(
        &mut a,
        &a_online,
        &mut b,
        &b_online,
        &asset_id,
        &bridge,
        MINT_AMOUNT,
    )
    .await;

    // the list allows the moved rights again: 2 clean mint rights share the change UTXO, which
    // follows the UTXO of the untouched mint right
    deploy_reject_list(&reject_list, &[]).await;
    finalize_evm().await;
    let rights = mint_right_opouts(&mut a, &asset_id);
    assert_eq!(rights.len(), 2);
    assert_eq!(rights[0].len(), 1);
    assert_eq!(rights[1].len(), 2);

    // the list rejects the untouched mint right: the retry selects the shared UTXO too, but the
    // mint spends a single mint right and moves the other 2
    deploy_reject_list_opouts(&reject_list, &[rights[0][0]]).await;
    finalize_evm().await;
    mint(
        &mut a,
        &a_online,
        &mut b,
        &b_online,
        &asset_id,
        &bridge,
        MINT_AMOUNT,
    )
    .await;
    let rights = mint_right_opouts(&mut a, &asset_id);
    assert_eq!(rights.len(), 1);
    assert_eq!(rights[0].len(), 2);
    assert_eq!(
        b.get_asset_balance(asset_id).unwrap().settled,
        MINT_AMOUNT * 2
    );
}

/// Set `value` in a field of the batch transfer `idx` of `wallet`. The DB API cannot change
/// every field.
fn edit_batch_transfer(wallet: &mut Wallet, idx: i32, field: &str, value: i64) {
    let mut db = serde_json::to_value(&*wallet.database).unwrap();
    let batch_transfer = db["batch_transfers"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|b| b["idx"] == idx)
        .unwrap();
    batch_transfer[field] = json!(value);
    let mut snapshot = wallet.snapshot().unwrap();
    snapshot.db = serde_json::from_value(db).unwrap();
    wallet.restore_from_snapshot(snapshot).unwrap();
}

fn transfer_status(wallet: &mut Wallet, recipient_id: &str) -> TransferStatus {
    wallet
        .list_transfers(AssetFilter::Any, None)
        .unwrap()
        .into_iter()
        .find(|t| t.recipient_id.as_deref() == Some(recipient_id))
        .unwrap()
        .status
}

#[wasm_bindgen_test]
async fn bfa_mint_receiver_checks() {
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 5).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let bridge = new_evm_address();
    deploy_bridge(&bridge).await;
    let asset_id = a
        .issue_asset_bfa(s("BFA"), s("Bridged"), 0, 4, CHAIN_ID, s(&bridge), None)
        .unwrap()
        .asset_id;

    // a mint of another amount than the requested one is refused before the lock
    let rcv = mint_rcv(&mut b, &b_online, default_rcv_expiration()).await;
    mint_begin(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&rcv, MINT_AMOUNT / 2),
    )
    .await
    .unwrap();
    b.refresh(b_online.clone(), None, vec![], false)
        .await
        .unwrap();
    assert_eq!(
        transfer_status(&mut b, &rcv.recipient_id),
        TransferStatus::Failed
    );
    assert!(b.get_received_mint(rcv.recipient_id).unwrap().is_none());

    // a mint that commits to a block older than the invoice by more than the grace is refused
    let rcv = mint_rcv(&mut b, &b_online, default_rcv_expiration()).await;
    let created_at = latest_evm_timestamp().await + MINT_AFTER_BLOCK_GRACE_SECS + 60;
    edit_batch_transfer(&mut b, rcv.batch_transfer_idx, "created_at", created_at);
    mint_begin(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&rcv, MINT_AMOUNT),
    )
    .await
    .unwrap();
    b.refresh(b_online.clone(), None, vec![], false)
        .await
        .unwrap();
    assert_eq!(
        transfer_status(&mut b, &rcv.recipient_id),
        TransferStatus::Failed
    );
    assert!(b.get_received_mint(rcv.recipient_id).unwrap().is_none());

    // with no EVM RPC URL for the chain, the receiver waits and checks the mint later
    let rcv = mint_rcv(&mut b, &b_online, default_rcv_expiration()).await;
    mint_begin(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&rcv, MINT_AMOUNT),
    )
    .await
    .unwrap();
    let no_evm = b.go_online(go_online_options(false)).await.unwrap();
    b.refresh(no_evm, None, vec![], false).await.unwrap();
    assert_eq!(
        transfer_status(&mut b, &rcv.recipient_id),
        TransferStatus::WaitingCounterparty
    );
    assert!(
        b.get_received_mint(rcv.recipient_id.clone())
            .unwrap()
            .is_none()
    );
    let b_online = b
        .go_online(OnlineOptions {
            evm_rpc_urls: evm_rpc_urls(),
            ..go_online_options(false)
        })
        .await
        .unwrap();
    b.refresh(b_online, None, vec![], false).await.unwrap();
    assert_eq!(
        b.get_received_mint(rcv.recipient_id.clone())
            .unwrap()
            .unwrap()
            .amount,
        MINT_AMOUNT
    );

    // with no EVM RPC URL the lock cannot be looked up, so an expired mint waits in the grace
    let now = crate::utils::now().unix_timestamp();
    edit_batch_transfer(&mut b, rcv.batch_transfer_idx, "expiration", now - 1);
    let no_evm = b.go_online(go_online_options(false)).await.unwrap();
    b.refresh(no_evm.clone(), None, vec![], false)
        .await
        .unwrap();
    assert_eq!(
        transfer_status(&mut b, &rcv.recipient_id),
        TransferStatus::WaitingCounterparty
    );

    // with no EVM RPC URL and no lock, a mint past its deadline and grace fails on refresh
    let expiration = now - MINT_FINALITY_GRACE_SECS - 1;
    edit_batch_transfer(&mut b, rcv.batch_transfer_idx, "expiration", expiration);
    b.refresh(no_evm, None, vec![], false).await.unwrap();
    assert_eq!(
        transfer_status(&mut b, &rcv.recipient_id),
        TransferStatus::Failed
    );
}

/// Begin a mint to a blinded UTXO of `receiver` and let the receiver check it.
async fn begin_checked_mint(
    minter: &mut Wallet,
    minter_online: &Online,
    receiver: &mut Wallet,
    receiver_online: &Online,
    asset_id: &str,
) -> (MintBeginResult, ReceiveData) {
    let receive = mint_rcv(receiver, receiver_online, default_rcv_expiration()).await;
    let res = mint_begin(
        minter,
        minter_online,
        asset_id,
        mint_recipient(&receive, MINT_AMOUNT),
    )
    .await
    .unwrap();
    receiver
        .refresh(receiver_online.clone(), None, vec![], false)
        .await
        .unwrap();
    assert!(
        receiver
            .get_received_mint(receive.recipient_id.clone())
            .unwrap()
            .is_some()
    );
    (res, receive)
}

/// Set the lock deadline of the mint on both sides.
fn set_deadline(
    minter: &mut Wallet,
    res: &MintBeginResult,
    receiver: &mut Wallet,
    receive: &ReceiveData,
    deadline: i64,
) {
    edit_batch_transfer(minter, res.batch_transfer_idx, "expiration", deadline);
    edit_batch_transfer(receiver, receive.batch_transfer_idx, "expiration", deadline);
}

#[wasm_bindgen_test]
async fn bfa_mint_lock_deadline() {
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 4).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let bridge = new_evm_address();
    deploy_bridge(&bridge).await;
    let asset_id = a
        .issue_asset_bfa(s("BFA"), s("Bridged"), 0, 3, CHAIN_ID, s(&bridge), None)
        .unwrap()
        .asset_id;

    // a lock included by the deadline, but finalized after it, is waited for: neither side fails
    // the mint, which is then broadcast past the deadline and settles
    let (res, receive) = begin_checked_mint(&mut a, &a_online, &mut b, &b_online, &asset_id).await;
    let opid = OpId::from_str(&res.details.opid).unwrap();
    emit_funds_in(&bridge, opid, MINT_AMOUNT).await;
    let deadline = latest_evm_timestamp().await;
    set_deadline(&mut a, &res, &mut b, &receive, deadline);
    let wait = deadline + 1 - crate::utils::now().unix_timestamp();
    if wait > 0 {
        sleep_ms(wait as u32 * 1000).await;
    }
    assert!(
        !b.fail_transfers(b_online.clone(), None, false, false)
            .await
            .unwrap()
    );
    assert!(
        !a.fail_transfers(a_online.clone(), None, false, false)
            .await
            .unwrap()
    );
    finalize_evm().await;
    b.refresh(b_online.clone(), None, vec![], false)
        .await
        .unwrap();
    mint_end(&mut a, &a_online, &res).await.unwrap();
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
    assert_eq!(
        b.get_asset_balance(asset_id.clone()).unwrap().settled,
        MINT_AMOUNT
    );

    // with no lock by the deadline, then with a lock after it, the mint is not broadcast and both
    // sides fail it
    let (res, receive) = begin_checked_mint(&mut a, &a_online, &mut b, &b_online, &asset_id).await;
    let deadline = crate::utils::now().unix_timestamp() - 1;
    set_deadline(&mut a, &res, &mut b, &receive, deadline);
    let result = mint_end(&mut a, &a_online, &res).await;
    assert!(matches!(result, Err(Error::TransferExpired)), "{result:?}");
    lock_mint(&bridge, &res, MINT_AMOUNT).await;
    let result = mint_end(&mut a, &a_online, &res).await;
    assert!(matches!(result, Err(Error::TransferExpired)), "{result:?}");
    assert!(
        b.fail_transfers(b_online.clone(), None, false, false)
            .await
            .unwrap()
    );
    assert_eq!(
        transfer_status(&mut b, &receive.recipient_id),
        TransferStatus::Failed
    );
    assert!(b.get_received_mint(receive.recipient_id).unwrap().is_none());
    assert!(
        a.fail_transfers(a_online.clone(), None, false, false)
            .await
            .unwrap()
    );
}

#[wasm_bindgen_test]
async fn bfa_mint_begin_reserves_inputs() {
    let (mut a, a_online) =
        funded_wallet(wallet_data(vec![AssetSchema::Bfa, AssetSchema::Ifa]), 5).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let bridge = new_evm_address();
    deploy_bridge(&bridge).await;
    let asset_id = a
        .issue_asset_bfa(s("BFA"), s("Bridged"), 0, 2, CHAIN_ID, s(&bridge), None)
        .unwrap()
        .asset_id;

    let rcv = mint_rcv(&mut b, &b_online, default_rcv_expiration()).await;
    let res = mint_begin(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&rcv, MINT_AMOUNT),
    )
    .await
    .unwrap();
    assert_eq!(
        transfer_status(&mut a, &rcv.recipient_id),
        TransferStatus::Initiated
    );
    let reserved = a.get_reserved_outpoints().unwrap();
    let psbt = Psbt::from_str(&res.psbt).unwrap();
    assert!(
        psbt.unsigned_tx
            .input
            .iter()
            .all(|i| reserved.contains(&i.previous_output.into())),
        "{reserved:?}"
    );

    // mint_end needs the EVM RPC URL of the chain
    let signed = a.sign_psbt(res.psbt.clone(), None).unwrap();
    let no_evm = a.go_online(go_online_options(false)).await.unwrap();
    let result = a.mint_end(no_evm, signed.clone()).await;
    assert!(
        matches!(result, Err(Error::NoEvmRpcUrl { chain_id: CHAIN_ID })),
        "{result:?}"
    );
    let a_online = a
        .go_online(OnlineOptions {
            evm_rpc_urls: evm_rpc_urls(),
            ..go_online_options(false)
        })
        .await
        .unwrap();

    assert!(
        a.fail_transfers(a_online.clone(), Some(res.batch_transfer_idx), false, false)
            .await
            .unwrap()
    );
    assert_eq!(
        transfer_status(&mut a, &rcv.recipient_id),
        TransferStatus::Failed
    );
    assert!(a.get_reserved_outpoints().unwrap().is_empty());
    let result = a.mint_end(a_online.clone(), signed).await;
    assert!(
        matches!(result, Err(Error::CannotEndTransfer { .. })),
        "{result:?}"
    );

    // a burn PSBT does not end a mint
    let ifa = a
        .issue_asset_ifa(s("IFA"), s("Inflatable"), 0, vec![100], vec![], None)
        .unwrap();
    let burn = a
        .burn_begin(a_online.clone(), ifa.asset_id, 10, None, 1, 1, true)
        .await
        .unwrap();
    let signed = a.sign_psbt(burn.psbt, None).unwrap();
    let result = a.mint_end(a_online.clone(), signed).await;
    assert!(
        matches!(result, Err(Error::WrongOperation { .. })),
        "{result:?}"
    );

    // a failed post releases the inputs
    let rcv = mint_rcv(&mut b, &b_online, default_rcv_expiration()).await;
    rpc_fail(PROXY_URL, "consignment.post");
    let result = mint_begin(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&rcv, MINT_AMOUNT),
    )
    .await;
    clear_rpc_hooks();
    assert!(result.is_err(), "{result:?}");
    assert_eq!(
        transfer_status(&mut a, &rcv.recipient_id),
        TransferStatus::Failed
    );
    assert!(a.get_reserved_outpoints().unwrap().is_empty());
}
