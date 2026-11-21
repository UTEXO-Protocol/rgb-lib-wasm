use std::collections::HashMap;
use wasm_bindgen_test::*;

use super::test_bfa::utils::*;
use super::test_bfa::*;
use super::test_send::{ResetMocks, serve_media, test_media, validations, witness_ord};
use crate::no_wallet::{
    BundleFilter, Invoice, RgbBridgeLocationInfo, RgbEvmContractInfo, TransitionGlobal,
    TransitionMetadata, inspect_consignment,
};
use crate::wallet::offline::MOCK_ISSUE_MEDIA;
use crate::wallet::offline::address_reuse_tests::{assert_no_partial_change, state};
use crate::wallet::online::{
    MOCK_MINT_KEPT_AMOUNT, RECEIVED_CONSIGNMENT_VALIDATIONS, paying_mint_opid, unsupported_claims,
};
use crate::wallet::rust_only::validate_consignment_offchain;
use crate::wallet::{
    AssetFilter, Balance, MintBeginResult, Online, OnlineOptions, OperationResult, ReceiveData,
    ReceivedMint, Recipient, Wallet,
};
use crate::{
    AssetSchema, Assignment, BitcoinNetwork, Encryption, Error, FileContent, OS_ASSET, OS_MINT,
    RgbTransfer, TransferKind, TransferStatus, TypeOfTransition, ValidationConfig,
    script_buf_from_recipient_id,
};
use amplify::ByteArray;
use bdk_wallet::KeychainKind;
use bdk_wallet::bitcoin::{FeeRate, Psbt};
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
        .issue_asset_bfa(
            s("BFA"),
            s("Bridged"),
            0,
            3,
            CHAIN_ID,
            s(&bridge),
            None,
            false,
            None,
            false,
        )
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

    let txid = psbt.unsigned_tx.compute_txid().to_string();
    let bytes = a.send_consignment_bytes(&txid, &asset_id).unwrap();

    // the mint consignment shows what is minted before the lock
    let inspection = inspect_consignment(&bytes, BundleFilter::Terminals).unwrap();
    assert_eq!(inspection.asset_id, asset_id);
    assert_eq!(inspection.asset_schema, AssetSchema::Bfa);
    assert_eq!(
        inspection.genesis.bridge_location,
        Some(RgbBridgeLocationInfo::Evm(RgbEvmContractInfo {
            chain_id: CHAIN_ID,
            address: bridge.to_lowercase(),
        }))
    );
    assert_eq!(inspection.witnesses.len(), 1);
    let witness = &inspection.witnesses[0];
    assert_eq!(witness.txid, txid);
    assert!(!witness.has_concealed_transitions());
    let mints: Vec<_> = witness
        .operations
        .iter()
        .flat_map(|o| &o.transitions)
        .filter(|t| t.r#type == TypeOfTransition::Mint)
        .collect();
    assert_eq!(mints.len(), 1);
    let mint = mints[0];
    assert_eq!(mint.opid, res.details.opid);
    let inputs: Vec<_> = mint.inputs.iter().map(|i| &i.assignment).collect();
    assert_eq!(inputs, [&Some(Assignment::MintRight)]);
    assert_eq!(
        mint.metadata,
        [TransitionMetadata::BeforeBlock(before_block)]
    );
    assert_eq!(mint.globals, [TransitionGlobal::IssuedSupply(MINT_AMOUNT)]);
    let outputs: Vec<_> = mint
        .outputs
        .iter()
        .filter(|o| o.assignment == Assignment::Fungible(MINT_AMOUNT))
        .collect();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].recipient_id, Some(receive.recipient_id.clone()));

    // only the mint that creates the received allocation pays it, and only on a schema with mint
    let consignment = RgbTransfer::load(&bytes[..]).unwrap();
    let bundle = consignment
        .bundles
        .iter()
        .find(|ab| ab.witness_id().to_string() == txid)
        .unwrap();
    let opid = OpId::from_str(&res.details.opid).unwrap();
    assert_eq!(
        paying_mint_opid(AssetSchema::Bfa, bundle, [opid]),
        Some(opid)
    );
    assert_eq!(
        paying_mint_opid(AssetSchema::Bfa, bundle, [OpId::from([0xaa; 32])]),
        None
    );
    assert_eq!(paying_mint_opid(AssetSchema::Nia, bundle, [opid]), None);
    let mut pending = consignment
        .clone()
        .validate_deterministic(
            &AssetSchema::Bfa.schema_rules(),
            &ValidationConfig {
                chain_net: a.chain_net(),
                build_ops_dag: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!unsupported_claims(AssetSchema::Bfa, &mut pending));
    assert!(unsupported_claims(AssetSchema::Nia, &mut pending));
    // an import of the mint needs an EVM RPC URL and a confirmed lock
    let mut d1 = Wallet::new(wallet_data(vec![AssetSchema::Bfa])).unwrap();
    let d1_online = d1.go_online(go_online_options(false)).await.unwrap();
    assert!(matches!(
        d1.save_new_asset(d1_online, &bytes, &txid).await,
        Err(Error::NoEvmRpcUrl { chain_id: CHAIN_ID })
    ));
    let mut d2 = Wallet::new(wallet_data(vec![AssetSchema::Bfa])).unwrap();
    let d2_online = d2
        .go_online(OnlineOptions {
            evm_chains: evm_chains(2, 1_000_000),
            ..go_online_options(false)
        })
        .await
        .unwrap();

    // the receiver checks the mint before the lock and tells what to lock
    assert!(
        b.get_received_mint(receive.recipient_id.clone())
            .unwrap()
            .is_none()
    );
    b.backup("pw").unwrap();
    assert!(!b.backup_info().unwrap());
    b.refresh(b_online.clone(), None, vec![], false)
        .await
        .unwrap();
    let received = b
        .get_received_mint(receive.recipient_id.clone())
        .unwrap()
        .unwrap();
    // the check keeps the transfer waiting and marks a backup as required
    assert_eq!(
        transfer_status(&mut b, &receive.recipient_id),
        TransferStatus::WaitingCounterparty
    );
    assert!(b.backup_info().unwrap());
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
        let result = d2.save_new_asset(d2_online.clone(), &bytes, &txid).await;
        assert!(
            matches!(result, Err(Error::MintLockNotFound { ref opid }) if *opid == res.details.opid),
            "{result:?}"
        );
    }
    assert!(d2.rgb_contract_ids().unwrap().is_empty());
    assert_eq!(
        transfer_status(&mut a, &receive.recipient_id),
        TransferStatus::Initiated
    );

    // the test bridge gives the word of a zero lock ID and amount as its circulating supply
    record_lock(&bridge, &LockId::from([0u8; 32]), 0, MINT_AMOUNT - 1).await;
    finalize_evm().await;
    let minted = mint_end(&mut a, &a_online, &res).await.unwrap();
    assert!(witness_ord(&a, &txid).is_some());
    assert_eq!(minted.batch_transfer_idx, res.batch_transfer_idx);
    assert_eq!(minted.entropy, res.details.entropy);
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
    for wallet in [&a, &b] {
        let metadata = wallet.get_asset_metadata(asset_id.clone()).unwrap();
        assert_eq!(metadata.known_circulating_supply, MINT_AMOUNT);
    }

    // a mint posts its consignment and makes no ACK call
    let calls = (
        rpc_count("consignment.post"),
        rpc_count("ack.prepare"),
        rpc_count("ack.post"),
    );
    clear_rpc_hooks();
    assert_eq!(calls, (1, 0, 0));

    // an EVM node on another chain cannot confirm the lock, then the import succeeds
    rpc_hook(
        EVM_RPC_URL,
        "eth_chainId",
        &format!("return {{ ...res, result: '0x{:x}' }}", CHAIN_ID + 1),
    );
    let result = d2.save_new_asset(d2_online.clone(), &bytes, &txid).await;
    clear_rpc_hooks();
    assert!(
        matches!(result, Err(Error::Network { ref details }) if details.contains("another chain")),
        "{result:?}"
    );
    assert!(d2.rgb_contract_ids().unwrap().is_empty());
    d2.save_new_asset(d2_online, &bytes, &txid).await.unwrap();
    let listed = d2.list_assets(vec![AssetSchema::Bfa]).unwrap().bfa.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].asset_id, asset_id);
    assert_eq!(
        listed[0].balance,
        Balance {
            settled: 0,
            future: 0,
            spendable: 0
        }
    );

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
    assert_eq!(mint_rights(&mut a, &asset_id).await, 2);
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
    for _ in 0..3 {
        b.full_sync(b_online.clone()).await.unwrap();
        refresh(&mut b, &b_online).await;
        if transfer_status(&mut b, &receive.recipient_id) == TransferStatus::WaitingConfirmations {
            break;
        }
    }
    assert_eq!(
        transfer_status(&mut b, &receive.recipient_id),
        TransferStatus::WaitingConfirmations
    );

    // after a reload, settlement checks the lock again: with no EVM RPC URL it waits
    let mut b = Wallet::restore(b.get_wallet_data()).await.unwrap();
    let no_evm = b.go_online(go_online_options(false)).await.unwrap();
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    b.full_sync(no_evm.clone()).await.unwrap();
    let refreshed = b.refresh(no_evm, None, vec![], false).await.unwrap();
    assert!(
        refreshed.values().all(|t| t.failure.is_none()),
        "{refreshed:?}"
    );
    assert_eq!(
        transfer_status(&mut b, &receive.recipient_id),
        TransferStatus::WaitingConfirmations
    );
    let b_online = b
        .go_online(OnlineOptions {
            evm_chains: evm_chains(2, 1_000_000),
            ..go_online_options(false)
        })
        .await
        .unwrap();
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
    assert_eq!(
        b.get_asset_balance(asset_id).unwrap().settled,
        70 + MINT_AMOUNT
    );
}

