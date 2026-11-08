use std::collections::HashMap;
use wasm_bindgen_test::*;

use super::test_bfa::utils::*;
use super::test_bfa::*;
use crate::wallet::rust_only::validate_consignment_offchain;
use crate::wallet::{
    AssetFilter, MintBeginResult, Online, OnlineOptions, OperationResult, ReceiveData,
    ReceivedMint, Recipient, Wallet,
};
use crate::{
    AssetSchema, Assignment, BitcoinNetwork, Error, OS_ASSET, OS_MINT, RgbOutExt, TransferKind,
    TransferStatus, script_buf_from_recipient_id,
};
use amplify::ByteArray;
use bdk_wallet::bitcoin::Psbt;
use rgbstd::containers::LockId;
use rgbstd::{OpId, Opout, Txid as RgbTxid};
use serde_json::json;
use std::str::FromStr;

const MINT_AMOUNT: u64 = 100;

/// A blind receive of `MINT_AMOUNT`.
async fn mint_rcv(wallet: &mut Wallet, online: &Online) -> ReceiveData {
    blind_rcv(
        wallet,
        online,
        None,
        Assignment::Fungible(MINT_AMOUNT),
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap()
}

/// A mint recipient, with no expiration.
fn mint_recipient(receive: &ReceiveData, amount: u64) -> Recipient {
    Recipient {
        expiration_timestamp: None,
        ..recipient(&receive.recipient_id, amount, None, PROXY_URL)
    }
}

/// A mint recipient for a witness receive, with no expiration.
fn witness_mint_recipient(receive: &ReceiveData) -> Recipient {
    Recipient {
        expiration_timestamp: None,
        ..recipient(&receive.recipient_id, MINT_AMOUNT, Some(1000), PROXY_URL)
    }
}

async fn mint_begin(
    minter: &mut Wallet,
    minter_online: &Online,
    asset_id: &str,
    recipient: Recipient,
) -> Result<MintBeginResult, Error> {
    let before_block = evm_block_number().await + 1000;
    mint_begin_before(minter, minter_online, asset_id, recipient, before_block).await
}

async fn mint_begin_before(
    minter: &mut Wallet,
    minter_online: &Online,
    asset_id: &str,
    recipient: Recipient,
    before_block: u64,
) -> Result<MintBeginResult, Error> {
    minter
        .mint_begin(
            minter_online.clone(),
            asset_id.to_string(),
            recipient,
            before_block,
            1,
            1,
            transport_endpoint(PROXY_URL),
        )
        .await
}

fn mint_lock_id(res: &MintBeginResult) -> LockId {
    lock_id_of(&res.psbt, OpId::from_str(&res.details.opid).unwrap())
}

/// Lock the mint on the bridge and finalize the lock.
async fn lock_mint(bridge: &str, res: &MintBeginResult, amount: u64) {
    record_lock(bridge, &mint_lock_id(res), amount, evm_block_number().await).await;
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

    let receive = mint_rcv(&mut b, &b_online).await;
    spy_rpc();
    clear_rpc_hooks();
    let before_block = evm_block_number().await + 1000;
    let res = mint_begin_before(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&receive, MINT_AMOUNT),
        before_block,
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
    assert_eq!(
        ReceivedMint {
            bridge_contract: received.bridge_contract.to_lowercase(),
            ..received.clone()
        },
        ReceivedMint {
            opid: res.details.opid.clone(),
            lock_id: mint_lock_id(&res).to_string(),
            amount: MINT_AMOUNT,
            before_block,
            evm_chain_id: CHAIN_ID,
            bridge_contract: bridge.clone(),
        }
    );

    // a lock under the bare operation ID, or under the operation in another TX, does not confirm
    // the mint
    let opid = OpId::from_str(&res.details.opid).unwrap();
    let block = evm_block_number().await;
    let other_tx = RgbTxid::from_str(&"07".repeat(32)).unwrap();
    for lock_id in [
        LockId::from(opid.to_byte_array()),
        LockId::with(opid, other_tx),
    ] {
        record_lock(&bridge, &lock_id, MINT_AMOUNT, block).await;
    }
    finalize_evm().await;
    b.refresh(b_online.clone(), None, vec![], false)
        .await
        .unwrap();
    assert_eq!(
        transfer_status(&mut b, &receive.recipient_id),
        TransferStatus::WaitingCounterparty
    );

    // no lock under the lock ID yet, then a lock not finalized yet
    for lock in [false, true] {
        if lock {
            record_lock(
                &bridge,
                &mint_lock_id(&res),
                MINT_AMOUNT,
                evm_block_number().await,
            )
            .await;
        }
        let result = mint_end(&mut a, &a_online, &res).await;
        assert!(
            matches!(result, Err(Error::MintLockNotFound { ref opid }) if *opid == res.details.opid),
            "{result:?}"
        );
    }
    assert_eq!(
        transfer_status(&mut a, &receive.recipient_id),
        TransferStatus::Initiated
    );

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
    let res = mint_begin(
        &mut a,
        &a_online,
        &asset_id,
        witness_mint_recipient(&receive),
    )
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
    let rcv = mint_rcv(&mut b, &b_online).await;

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

    // a mint recipient has no expiration
    let bfa = a
        .issue_asset_bfa(s("BFA"), s("Bridged"), 0, 2, CHAIN_ID, s(&bridge), None)
        .unwrap();
    let id = &bfa.asset_id;
    let recipient = recipient(&rcv.recipient_id, 100, None, PROXY_URL);
    let result = mint_begin(&mut a, &a_online, id, recipient).await;
    assert!(
        matches!(result, Err(Error::InvalidRecipientData { ref details }) if details.contains("cannot have an expiration")),
        "{result:?}"
    );

    // the block the lock has to be included before leaves a block for it, and is within the
    // configured bounds
    let (mut c, c_online) =
        funded_wallet_with(wallet_data(vec![AssetSchema::Bfa]), 5, 10, 100).await;
    let bfa_c = c
        .issue_asset_bfa(s("BFA"), s("Bridged"), 0, 2, CHAIN_ID, s(&bridge), None)
        .unwrap();
    for blocks in [1, 5, 101] {
        let before_block = evm_block_number().await + blocks;
        let result = mint_begin_before(
            &mut c,
            &c_online,
            &bfa_c.asset_id,
            mint_recipient(&rcv, 100),
            before_block,
        )
        .await;
        assert!(
            matches!(result, Err(Error::InvalidBeforeBlock { .. })),
            "{blocks}: {result:?}"
        );
    }

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
    let receive = mint_rcv(&mut c, &c_online).await;
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
    opouts_holding(wallet, asset_id, &Assignment::MintRight)
        .into_iter()
        .map(|opouts| opouts.into_iter().filter(|o| o.ty == OS_MINT).collect())
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
    let rcv = mint_rcv(&mut b, &b_online).await;
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

    // with no EVM RPC URL for the chain, the receiver waits and checks the mint later
    let rcv = mint_rcv(&mut b, &b_online).await;
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
            evm_chains: evm_chains(2, 1_000_000),
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

    // a checked mint is not failed by its expiration, even with no EVM RPC URL
    let now = crate::utils::now().unix_timestamp();
    edit_batch_transfer(&mut b, rcv.batch_transfer_idx, "expiration", now - 1000);
    let no_evm = b.go_online(go_online_options(false)).await.unwrap();
    b.refresh(no_evm.clone(), None, vec![], false)
        .await
        .unwrap();
    assert!(!b.fail_transfers(no_evm, None, false, false).await.unwrap());
    assert_eq!(
        transfer_status(&mut b, &rcv.recipient_id),
        TransferStatus::WaitingCounterparty
    );
}

/// Begin a mint to a blinded UTXO of `receiver` and let the receiver check it.
async fn begin_checked_mint(
    minter: &mut Wallet,
    minter_online: &Online,
    receiver: &mut Wallet,
    receiver_online: &Online,
    asset_id: &str,
    before_block: u64,
) -> (MintBeginResult, ReceiveData) {
    let receive = mint_rcv(receiver, receiver_online).await;
    let res = mint_begin_before(
        minter,
        minter_online,
        asset_id,
        mint_recipient(&receive, MINT_AMOUNT),
        before_block,
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

    let rcv = mint_rcv(&mut b, &b_online).await;
    let before_block = evm_block_number().await + 3;
    let res = mint_begin_before(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&rcv, MINT_AMOUNT),
        before_block,
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
            evm_chains: evm_chains(2, 1_000_000),
            ..go_online_options(false)
        })
        .await
        .unwrap();

    // a mint is failed only once its lock has expired
    let result = a
        .fail_transfers(a_online.clone(), Some(res.batch_transfer_idx), false, false)
        .await;
    assert!(
        matches!(result, Err(Error::CannotFailBatchTransfer)),
        "{result:?}"
    );
    finalize_evm().await;
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
    let rcv = mint_rcv(&mut b, &b_online).await;
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

/// A minter with a BFA of `rights` mint rights on a new bridge, and a receiver.
async fn minter_and_receiver(
    rights: u8,
    receiver_utxos: u8,
    reject_list: Option<String>,
) -> (Wallet, Online, Wallet, Online, String, String) {
    let (a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), rights + 1).await;
    let (b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), receiver_utxos).await;
    let bridge = new_evm_address();
    deploy_bridge(&bridge).await;
    let asset_id = a
        .issue_asset_bfa(
            s("BFA"),
            s("Bridged"),
            0,
            rights,
            CHAIN_ID,
            s(&bridge),
            reject_list,
        )
        .unwrap()
        .asset_id;
    (a, a_online, b, b_online, bridge, asset_id)
}