#[wasm_bindgen_test]
async fn bfa_mint_begin_refusals() {
    const DOWN_URL: &str = "http://127.0.0.1:1/json-rpc";
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 5).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let bridge = new_evm_address();
    let rcv = mint_rcv(&mut b, &b_online).await;

    // the last mint right is never spent
    let single = a
        .issue_asset_bfa(
            s("BFA"),
            s("Bridged"),
            0,
            1,
            CHAIN_ID,
            s(&bridge),
            None,
            false,
            None,
            false,
        )
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
        .issue_asset_bfa(
            s("BFA"),
            s("Bridged"),
            0,
            2,
            CHAIN_ID,
            s(&bridge),
            None,
            false,
            None,
            false,
        )
        .unwrap();
    let id = &bfa.asset_id;
    let mut rcpt = recipient(&rcv.recipient_id, 100, None, PROXY_URL);
    // the checks that need no network come before any proxy call
    rcpt.transport_endpoints = vec![transport_endpoint(DOWN_URL)];
    let result = mint_begin(&mut a, &a_online, id, rcpt.clone()).await;
    assert!(
        matches!(result, Err(Error::InvalidRecipientData { ref details }) if details.contains("cannot have an expiration")),
        "{result:?}"
    );
    // mint contacts the recipient proxy
    rcpt.expiration_timestamp = None;
    let result = mint_begin(&mut a, &a_online, id, rcpt).await;
    assert!(
        matches!(result, Err(Error::InvalidTransportEndpoints { ref details }) if details == "no valid transport endpoints"),
        "{result:?}"
    );

    // the block the lock has to be included before leaves a block for it, and is within the
    // configured bounds
    let (mut c, c_online) =
        funded_wallet_with(wallet_data(vec![AssetSchema::Bfa]), 5, None, 10, 100).await;
    let bfa_c = c
        .issue_asset_bfa(
            s("BFA"),
            s("Bridged"),
            0,
            2,
            CHAIN_ID,
            s(&bridge),
            None,
            false,
            None,
            false,
        )
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
            false,
            None,
            false,
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
        .await
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
async fn mint_right_opouts(wallet: &mut Wallet, asset_id: &str) -> Vec<Vec<Opout>> {
    opouts_holding(wallet, asset_id, &Assignment::MintRight)
        .await
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
            false,
            None,
            false,
        )
        .unwrap()
        .asset_id;

    // each mint right is on its own UTXO
    let rights = mint_right_opouts(&mut a, &asset_id).await;
    assert_eq!(rights.len(), 4);
    assert!(rights.iter().all(|opouts| opouts.len() == 1));

    // the list rejects the first 2 mint rights to be selected: the mint retries with the next one
    // and moves the rejected rights, each to its own UTXO
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
    deploy_reject_list(&reject_list, &[]).await;
    finalize_evm().await;
    let rights = mint_right_opouts(&mut a, &asset_id).await;
    assert_eq!(rights.len(), 3);
    assert!(rights.iter().all(|opouts| opouts.len() == 1));

    // the list rejects another mint right: the mint moves the rejected right, and each UTXO still
    // holds one mint right
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
    let rights = mint_right_opouts(&mut a, &asset_id).await;
    assert_eq!(rights.len(), 2);
    assert!(rights.iter().all(|opouts| opouts.len() == 1));
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
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 6).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 2).await;
    let bridge = new_evm_address();
    deploy_bridge(&bridge).await;
    let asset_id = a
        .issue_asset_bfa(
            s("BFA"),
            s("Bridged"),
            0,
            5,
            CHAIN_ID,
            s(&bridge),
            None,
            false,
            None,
            false,
        )
        .unwrap()
        .asset_id;
    let no_evm = b.go_online(go_online_options(false)).await.unwrap();

    // a mint of another amount than the requested one is refused before the lock, with no NACK
    let rcv = mint_rcv(&mut b, &b_online).await;
    mint_begin(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&rcv, MINT_AMOUNT / 2),
    )
    .await
    .unwrap();
    spy_rpc();
    clear_rpc_hooks();
    b.refresh(no_evm.clone(), None, vec![], false)
        .await
        .unwrap();
    assert_eq!(
        transfer_status(&mut b, &rcv.recipient_id),
        TransferStatus::Failed
    );
    assert!(b.get_received_mint(rcv.recipient_id).unwrap().is_none());
    assert_eq!(rpc_count("ack.post"), 0);

    // a mint keeping part of the amount minted is refused before the lock, with no NACK
    let rcv = mint_rcv(&mut b, &b_online).await;
    MOCK_MINT_KEPT_AMOUNT.set(Some(MINT_AMOUNT));
    mint_begin(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&rcv, MINT_AMOUNT),
    )
    .await
    .unwrap();
    assert!(MOCK_MINT_KEPT_AMOUNT.take().is_none());
    clear_rpc_hooks();
    b.refresh(no_evm.clone(), None, vec![], false)
        .await
        .unwrap();
    assert_eq!(
        transfer_status(&mut b, &rcv.recipient_id),
        TransferStatus::Failed
    );
    assert!(b.get_received_mint(rcv.recipient_id).unwrap().is_none());
    assert_eq!(rpc_count("ack.post"), 0);
    clear_rpc_hooks();

    // with no EVM RPC URL for the chain, the receiver waits, fetches no witness and checks the
    // mint later
    let rcv = mint_rcv(&mut b, &b_online).await;
    let res = mint_begin(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&rcv, MINT_AMOUNT),
    )
    .await
    .unwrap();
    let txid = Psbt::from_str(&res.psbt)
        .unwrap()
        .unsigned_tx
        .compute_txid()
        .to_string();
    run_js(&format!(
        "const g = globalThis;
        g.__witnessFetch = g.fetch;
        g.__witnessFetches = 0;
        const fetch = g.fetch;
        g.fetch = function (input, init) {{
            const url = input instanceof Request ? input.url : String(input);
            if (url.includes('/tx/{txid}')) {{
                g.__witnessFetches += 1;
                return Promise.reject(new TypeError('injected failure'));
            }}
            return fetch.call(this, input, init);
        }};"
    ));
    let r = b.refresh(no_evm, None, vec![], false).await;
    let fetches = run_js(
        "const g = globalThis;
        g.fetch = g.__witnessFetch;
        delete g.__witnessFetch;
        return g.__witnessFetches;",
    )
    .as_f64()
    .unwrap();
    let r = r.unwrap();
    assert!(r[&rcv.batch_transfer_idx].failure.is_none(), "{r:?}");
    assert_eq!(fetches, 0.0);
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

// a mint that puts more than u64::MAX of the asset on the UTXO is refused before the lock
#[wasm_bindgen_test]
async fn bfa_mint_past_u64_on_utxo_refused() {
    let (mut a, a_online, mut b, b_online, bridge, asset_id) =
        minter_and_receiver(3, 1, None).await;
    let (first, _) = mint(
        &mut a,
        &a_online,
        &mut b,
        &b_online,
        &asset_id,
        &bridge,
        MINT_AMOUNT,
    )
    .await;
    let rcv = blind_rcv(
        &mut b,
        &b_online,
        None,
        Assignment::Fungible(u64::MAX),
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap();
    assert_eq!(
        transfer_of(&b, &first).receive_utxo,
        transfer_of(&b, &rcv.recipient_id).receive_utxo
    );
    mint_begin(&mut a, &a_online, &asset_id, mint_recipient(&rcv, u64::MAX))
        .await
        .unwrap();
    b.refresh(b_online.clone(), None, vec![], false)
        .await
        .unwrap();
    assert_eq!(
        transfer_status(&mut b, &rcv.recipient_id),
        TransferStatus::Failed
    );
    assert!(
        b.get_received_mint(rcv.recipient_id.clone())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        b.get_asset_balance(asset_id.clone()).unwrap().settled,
        MINT_AMOUNT
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
        .issue_asset_bfa(
            s("BFA"),
            s("Bridged"),
            0,
            2,
            CHAIN_ID,
            s(&bridge),
            None,
            false,
            None,
            false,
        )
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
    let txid = psbt.unsigned_tx.compute_txid().to_string();
    assert_eq!(witness_ord(&a, &txid), None);
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
    assert_eq!(witness_ord(&a, &txid), None);
    let result = a.mint_end(a_online.clone(), signed).await;
    assert!(
        matches!(result, Err(Error::CannotEndTransfer { .. })),
        "{result:?}"
    );

    // a burn PSBT does not end a mint
    let ifa = a
        .issue_asset_ifa(
            s("IFA"),
            s("Inflatable"),
            0,
            vec![100],
            vec![],
            None,
            false,
            None,
            false,
        )
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

    // a failed post leaves no transfer
    let rcv = mint_rcv(&mut b, &b_online).await;
    let before = state(&a);
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
    assert_eq!(state(&a), before);
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
            false,
            None,
            false,
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
    let mut copy = Wallet::restore(a.get_wallet_data()).await.unwrap();
    assert_eq!(
        transfer_status(&mut copy, &receive.recipient_id),
        TransferStatus::Failed
    );
    assert!(copy.get_reserved_outpoints().unwrap().is_empty());
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
    a.backup("pw").unwrap();
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
    assert!(!a.backup_info().unwrap());
    assert!(
        a.fail_transfers(a_online.clone(), Some(res.batch_transfer_idx), false, false)
            .await
            .unwrap()
    );
    assert_eq!(
        transfer_status(&mut a, &receive.recipient_id),
        TransferStatus::Failed
    );
    assert!(a.backup_info().unwrap());

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
    let (mut b, b_online) =
        funded_wallet_with(wallet_data(vec![AssetSchema::Bfa]), 3, None, 2, 50).await;

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
    let rights = mint_right_opouts(&mut a, &asset_id).await.concat();
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
            false,
            None,
            false,
        )
        .unwrap()
        .asset_id;
    let rights = mint_right_opouts(&mut a, &asset_id).await.concat();
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
        .create_utxos_begin(a_online.clone(), false, Some(2), None, 1, true, false)
        .await
        .unwrap();
    let signed = a.sign_psbt(unsigned, None).unwrap();
    a.create_utxos_end(a_online.clone(), signed).await.unwrap();
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    a.full_sync(a_online.clone()).await.unwrap();
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
            false,
            None,
            false,
        )
        .unwrap()
        .asset_id;
    let rights = mint_right_opouts(&mut a, &asset_id).await.concat();
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
    assert!(
        b.get_received_mint(receive.recipient_id.clone())
            .unwrap()
            .is_none()
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
        b.get_received_mint(receive.recipient_id.clone())
            .unwrap()
            .is_some()
    );
    let copy = Wallet::restore(b.get_wallet_data()).await.unwrap();
    assert!(
        copy.get_received_mint(receive.recipient_id.clone())
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

#[wasm_bindgen_test]
async fn bfa_mint_is_atomic() {
    let (mut a, a_online, mut b, b_online, bridge, asset_id) =
        minter_and_receiver(2, 1, None).await;
    assert_no_partial_change(&mut a, "issue_asset_bfa", 0, async |w| {
        w.issue_asset_bfa(
            s("BFA"),
            s("Bridged"),
            0,
            1,
            CHAIN_ID,
            s(&bridge),
            None,
            false,
            None,
            false,
        )
    })
    .await;

    let rcv = mint_rcv(&mut b, &b_online).await;
    let before_block = evm_block_number().await + 1000;
    let res = mint_begin_before(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&rcv, MINT_AMOUNT),
        before_block,
    )
    .await
    .unwrap();
    // a failed refresh forgets the mint it checked
    assert_no_partial_change(&mut b, "refresh", 0, async |w| {
        w.refresh(b_online.clone(), None, vec![], false).await
    })
    .await;
    assert!(
        b.get_received_mint(rcv.recipient_id.clone())
            .unwrap()
            .is_none()
    );
    b.refresh(b_online.clone(), None, vec![], false)
        .await
        .unwrap();
    assert!(
        b.get_received_mint(rcv.recipient_id.clone())
            .unwrap()
            .is_some()
    );
    lock_mint(&bridge, &res, MINT_AMOUNT).await;
    assert_no_partial_change(&mut a, "mint_end", 1, async |w| {
        mint_end(w, &a_online, &res).await
    })
    .await;
}

#[wasm_bindgen_test]
async fn bfa_mint_retry_looks_up_lock() {
    let (mut a, a_online, mut b, b_online, bridge, asset_id) =
        minter_and_receiver(2, 1, None).await;
    let rcv = mint_rcv(&mut b, &b_online).await;
    let res = mint_begin(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&rcv, MINT_AMOUNT),
    )
    .await
    .unwrap();

    // checking the mint validates its consignment and saves the locks it waits for
    let v0 = validations();
    refresh(&mut b, &b_online).await;
    assert!(
        b.get_received_mint(rcv.recipient_id.clone())
            .unwrap()
            .is_some()
    );
    assert_eq!(
        transfer_status(&mut b, &rcv.recipient_id),
        TransferStatus::WaitingCounterparty
    );
    assert_eq!(validations(), v0 + 1);
    assert!(
        Wallet::restore(b.get_wallet_data())
            .await
            .unwrap()
            .pending_mint_locks
            .contains_key(&rcv.recipient_id)
    );

    // a saved entry for other consignment bytes is not used: the consignment is validated again
    let digest = b.pending_mint_locks[&rcv.recipient_id]
        .consignment_digest
        .clone();
    b.pending_mint_locks
        .get_mut(&rcv.recipient_id)
        .unwrap()
        .consignment_digest = "00".repeat(32);
    refresh(&mut b, &b_online).await;
    assert_eq!(validations(), v0 + 2);
    assert_eq!(
        transfer_status(&mut b, &rcv.recipient_id),
        TransferStatus::WaitingCounterparty
    );
    assert_eq!(
        b.pending_mint_locks[&rcv.recipient_id].consignment_digest,
        digest
    );

    // until its lock is finalized, a refresh only looks the lock up
    refresh(&mut b, &b_online).await;
    record_lock(
        &bridge,
        &mint_lock_id(&res),
        MINT_AMOUNT,
        evm_block_number().await,
    )
    .await;
    refresh(&mut b, &b_online).await;
    assert_eq!(validations(), v0 + 2);
    assert_eq!(
        transfer_status(&mut b, &rcv.recipient_id),
        TransferStatus::WaitingCounterparty
    );

    // once it is, the consignment is validated again and accepted
    finalize_evm().await;
    refresh(&mut b, &b_online).await;
    assert_eq!(validations(), v0 + 3);
    assert_eq!(
        transfer_status(&mut b, &rcv.recipient_id),
        TransferStatus::WaitingBroadcast
    );
    mint_end(&mut a, &a_online, &res).await.unwrap();
    for _ in 0..3 {
        b.full_sync(b_online.clone()).await.unwrap();
        refresh(&mut b, &b_online).await;
        if transfer_status(&mut b, &rcv.recipient_id) == TransferStatus::WaitingConfirmations {
            break;
        }
    }
    assert_eq!(
        transfer_status(&mut b, &rcv.recipient_id),
        TransferStatus::WaitingConfirmations
    );

    // settlement uses the validated copy: it validates nothing and needs no EVM RPC URL
    assert!(
        b.received_valid_consignments
            .contains_key(&rcv.recipient_id)
    );
    let no_evm = b.go_online(go_online_options(false)).await.unwrap();
    let statuses = settle_both(&mut a, &a_online, &mut b, &no_evm).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
    assert_eq!(b.get_asset_balance(asset_id).unwrap().settled, MINT_AMOUNT);
    assert_eq!(validations(), v0 + 3);
}