#[wasm_bindgen_test]
async fn bfa_mint_lock_before_block() {
    let (mut a, a_online, mut b, b_online, bridge, asset_id) =
        minter_and_receiver(4, 3, None).await;

    // a lock in the block the mint commits to expires it on both sides
    let before_block = evm_block_number().await + 3;
    let (res, receive) = begin_checked_mint(
        &mut a,
        &a_online,
        &mut b,
        &b_online,
        &asset_id,
        before_block,
    )
    .await;
    evm_mine(5).await;
    record_lock(&bridge, &mint_lock_id(&res), MINT_AMOUNT, before_block).await;
    finalize_evm().await;
    b.refresh(b_online.clone(), None, vec![], false)
        .await
        .unwrap();
    assert_eq!(
        transfer_status(&mut b, &receive.recipient_id),
        TransferStatus::Failed
    );
    let result = mint_end(&mut a, &a_online, &res).await;
    assert!(
        matches!(result, Err(Error::MintLockExpired { ref opid, before_block: block })
            if *opid == res.details.opid && block == before_block),
        "{result:?}"
    );
    assert_eq!(
        transfer_status(&mut a, &receive.recipient_id),
        TransferStatus::Failed
    );
    let result = mint_end(&mut a, &a_online, &res).await;
    assert!(
        matches!(result, Err(Error::CannotEndTransfer { .. })),
        "{result:?}"
    );

    // no lock before the block: the receiver fails the mint once the blocks before it are
    // finalized, the sender when asked to
    let before_block = evm_block_number().await + 3;
    let (res, receive) = begin_checked_mint(
        &mut a,
        &a_online,
        &mut b,
        &b_online,
        &asset_id,
        before_block,
    )
    .await;
    assert_eq!(
        transfer_status(&mut b, &receive.recipient_id),
        TransferStatus::WaitingCounterparty
    );
    evm_mine(3).await;
    finalize_evm().await;
    b.refresh(b_online.clone(), None, vec![], false)
        .await
        .unwrap();
    assert_eq!(
        transfer_status(&mut b, &receive.recipient_id),
        TransferStatus::Failed
    );
    a.refresh(a_online.clone(), None, vec![], false)
        .await
        .unwrap();
    assert_eq!(
        transfer_status(&mut a, &receive.recipient_id),
        TransferStatus::Initiated
    );
    // a mint is of an asset, so no_asset_only keeps it
    let result = a
        .fail_transfers(a_online.clone(), Some(res.batch_transfer_idx), true, false)
        .await;
    assert!(
        matches!(result, Err(Error::CannotFailBatchTransfer)),
        "{result:?}"
    );
    assert_eq!(
        transfer_status(&mut a, &receive.recipient_id),
        TransferStatus::Initiated
    );
    assert!(
        a.fail_transfers(a_online.clone(), Some(res.batch_transfer_idx), false, false)
            .await
            .unwrap()
    );
    assert_eq!(
        transfer_status(&mut a, &receive.recipient_id),
        TransferStatus::Failed
    );

    // fail_transfers fails a sent mint only once its lock has expired
    let receive = mint_rcv(&mut b, &b_online).await;
    let before_block = evm_block_number().await + 3;
    mint_begin_before(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&receive, MINT_AMOUNT),
        before_block,
    )
    .await
    .unwrap();
    assert!(
        !a.fail_transfers(a_online.clone(), None, false, false)
            .await
            .unwrap()
    );
    assert_eq!(
        transfer_status(&mut a, &receive.recipient_id),
        TransferStatus::Initiated
    );
    evm_mine(3).await;
    finalize_evm().await;
    assert!(
        !a.fail_transfers(a_online.clone(), None, true, false)
            .await
            .unwrap()
    );
    assert_eq!(
        transfer_status(&mut a, &receive.recipient_id),
        TransferStatus::Initiated
    );
    assert!(
        a.fail_transfers(a_online.clone(), None, false, false)
            .await
            .unwrap()
    );
    assert_eq!(
        transfer_status(&mut a, &receive.recipient_id),
        TransferStatus::Failed
    );
}