// the recipient of a send of minted units reads the supply from the bridge before settlement
#[wasm_bindgen_test]
async fn bfa_send_reads_bridge_supply() {
    let (mut a, a_online, mut b, b_online, bridge, asset_id) =
        minter_and_receiver(2, 3, None).await;
    let (mut c, c_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    // the test bridge gives the word of a zero lock ID and amount as its circulating supply
    record_lock(&bridge, &LockId::from([0u8; 32]), 0, MINT_AMOUNT - 1).await;
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

    send(
        &mut b,
        &b_online,
        &mut c,
        &c_online,
        &asset_id,
        Assignment::Fungible(30),
    )
    .await;
    let res = c.refresh(c_online, None, vec![], false).await.unwrap();
    let refreshed = res.values().next().unwrap();
    assert!(refreshed.failure.is_none(), "{:?}", refreshed.failure);
    assert_eq!(
        refreshed.updated_status,
        Some(TransferStatus::WaitingConfirmations)
    );
    let metadata = c.get_asset_metadata(asset_id).unwrap();
    assert_eq!(metadata.known_circulating_supply, MINT_AMOUNT);
}

// a mint whose media does not match the contract is not imported, also on the next refresh
#[wasm_bindgen_test]
async fn bfa_mint_media_mismatch_not_imported() {
    let _reset = ResetMocks;
    MOCK_ISSUE_MEDIA.set(Some(test_media()));
    let (mut a, a_online, mut b, b_online, bridge, asset_id) =
        minter_and_receiver(2, 1, None).await;
    assert!(MOCK_ISSUE_MEDIA.with_borrow(|m| m.is_none()));
    let rcv = blind_rcv_enc(
        &mut b,
        &b_online,
        None,
        Assignment::Fungible(MINT_AMOUNT),
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
        Some(Encryption::Optional),
    )
    .await
    .unwrap();
    let res = mint_begin(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&rcv, MINT_AMOUNT),
    )
    .await
    .unwrap();
    lock_mint(&bridge, &res, MINT_AMOUNT).await;

    for _ in 0..2 {
        serve_media("other");
        let r = b
            .refresh(b_online.clone(), None, vec![], false)
            .await
            .unwrap();
        assert!(
            matches!(
                r[&rcv.batch_transfer_idx].failure,
                Some(Error::HashMismatch)
            ),
            "{r:?}"
        );
        assert_eq!(
            transfer_status(&mut b, &rcv.recipient_id),
            TransferStatus::WaitingCounterparty
        );
        assert!(b.database.check_asset_exists(asset_id.clone()).is_err());
        assert!(!b.received_mints.contains_key(&rcv.recipient_id));
    }

    serve_media("media");
    let r = b
        .refresh(b_online.clone(), None, vec![], false)
        .await
        .unwrap();
    clear_rpc_hooks();
    assert!(r[&rcv.batch_transfer_idx].failure.is_none(), "{r:?}");
    assert_eq!(
        transfer_status(&mut b, &rcv.recipient_id),
        TransferStatus::WaitingBroadcast
    );
}