#[wasm_bindgen_test]
async fn bfa_mint_before_block_too_far() {
    let (mut a, a_online, _, _, bridge, asset_id) = minter_and_receiver(4, 1, None).await;
    let (mut b, b_online) = funded_wallet_with(wallet_data(vec![AssetSchema::Bfa]), 3, 2, 50).await;

    // a mint committing to a block further than the receiver accepts is refused
    let receive = mint_rcv(&mut b, &b_online).await;
    let before_block = evm_block_number().await + 100;
    mint_begin_before(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&receive, MINT_AMOUNT),
        before_block,
    )
    .await
    .unwrap();
    b.refresh(b_online.clone(), None, vec![], false)
        .await
        .unwrap();
    assert_eq!(
        transfer_status(&mut b, &receive.recipient_id),
        TransferStatus::Failed
    );

    // a closer block is accepted
    let before_block = evm_block_number().await + 25;
    let (_, receive) = begin_checked_mint(
        &mut a,
        &a_online,
        &mut b,
        &b_online,
        &asset_id,
        before_block,
    )
    .await;
    assert_eq!(
        b.get_received_mint(receive.recipient_id)
            .unwrap()
            .unwrap()
            .before_block,
        before_block
    );

    // a further block is accepted when the lock is already recorded
    let receive = mint_rcv(&mut b, &b_online).await;
    let before_block = evm_block_number().await + 100;
    let res = mint_begin_before(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&receive, MINT_AMOUNT),
        before_block,
    )
    .await
    .unwrap();
    record_lock(
        &bridge,
        &mint_lock_id(&res),
        MINT_AMOUNT,
        evm_block_number().await,
    )
    .await;
    b.refresh(b_online.clone(), None, vec![], false)
        .await
        .unwrap();
    assert!(
        b.get_received_mint(receive.recipient_id.clone())
            .unwrap()
            .is_some()
    );
    finalize_evm().await;
    mint_end(&mut a, &a_online, &res).await.unwrap();
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
}