#[wasm_bindgen_test]
async fn bfa_mint_tx_inputs_spent() {
    let (mut a, a_online, mut b, b_online, bridge, asset_id) =
        minter_and_receiver(2, 1, None).await;
    let rcv = mint_rcv(&mut b, &b_online).await;
    let res = mint_begin(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&rcv, MINT_AMOUNT),
    )
    .await
    .unwrap();
    refresh(&mut b, &b_online).await;
    lock_mint(&bridge, &res, MINT_AMOUNT).await;
    refresh(&mut b, &b_online).await;
    assert_eq!(
        transfer_status(&mut b, &rcv.recipient_id),
        TransferStatus::WaitingBroadcast
    );

    // while the mint TX can still be broadcast, the receive waits for it
    refresh(&mut b, &b_online).await;
    assert_eq!(
        transfer_status(&mut b, &rcv.recipient_id),
        TransferStatus::WaitingBroadcast
    );
    let result = b
        .fail_transfers(b_online.clone(), Some(rcv.batch_transfer_idx), false, false)
        .await;
    assert!(
        matches!(result, Err(Error::CannotFailBatchTransfer)),
        "{result:?}"
    );

    // the minter spends an input of the mint TX elsewhere
    let outpoint = Psbt::from_str(&res.psbt).unwrap().unsigned_tx.input[0].previous_output;
    let script = a
        .bdk_wallet
        .peek_address(KeychainKind::External, 0)
        .script_pubkey();
    let mut builder = a.bdk_wallet.build_tx();
    builder
        .add_utxo(outpoint)
        .unwrap()
        .manually_selected_only()
        .drain_to(script)
        .fee_rate(FeeRate::from_sat_per_vb_u32(1));
    let psbt = builder.finish().unwrap();
    let signed = a.sign_psbt(psbt.to_string(), None).unwrap();
    let tx = Psbt::from_str(&signed).unwrap().extract_tx().unwrap();
    a.indexer().broadcast(&tx).await.unwrap();

    // a confirmed spend fails it
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    refresh(&mut b, &b_online).await;
    assert_eq!(
        transfer_status(&mut b, &rcv.recipient_id),
        TransferStatus::Failed
    );
}