#[wasm_bindgen_test]
async fn bfa_mint_reject_list_not_repeated() {
    let reject_list = new_evm_address();
    deploy_reject_list(&reject_list, &[]).await;
    let (mut a, a_online, mut b, b_online, bridge, asset_id) =
        minter_and_receiver(3, 3, Some(reject_list.clone())).await;

    // a checked mint is not refused by a later reject list
    let rights = mint_right_opouts(&mut a, &asset_id).concat();
    let before_block = evm_block_number().await + 1000;
    let (res, _) = begin_checked_mint(
        &mut a,
        &a_online,
        &mut b,
        &b_online,
        &asset_id,
        before_block,
    )
    .await;
    deploy_reject_list_opouts(&reject_list, &rights).await;
    lock_mint(&bridge, &res, MINT_AMOUNT).await;
    mint_end(&mut a, &a_online, &res).await.unwrap();
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));

    // a mint whose lock is finalized before the first check is not refused either. The list only
    // grows, so this is another asset with another list
    let reject_list = new_evm_address();
    deploy_reject_list(&reject_list, &[]).await;
    let asset_id = a
        .issue_asset_bfa(
            s("BFA"),
            s("Bridged"),
            0,
            2,
            CHAIN_ID,
            s(&bridge),
            Some(reject_list.clone()),
        )
        .unwrap()
        .asset_id;
    let rights = mint_right_opouts(&mut a, &asset_id).concat();
    let receive = mint_rcv(&mut b, &b_online).await;
    let res = mint_begin(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&receive, MINT_AMOUNT),
    )
    .await
    .unwrap();
    lock_mint(&bridge, &res, MINT_AMOUNT).await;
    deploy_reject_list_opouts(&reject_list, &rights).await;
    finalize_evm().await;
    b.refresh(b_online.clone(), None, vec![], false)
        .await
        .unwrap();
    assert_ne!(
        transfer_status(&mut b, &receive.recipient_id),
        TransferStatus::Failed
    );
    mint_end(&mut a, &a_online, &res).await.unwrap();
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
    assert_eq!(b.get_asset_balance(asset_id).unwrap().settled, MINT_AMOUNT);

    // nor is a mint whose lock is recorded but not finalized before the first check. An issue
    // needs empty UTXOs
    let unsigned = a
        .create_utxos_begin(a_online.clone(), false, Some(2), None, 1, true)
        .await
        .unwrap();
    let signed = a.sign_psbt(unsigned, None).unwrap();
    a.create_utxos_end(a_online.clone(), signed, true)
        .await
        .unwrap();
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    a.sync(a_online.clone()).await.unwrap();
    let reject_list = new_evm_address();
    deploy_reject_list(&reject_list, &[]).await;
    let asset_id = a
        .issue_asset_bfa(
            s("BFA"),
            s("Bridged"),
            0,
            2,
            CHAIN_ID,
            s(&bridge),
            Some(reject_list.clone()),
        )
        .unwrap()
        .asset_id;
    let rights = mint_right_opouts(&mut a, &asset_id).concat();
    let receive = mint_rcv(&mut b, &b_online).await;
    let res = mint_begin(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&receive, MINT_AMOUNT),
    )
    .await
    .unwrap();
    deploy_reject_list_opouts(&reject_list, &rights).await;
    finalize_evm().await;
    record_lock(
        &bridge,
        &mint_lock_id(&res),
        MINT_AMOUNT,
        evm_block_number().await,
    )
    .await;
    b.refresh(b_online.clone(), None, vec![], false)
        .await
        .unwrap();
    assert_eq!(
        transfer_status(&mut b, &receive.recipient_id),
        TransferStatus::WaitingCounterparty
    );
    assert!(
        b.get_received_mint(receive.recipient_id.clone())
            .unwrap()
            .is_some()
    );
    finalize_evm().await;
    mint_end(&mut a, &a_online, &res).await.unwrap();
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
    assert_eq!(b.get_asset_balance(asset_id).unwrap().settled, MINT_AMOUNT);
}

#[wasm_bindgen_test]
async fn bfa_mint_never_failed_by_clock() {
    let (mut a, a_online, mut b, b_online, bridge, asset_id) =
        minter_and_receiver(3, 2, None).await;
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
    let res = mint_begin(
        &mut a,
        &a_online,
        &asset_id,
        witness_mint_recipient(&receive),
    )
    .await
    .unwrap();
    record_lock(
        &bridge,
        &mint_lock_id(&res),
        MINT_AMOUNT,
        evm_block_number().await,
    )
    .await;

    // past the invoice expiration, failing the expired transfers checks the mint and does not
    // fail it, on either side
    let now = crate::utils::now().unix_timestamp();
    edit_batch_transfer(&mut b, receive.batch_transfer_idx, "expiration", now - 1000);
    edit_batch_transfer(&mut a, res.batch_transfer_idx, "expiration", now - 1000);
    assert!(
        b.get_received_mint(receive.recipient_id.clone())
            .unwrap()
            .is_none()
    );
    assert!(
        !b.fail_transfers(b_online.clone(), None, false, false)
            .await
            .unwrap()
    );
    assert!(
        b.get_received_mint(receive.recipient_id.clone())
            .unwrap()
            .is_some()
    );
    assert_eq!(
        transfer_status(&mut b, &receive.recipient_id),
        TransferStatus::WaitingCounterparty
    );
    let result = b
        .fail_transfers(
            b_online.clone(),
            Some(receive.batch_transfer_idx),
            false,
            false,
        )
        .await;
    assert!(
        matches!(result, Err(Error::CannotFailBatchTransfer)),
        "{result:?}"
    );
    assert!(
        !a.fail_transfers(a_online.clone(), None, false, false)
            .await
            .unwrap()
    );
    assert_eq!(
        transfer_status(&mut a, &receive.recipient_id),
        TransferStatus::Initiated
    );
    finalize_evm().await;
    mint_end(&mut a, &a_online, &res).await.unwrap();
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
    assert_eq!(
        transfer_status(&mut a, &receive.recipient_id),
        TransferStatus::Settled
    );

    // failing an unchecked mint checks it and is denied
    let receive = mint_rcv(&mut b, &b_online).await;
    let res = mint_begin(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&receive, MINT_AMOUNT),
    )
    .await
    .unwrap();
    record_lock(
        &bridge,
        &mint_lock_id(&res),
        MINT_AMOUNT,
        evm_block_number().await,
    )
    .await;
    let result = b
        .fail_transfers(
            b_online.clone(),
            Some(receive.batch_transfer_idx),
            false,
            false,
        )
        .await;
    assert!(
        matches!(result, Err(Error::CannotFailBatchTransfer)),
        "{result:?}"
    );
    assert!(
        b.get_received_mint(receive.recipient_id.clone())
            .unwrap()
            .is_some()
    );
    assert_eq!(
        transfer_status(&mut b, &receive.recipient_id),
        TransferStatus::WaitingCounterparty
    );
    finalize_evm().await;
    mint_end(&mut a, &a_online, &res).await.unwrap();
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));

    // a receive with no mint expires with its invoice
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
    edit_batch_transfer(&mut b, receive.batch_transfer_idx, "expiration", now - 1000);
    assert!(
        b.fail_transfers(b_online.clone(), None, false, false)
            .await
            .unwrap()
    );
    assert_eq!(
        transfer_status(&mut b, &receive.recipient_id),
        TransferStatus::Failed
    );
}