#[wasm_bindgen_test]
async fn bfa_mint_invoice_restrictions() {
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 8).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 3).await;
    let mut assets = vec![];
    for (ticker, rights) in [("BFA", 4), ("BFB", 2)] {
        let bridge = new_evm_address();
        deploy_bridge(&bridge).await;
        let asset_id = a
            .issue_asset_bfa(
                s(ticker),
                s("Bridged"),
                0,
                rights,
                CHAIN_ID,
                s(&bridge),
                None,
                false,
                None,
                false,
            )
            .unwrap()
            .asset_id;
        assets.push((asset_id, bridge));
    }
    let [(asset_1, bridge_1), (asset_2, _)] = &assets[..] else {
        unreachable!()
    };
    mint(
        &mut a,
        &a_online,
        &mut b,
        &b_online,
        asset_1,
        bridge_1,
        MINT_AMOUNT,
    )
    .await;

    // a mint of another asset than the invoice's is refused
    let rcv = blind_rcv(
        &mut b,
        &b_online,
        Some(asset_1.clone()),
        Assignment::Fungible(MINT_AMOUNT),
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap();
    mint_begin(
        &mut a,
        &a_online,
        asset_2,
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

    // a mint that keeps the rules passes the receive checks
    let rcv = blind_rcv(
        &mut b,
        &b_online,
        Some(asset_1.clone()),
        Assignment::Fungible(MINT_AMOUNT),
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap();
    mint_begin(
        &mut a,
        &a_online,
        asset_1,
        mint_recipient(&rcv, MINT_AMOUNT),
    )
    .await
    .unwrap();
    b.refresh(b_online.clone(), None, vec![], false)
        .await
        .unwrap();
    assert_eq!(
        b.get_received_mint(rcv.recipient_id)
            .unwrap()
            .unwrap()
            .amount,
        MINT_AMOUNT
    );
}

/// A recipient of a mint right.
fn mint_right_recipient(receive: &ReceiveData, sat: Option<u64>) -> Recipient {
    Recipient {
        assignment: Assignment::MintRight,
        ..recipient(&receive.recipient_id, 0, sat, PROXY_URL)
    }
}

fn issue_bfa(wallet: &mut Wallet, rights: u8, bridge: &str) -> String {
    wallet
        .issue_asset_bfa(
            s("BFA"),
            s("Bridged"),
            0,
            rights,
            CHAIN_ID,
            s(bridge),
            None,
            false,
            None,
            false,
        )
        .unwrap()
        .asset_id
}

/// The number of mint rights of `asset_id` on each UTXO that holds one.
async fn rights_by_utxo(wallet: &mut Wallet, asset_id: &str) -> Vec<usize> {
    wallet
        .list_unspents(None, false, true)
        .await
        .unwrap()
        .iter()
        .map(|u| {
            u.rgb_allocations
                .iter()
                .filter(|a| {
                    a.asset_id.as_deref() == Some(asset_id) && a.assignment == Assignment::MintRight
                })
                .count()
        })
        .filter(|n| *n > 0)
        .collect()
}

async fn any_rcv(wallet: &mut Wallet, online: &Online) -> ReceiveData {
    blind_rcv(
        wallet,
        online,
        None,
        Assignment::Any,
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap()
}

async fn mint_right_rcv(
    wallet: &mut Wallet,
    online: &Online,
    asset_id: &str,
) -> Result<ReceiveData, Error> {
    blind_rcv(
        wallet,
        online,
        Some(asset_id.to_string()),
        Assignment::MintRight,
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
}

// one mint right of the sender goes to several recipients
#[wasm_bindgen_test]
async fn bfa_send_mint_right() {
    let bridge = new_evm_address();
    deploy_bridge(&bridge).await;
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 3).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 3).await;
    let asset_id = issue_bfa(&mut a, 2, &bridge);

    let r1 = any_rcv(&mut b, &b_online).await;
    let r2 = any_rcv(&mut b, &b_online).await;
    let rw = witness_rcv(
        &mut b,
        &b_online,
        None,
        Assignment::Any,
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap();
    let txid = send_recipients(
        &mut a,
        &a_online,
        &asset_id,
        vec![
            mint_right_recipient(&r1, None),
            mint_right_recipient(&r2, None),
            mint_right_recipient(&rw, Some(1000)),
        ],
    )
    .await;
    // only propagate transitions move the mint right
    let inspection = inspect_consignment(
        &a.send_consignment_bytes(&txid, &asset_id).unwrap(),
        BundleFilter::All,
    )
    .unwrap();
    assert_eq!(inspection.asset_schema, AssetSchema::Bfa);
    assert_eq!(inspection.witnesses.len(), 1);
    let transitions = &inspection.witnesses[0].operations[0].transitions;
    assert!(!transitions.is_empty());
    assert!(
        transitions
            .iter()
            .all(|t| t.r#type == TypeOfTransition::Propagate)
    );

    refresh(&mut b, &b_online).await;
    refresh(&mut a, &a_online).await;
    refresh(&mut b, &b_online).await;
    assert_eq!(
        transfer_status(&mut b, &r1.recipient_id),
        TransferStatus::WaitingConfirmations
    );
    let send_transfers = |wallet: &Wallet| -> Vec<crate::wallet::Transfer> {
        wallet
            .list_transfers(AssetFilter::Id(asset_id.clone()), None)
            .unwrap()
            .into_iter()
            .filter(|t| t.kind == TransferKind::Send && t.txid.as_ref() == Some(&txid))
            .collect()
    };
    let transfers = send_transfers(&a);
    assert_eq!(transfers.len(), 3);
    assert!(
        transfers
            .iter()
            .all(|t| t.status == TransferStatus::WaitingConfirmations)
    );
    assert_eq!(b.list_assets(vec![]).unwrap().bfa.unwrap().len(), 1);
    assert_eq!(
        b.get_asset_balance(asset_id.clone()).unwrap(),
        Balance {
            settled: 0,
            future: 0,
            spendable: 0,
        }
    );

    settle_both(&mut a, &a_online, &mut b, &b_online).await;
    let transfers = send_transfers(&a);
    assert_eq!(transfers.len(), 3);
    for t in &transfers {
        assert_eq!(t.status, TransferStatus::Settled);
        assert!(t.change_utxos.is_empty());
        assert_eq!(t.requested_assignment, Some(Assignment::MintRight));
        assert!(t.assignments.is_empty());
    }
    let received: Vec<crate::wallet::Transfer> = b
        .list_transfers(AssetFilter::Id(asset_id.clone()), None)
        .unwrap()
        .into_iter()
        .filter(|t| t.kind == TransferKind::ReceiveBlind)
        .collect();
    assert_eq!(received.len(), 2);
    assert!(
        received
            .iter()
            .all(|t| t.assignments == vec![Assignment::MintRight])
    );
    assert_eq!(mint_rights(&mut b, &asset_id).await, 3);
    assert_eq!(mint_rights(&mut a, &asset_id).await, 1);

    // the sender keeps its last mint right
    let rcv = mint_rcv(&mut b, &b_online).await;
    assert!(matches!(
        mint_begin(
            &mut a,
            &a_online,
            &asset_id,
            mint_recipient(&rcv, MINT_AMOUNT)
        )
        .await,
        Err(Error::LastMintRight)
    ));

    // the receiver mints with a received mint right
    add_utxos(&mut b, &b_online, 1, None).await;
    mint(
        &mut b,
        &b_online,
        &mut a,
        &a_online,
        &asset_id,
        &bridge,
        MINT_AMOUNT,
    )
    .await;
    assert_eq!(
        a.get_asset_balance(asset_id.clone()).unwrap().settled,
        MINT_AMOUNT
    );
    assert_eq!(mint_rights(&mut b, &asset_id).await, 2);
}

// a UTXO receives at most one mint right of an asset
#[wasm_bindgen_test]
async fn bfa_receive_mint_right_on_utxo_with_one() {
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 5).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let asset_id = issue_bfa(&mut a, 4, BRIDGE);

    // two invoices on the only UTXO of the receiver
    let i1 = any_rcv(&mut b, &b_online).await;
    let i2 = any_rcv(&mut b, &b_online).await;
    send_recipients(
        &mut a,
        &a_online,
        &asset_id,
        vec![mint_right_recipient(&i1, None)],
    )
    .await;
    refresh(&mut b, &b_online).await;
    refresh(&mut a, &a_online).await;
    refresh(&mut b, &b_online).await;
    assert_eq!(
        transfer_status(&mut b, &i1.recipient_id),
        TransferStatus::WaitingConfirmations
    );

    // the second mint right is refused
    send_recipients(
        &mut a,
        &a_online,
        &asset_id,
        vec![mint_right_recipient(&i2, None)],
    )
    .await;
    settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(
        transfer_status(&mut b, &i2.recipient_id),
        TransferStatus::Failed
    );
    assert_eq!(
        transfer_of(&a, &i2.recipient_id).status,
        TransferStatus::Failed
    );
    assert_eq!(
        transfer_status(&mut b, &i1.recipient_id),
        TransferStatus::Settled
    );
    assert_eq!(
        transfer_of(&a, &i1.recipient_id).status,
        TransferStatus::Settled
    );

    // no UTXO is left for a mint-right invoice
    assert!(matches!(
        mint_right_rcv(&mut b, &b_online, &asset_id).await,
        Err(Error::InsufficientAllocationSlots)
    ));

    // a mint right paid to an Any invoice on the UTXO is refused too
    let i3 = any_rcv(&mut b, &b_online).await;
    send_recipients(
        &mut a,
        &a_online,
        &asset_id,
        vec![mint_right_recipient(&i3, None)],
    )
    .await;
    settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(
        transfer_status(&mut b, &i3.recipient_id),
        TransferStatus::Failed
    );
    let held: Vec<_> = b
        .list_unspents(None, true, true)
        .await
        .unwrap()
        .into_iter()
        .filter(|u| !u.rgb_allocations.is_empty())
        .collect();
    assert_eq!(held.len(), 1);
    assert_eq!(
        held[0]
            .rgb_allocations
            .iter()
            .map(|a| a.assignment.clone())
            .collect::<Vec<_>>(),
        vec![Assignment::MintRight]
    );

    // a new UTXO takes one mint-right invoice
    add_utxos(&mut b, &b_online, 1, None).await;
    let i4 = mint_right_rcv(&mut b, &b_online, &asset_id).await.unwrap();
    assert_eq!(
        Invoice::new(i4.invoice.clone())
            .unwrap()
            .invoice_data()
            .assignment,
        Assignment::MintRight
    );
    assert!(matches!(
        mint_right_rcv(&mut b, &b_online, &asset_id).await,
        Err(Error::InsufficientAllocationSlots)
    ));
    send_recipients(
        &mut a,
        &a_online,
        &asset_id,
        vec![mint_right_recipient(&i4, None)],
    )
    .await;
    settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(
        transfer_status(&mut b, &i4.recipient_id),
        TransferStatus::Settled
    );
    assert_eq!(rights_by_utxo(&mut b, &asset_id).await, vec![1, 1]);
}

/// Wallet A with 2 UTXOs, each holding a mint right of a new BFA, and wallet N with 1000 of a new
/// NIA. A's UTXOs hold `size` sats.
async fn rights_and_nia(size: Option<u32>) -> (Wallet, Online, String, Wallet, Online, String) {
    let (mut a, a_online) = funded_wallet_with(
        wallet_data(vec![AssetSchema::Nia, AssetSchema::Bfa]),
        2,
        size,
        2,
        1_000_000,
    )
    .await;
    let bfa = issue_bfa(&mut a, 2, BRIDGE);
    let (n, n_online) = funded_wallet(wallet_data(vec![AssetSchema::Nia]), 3).await;
    let nia = n
        .issue_asset_nia(s("NIA"), s("Plain"), 0, vec![1000])
        .unwrap()
        .asset_id;
    (a, a_online, bfa, n, n_online, nia)
}

// the mint rights a send moves go each to its own UTXO
#[wasm_bindgen_test]
async fn bfa_moved_mint_rights_on_separate_utxos() {
    let (mut a, a_online, bfa, mut n, n_online, nia) = rights_and_nia(None).await;
    assert_eq!(rights_by_utxo(&mut a, &bfa).await, vec![1, 1]);
    send_and_settle(&mut n, &n_online, &mut a, &a_online, &nia, 100).await;
    send_and_settle(&mut n, &n_online, &mut a, &a_online, &nia, 100).await;
    let colored: Vec<_> = a
        .list_unspents(None, false, true)
        .await
        .unwrap()
        .into_iter()
        .filter(|u| !u.rgb_allocations.is_empty())
        .collect();
    assert_eq!(colored.len(), 2);
    assert!(colored.iter().all(|u| u.rgb_allocations.len() == 2));

    add_utxos(&mut a, &a_online, 2, None).await;
    send_and_settle(&mut a, &a_online, &mut n, &n_online, &nia, 150).await;
    assert_eq!(rights_by_utxo(&mut a, &bfa).await, vec![1, 1]);
    assert_eq!(a.get_asset_balance(nia).unwrap().settled, 50);
}

// a moved mint right needs a UTXO without one when the send has no BTC change
#[wasm_bindgen_test]
async fn bfa_moved_mint_right_no_btc_change() {
    let (mut a, a_online, bfa, mut n, n_online, nia) = rights_and_nia(Some(400)).await;
    let rs = any_rcv(&mut a, &a_online).await;
    send_recipients(
        &mut n,
        &n_online,
        &nia,
        vec![recipient(&rs.recipient_id, 100, None, PROXY_URL)],
    )
    .await;
    settle_both(&mut n, &n_online, &mut a, &a_online).await;
    let s_utxo = transfer_of(&a, &rs.recipient_id).receive_utxo.unwrap();
    let colored: Vec<_> = a
        .list_unspents(None, false, true)
        .await
        .unwrap()
        .into_iter()
        .filter(|u| !u.rgb_allocations.is_empty())
        .collect();
    assert_eq!(colored.len(), 2);
    for u in &colored {
        let rights = u
            .rgb_allocations
            .iter()
            .filter(|a| a.assignment == Assignment::MintRight)
            .count();
        assert_eq!(rights, 1);
        let expected = if u.utxo.outpoint == s_utxo { 2 } else { 1 };
        assert_eq!(u.rgb_allocations.len(), expected);
    }

    // the other UTXO holds a mint right, so the moved one has no destination
    let rr = any_rcv(&mut n, &n_online).await;
    let recipient_map = HashMap::from([(
        nia.clone(),
        vec![recipient(&rr.recipient_id, 50, None, PROXY_URL)],
    )]);
    let res = a
        .send_begin(
            a_online.clone(),
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
        matches!(res, Err(Error::InsufficientAllocationSlots)),
        "{res:?}"
    );

    add_utxos(&mut a, &a_online, 1, Some(400)).await;
    send_recipients(
        &mut a,
        &a_online,
        &nia,
        vec![recipient(&rr.recipient_id, 50, None, PROXY_URL)],
    )
    .await;
    settle_both(&mut a, &a_online, &mut n, &n_online).await;
    let rights = rights_by_utxo(&mut a, &bfa).await;
    assert!(rights.iter().all(|r| *r == 1));
    assert_eq!(rights.iter().sum::<usize>(), 2);
    assert!(
        a.list_unspents(None, false, true)
            .await
            .unwrap()
            .iter()
            .all(|u| u.utxo.outpoint != s_utxo)
    );
    assert_eq!(a.get_asset_balance(nia).unwrap().settled, 50);
}
