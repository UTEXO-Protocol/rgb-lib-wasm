use wasm_bindgen_test::*;

use super::test_bfa::utils::*;
use super::test_bfa::*;
use super::test_burn::*;
use crate::database::enums::RecipientTypeFull;
use crate::wallet::offline::MOCK_IFA_LINK_RIGHT;
use crate::wallet::{AssetFilter, Invoice, Online, Recipient, Wallet};
use crate::{
    AssetSchema, Assignment, Error, FileContent, OS_LINK, RgbTransfer, TS_LINK, TransferKind,
    TransferStatus,
};
use bdk_wallet::bitcoin::OutPoint;
use bdk_wallet::bitcoin::psbt::Psbt;
use rgbstd::{ContractId, Opout};
use schemata::{GS_LINKED_TO_CONTRACT, TS_TRANSFER, TS_TRANSFER_LINK};
use std::collections::HashMap;
use std::str::FromStr;

fn nia_ifa() -> crate::wallet::WalletData {
    wallet_data(vec![AssetSchema::Nia, AssetSchema::Ifa])
}

fn link_rights(wallet: &mut Wallet, asset_id: &str) -> usize {
    assignments(wallet, asset_id)
        .iter()
        .filter(|a| **a == Assignment::LinkRight)
        .count()
}

/// A contract ID only parsed by the wallet.
fn contract(byte: u8) -> String {
    ContractId::copy_from_slice([byte; 32]).unwrap().to_string()
}

fn txid_of(psbt: &str) -> String {
    Psbt::from_str(psbt)
        .unwrap()
        .unsigned_tx
        .compute_txid()
        .to_string()
}

fn metadata_links(wallet: &Wallet, asset_id: &str) -> (Option<String>, Option<String>) {
    let metadata = wallet.get_asset_metadata(asset_id.to_string()).unwrap();
    (metadata.linked_from_contract, metadata.linked_to_contract)
}

fn receive_assignments(wallet: &Wallet, asset_id: &str, kind: TransferKind) -> Vec<Assignment> {
    wallet
        .list_transfers(AssetFilter::Id(asset_id.to_string()), None)
        .unwrap()
        .into_iter()
        .find(|t| t.kind == kind)
        .unwrap()
        .assignments
}

fn consignment(wallet: &Wallet, asset_id: &str, txid: &str) -> RgbTransfer {
    let bytes = &wallet.transfer_artifacts[txid].consignment_bytes[asset_id];
    RgbTransfer::load(&bytes[..]).unwrap()
}

async fn link_recipient(receiver: &mut Wallet, receiver_online: &Online) -> Recipient {
    let receive = blind_rcv(
        receiver,
        receiver_online,
        None,
        Assignment::Any,
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap();
    Recipient {
        assignment: Assignment::LinkRight,
        ..recipient(&receive.recipient_id, 0, None, PROXY_URL)
    }
}

async fn send_begin(
    sender: &mut Wallet,
    sender_online: &Online,
    asset_id: &str,
    recipients: Vec<Recipient>,
) -> Result<String, Error> {
    sender
        .send_begin(
            sender_online.clone(),
            HashMap::from([(asset_id.to_string(), recipients)]),
            false,
            1,
            1,
            default_send_expiration(),
            Some(transport_endpoint(PROXY_URL)),
            None,
            false,
        )
        .await
        .map(|begin| begin.psbt)
}

async fn send_all(
    sender: &mut Wallet,
    sender_online: &Online,
    asset_id: &str,
    recipients: Vec<Recipient>,
) -> String {
    let psbt = send_begin(sender, sender_online, asset_id, recipients)
        .await
        .unwrap();
    let signed = sender.sign_psbt(psbt, None).unwrap();
    sender
        .send_end(sender_online.clone(), signed, false)
        .await
        .unwrap()
        .txid
}

/// Send the link right of `asset_id` to a blinded UTXO of `receiver`. Return the TX ID.
async fn send_link_right(
    sender: &mut Wallet,
    sender_online: &Online,
    receiver: &mut Wallet,
    receiver_online: &Online,
    asset_id: &str,
) -> String {
    let recipient = link_recipient(receiver, receiver_online).await;
    send_all(sender, sender_online, asset_id, vec![recipient]).await
}

/// Check that the link consignment of `txid` spends the link right and commits `linked_to`.
fn assert_link_transition(wallet: &Wallet, asset_id: &str, txid: &str, linked_to: &str) {
    let consignment = consignment(wallet, asset_id, txid);
    let link = known_transitions(&consignment, txid)
        .into_iter()
        .find(|kt| kt.transition.transition_type == TS_LINK)
        .unwrap();
    let inputs: Vec<_> = link.transition.inputs.iter().collect();
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].ty, OS_LINK);
    assert!(link.transition.assignments.is_empty());
    let values: Vec<&[u8]> = link.transition.globals[&GS_LINKED_TO_CONTRACT]
        .iter()
        .map(|v| v.as_slice())
        .collect();
    let linked_to = ContractId::from_str(linked_to).unwrap();
    assert_eq!(values, vec![linked_to.as_slice()]);
}

/// Link `asset_id` to `linked_to`, then settle. Return the batch transfer idx and TX ID.
async fn link_and_settle(
    wallet: &mut Wallet,
    online: &Online,
    asset_id: &str,
    linked_to: &str,
) -> (i32, String) {
    let res = wallet
        .link(
            online.clone(),
            asset_id.to_string(),
            linked_to.to_string(),
            1,
            1,
        )
        .await
        .unwrap();
    mine_and_refresh(wallet, online).await;
    let linked = transfer(wallet, asset_id, res.batch_transfer_idx);
    assert_eq!(linked.kind, TransferKind::Link);
    assert_eq!(linked.status, TransferStatus::Settled);
    assert_eq!(linked.requested_assignment, Some(Assignment::LinkRight));
    assert_eq!(link_rights(wallet, asset_id), 0);
    (res.batch_transfer_idx, res.txid)
}

fn issue_ifa_link(wallet: &Wallet, linked_from: Option<String>) -> String {
    wallet
        .issue_asset_ifa(
            s("IFA"),
            s("Inflatable"),
            0,
            vec![100],
            vec![],
            None,
            true,
            linked_from,
        )
        .unwrap()
        .asset_id
}

#[wasm_bindgen_test]
async fn ifa_link_right_sent_and_linked() {
    let (mut a, a_online) = funded_wallet(nia_ifa(), 5).await;
    let (mut b, b_online) = funded_wallet(nia_ifa(), 2).await;
    let linked_from = a
        .issue_asset_nia(s("FROM"), s("From"), 0, vec![10])
        .unwrap()
        .asset_id;
    let linked_to = a
        .issue_asset_nia(s("TO"), s("To"), 0, vec![10])
        .unwrap()
        .asset_id;

    // issue with the link right
    let asset_id = issue_ifa_link(&a, Some(linked_from.clone()));
    assert_eq!(
        metadata_links(&a, &asset_id),
        (Some(linked_from.clone()), None)
    );
    assert_eq!(link_rights(&mut a, &asset_id), 1);
    assert!(
        receive_assignments(&a, &asset_id, TransferKind::Issuance).contains(&Assignment::LinkRight)
    );

    // the receiver sees the contract the asset is linked from
    send_link_right(&mut a, &a_online, &mut b, &b_online, &asset_id).await;
    settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(link_rights(&mut a, &asset_id), 0);
    assert_eq!(link_rights(&mut b, &asset_id), 1);
    assert_eq!(
        receive_assignments(&b, &asset_id, TransferKind::ReceiveBlind),
        vec![Assignment::LinkRight]
    );
    assert_eq!(
        metadata_links(&b, &asset_id),
        (Some(linked_from.clone()), None)
    );

    // the receiver links the asset, spending the link right
    let (_, txid) = link_and_settle(&mut b, &b_online, &asset_id, &linked_to).await;
    assert_link_transition(&b, &asset_id, &txid, &linked_to);
    assert_eq!(
        metadata_links(&b, &asset_id),
        (Some(linked_from), Some(linked_to))
    );
}

#[wasm_bindgen_test]
async fn bfa_link_keeps_mint_rights() {
    let (mut wallet, online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 3).await;
    let bridge = new_evm_address();
    deploy_bridge(&bridge).await;
    let asset_id = wallet
        .issue_asset_bfa(
            s("BFA"),
            s("Bridged"),
            0,
            1,
            CHAIN_ID,
            bridge,
            None,
            true,
            Some(contract(1)),
        )
        .unwrap()
        .asset_id;
    assert_eq!(
        metadata_links(&wallet, &asset_id),
        (Some(contract(1)), None)
    );
    let issued = receive_assignments(&wallet, &asset_id, TransferKind::Issuance);
    assert!(issued.contains(&Assignment::LinkRight));
    assert!(issued.contains(&Assignment::MintRight));

    link_and_settle(&mut wallet, &online, &asset_id, &contract(2)).await;
    assert_eq!(mint_rights(&mut wallet, &asset_id), 1);
    assert_eq!(
        metadata_links(&wallet, &asset_id),
        (Some(contract(1)), Some(contract(2)))
    );
}

#[wasm_bindgen_test]
async fn link_right_sent_beside_fungible() {
    let (mut a, a_online) = funded_wallet(nia_ifa(), 3).await;
    let (mut b, b_online) = funded_wallet(nia_ifa(), 2).await;
    let asset_id = issue_ifa_link(&a, None);

    let blind = blind_rcv(
        &mut b,
        &b_online,
        None,
        Assignment::Any,
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap();
    let witness = witness_rcv(
        &mut b,
        &b_online,
        None,
        Assignment::Any,
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap();
    let recipients = vec![
        recipient(&blind.recipient_id, 66, None, PROXY_URL),
        Recipient {
            assignment: Assignment::LinkRight,
            ..recipient(&witness.recipient_id, 0, Some(1000), PROXY_URL)
        },
    ];
    let txid = send_all(&mut a, &a_online, &asset_id, recipients).await;

    // a transfer moves the amount, a transfer of link rights moves the link right
    let consignment = consignment(&a, &asset_id, &txid);
    let mut types: Vec<_> = known_transitions(&consignment, &txid)
        .iter()
        .map(|kt| kt.transition.transition_type)
        .collect();
    types.sort();
    let mut expected = vec![TS_TRANSFER, TS_TRANSFER_LINK];
    expected.sort();
    assert_eq!(types, expected);

    settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(link_rights(&mut a, &asset_id), 0);
    assert_eq!(link_rights(&mut b, &asset_id), 1);
    assert_eq!(b.get_asset_balance(asset_id.clone()).unwrap().settled, 66);
    assert_eq!(
        receive_assignments(&b, &asset_id, TransferKind::ReceiveBlind),
        vec![Assignment::Fungible(66)]
    );
    assert_eq!(
        receive_assignments(&b, &asset_id, TransferKind::ReceiveWitness),
        vec![Assignment::LinkRight]
    );
}

#[wasm_bindgen_test]
async fn link_fails() {
    let (mut a, a_online) = funded_wallet(nia_ifa(), 6).await;
    let (mut b, b_online) = funded_wallet(nia_ifa(), 2).await;
    let nia = a
        .issue_asset_nia(s("NIA"), s("Fixed"), 0, vec![10])
        .unwrap()
        .asset_id;
    let no_link = a
        .issue_asset_ifa(
            s("IFA"),
            s("Inflatable"),
            0,
            vec![100],
            vec![],
            None,
            false,
            None,
        )
        .unwrap()
        .asset_id;
    let asset_id = issue_ifa_link(&a, None);

    // no link right
    let res = a
        .link_begin(a_online.clone(), no_link, nia.clone(), 1, 1, true)
        .await;
    assert!(matches!(res, Err(Error::InsufficientAssignments { .. })));

    // invalid PSBT
    let res = a.link_end(a_online.clone(), s("")).await;
    assert!(matches!(res, Err(Error::InvalidPsbt { .. })));

    // a link the wallet doesn't know
    let begin = a
        .link_begin(a_online.clone(), asset_id.clone(), nia.clone(), 1, 1, true)
        .await
        .unwrap();
    let signed_link = a.sign_psbt(begin.psbt, None).unwrap();
    let link_txid = txid_of(&signed_link);
    let res = b.link_end(b_online.clone(), signed_link).await;
    assert!(matches!(res, Err(Error::UnknownTransfer { txid }) if txid == link_txid));

    // the link right to more than one recipient
    let recipients = vec![
        link_recipient(&mut b, &b_online).await,
        link_recipient(&mut b, &b_online).await,
    ];
    let res = send_begin(&mut a, &a_online, &asset_id, recipients).await;
    assert!(matches!(res, Err(Error::InvalidRecipientData { .. })));

    // invoices requesting the link right
    let receive = blind_rcv(
        &mut a,
        &a_online,
        Some(asset_id.clone()),
        Assignment::LinkRight,
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap();
    let invoice = Invoice::new(receive.invoice).unwrap();
    assert_eq!(invoice.invoice_data().assignment, Assignment::LinkRight);
    let res = blind_rcv(
        &mut a,
        &a_online,
        Some(nia.clone()),
        Assignment::LinkRight,
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await;
    assert!(matches!(res, Err(Error::InvalidAssignment)));

    // an asset already linked
    link_and_settle(&mut a, &a_online, &asset_id, &nia).await;
    let res = a
        .link_begin(a_online.clone(), asset_id, nia, 1, 1, true)
        .await;
    assert!(matches!(res, Err(Error::InsufficientAssignments { .. })));
}

#[wasm_bindgen_test]
async fn link_begin_end() {
    let (mut wallet, online) = funded_wallet(nia_ifa(), 3).await;
    let asset_id = issue_ifa_link(&wallet, None);
    let last_operation = |wallet: &Wallet| {
        wallet
            .database
            .get_backup_info()
            .unwrap()
            .map(|b| b.last_operation_timestamp)
    };

    // a dry run records no operation
    let before = last_operation(&wallet);
    wallet
        .link_begin(online.clone(), asset_id.clone(), contract(1), 1, 1, true)
        .await
        .unwrap();
    assert_eq!(last_operation(&wallet), before);

    // begin saves the link, end records it
    let begin = wallet
        .link_begin(online.clone(), asset_id.clone(), contract(1), 1, 1, false)
        .await
        .unwrap();
    let idx = begin.batch_transfer_idx.unwrap();
    assert_ne!(last_operation(&wallet), before);
    assert_eq!(
        transfer(&wallet, &asset_id, idx).status,
        TransferStatus::Initiated
    );
    assert_eq!(metadata_links(&wallet, &asset_id), (None, None));
    let before = last_operation(&wallet);
    let signed = wallet.sign_psbt(begin.psbt, None).unwrap();
    let res = wallet.link_end(online.clone(), signed).await.unwrap();
    assert_eq!(res.batch_transfer_idx, idx);
    assert_ne!(last_operation(&wallet), before);
    assert_eq!(
        metadata_links(&wallet, &asset_id),
        (None, Some(contract(1)))
    );

    mine_and_refresh(&mut wallet, &online).await;
    let linked = transfer(&wallet, &asset_id, idx);
    assert_eq!(linked.status, TransferStatus::Settled);
    assert_eq!(linked.kind, TransferKind::Link);
    assert_eq!(link_rights(&mut wallet, &asset_id), 0);
}

#[wasm_bindgen_test]
async fn bfa_link_right_received_rejected_and_linked() {
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 3).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let bridge = new_evm_address();
    let reject_list = new_evm_address();
    deploy_bridge(&bridge).await;
    deploy_reject_list(&reject_list, &[]).await;
    let asset_id = a
        .issue_asset_bfa(
            s("BFA"),
            s("Bridged"),
            0,
            1,
            CHAIN_ID,
            bridge,
            Some(reject_list.clone()),
            true,
            Some(contract(1)),
        )
        .unwrap()
        .asset_id;

    // the receiver sees the contract the asset is linked from
    let txid = send_link_right(&mut a, &a_online, &mut b, &b_online, &asset_id).await;
    let consignment = consignment(&a, &asset_id, &txid);
    let opid = known_transitions(&consignment, &txid)
        .into_iter()
        .find(|kt| kt.transition.transition_type == TS_TRANSFER_LINK)
        .unwrap()
        .opid;
    let opout = Opout::new(opid, OS_LINK, 0);
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
    assert_eq!(link_rights(&mut b, &asset_id), 1);
    assert_eq!(
        receive_assignments(&b, &asset_id, TransferKind::ReceiveBlind),
        vec![Assignment::LinkRight]
    );
    assert_eq!(metadata_links(&b, &asset_id), (Some(contract(1)), None));

    // a rejected link right is not spent
    deploy_reject_list_opouts(&reject_list, &[opout]).await;
    let res = b
        .link(b_online.clone(), asset_id.clone(), contract(2), 1, 1)
        .await;
    assert!(
        matches!(res, Err(Error::InsufficientAssignments { asset_id: ref id, .. }) if *id == asset_id),
        "{res:?}"
    );
    assert_eq!(link_rights(&mut b, &asset_id), 1);

    // once allowed, the link right is spent
    deploy_reject_list_opouts(&reject_list, &[]).await;
    let (_, txid) = link_and_settle(&mut b, &b_online, &asset_id, &contract(2)).await;
    assert_eq!(
        metadata_links(&b, &asset_id),
        (Some(contract(1)), Some(contract(2)))
    );
    assert_link_transition(&b, &asset_id, &txid, &contract(2));
}

#[wasm_bindgen_test]
async fn link_right_moves_with_spent_utxo() {
    let (mut a, a_online) = funded_wallet(nia_ifa(), 3).await;
    let (mut b, b_online) = funded_wallet(nia_ifa(), 2).await;
    let link_right_outpoints = |wallet: &mut Wallet, asset_id: &str| {
        let unspents: Vec<OutPoint> = wallet
            .list_unspents(None, false, true)
            .unwrap()
            .into_iter()
            .map(|u| OutPoint::from(u.utxo.outpoint))
            .collect();
        let contract_id = ContractId::from_str(asset_id).unwrap();
        wallet
            .rgb_runtime()
            .unwrap()
            .contract_assignments_for(contract_id, unspents)
            .unwrap()
            .into_iter()
            .filter(|(_, state)| state.keys().any(|o| o.ty == OS_LINK))
            .map(|(seal, _)| seal.to_outpoint())
            .collect::<Vec<_>>()
    };

    // the genesis assigns the link right to the UTXO of a blind receive
    let receive = blind_rcv(
        &mut b,
        &b_online,
        None,
        Assignment::Any,
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap();
    let receive_outpoint = b
        .database
        .iter_transfers()
        .unwrap()
        .into_iter()
        .find_map(|t| match t.recipient_type {
            Some(RecipientTypeFull::Blind { unblinded_utxo }) => {
                Some(OutPoint::from(unblinded_utxo))
            }
            _ => None,
        })
        .unwrap();
    MOCK_IFA_LINK_RIGHT.replace(Some(a.get_builder_seal(receive_outpoint)));
    let asset_id = a
        .issue_asset_ifa(
            s("IFA"),
            s("Inflatable"),
            0,
            vec![100],
            vec![],
            None,
            false,
            None,
        )
        .unwrap()
        .asset_id;
    let recipients = vec![recipient(&receive.recipient_id, 66, None, PROXY_URL)];
    send_all(&mut a, &a_online, &asset_id, recipients).await;
    settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(
        link_right_outpoints(&mut b, &asset_id),
        vec![receive_outpoint]
    );

    // spending the UTXO moves the link right along
    send_and_settle(&mut b, &b_online, &mut a, &a_online, &asset_id, 66).await;
    let moved = link_right_outpoints(&mut b, &asset_id);
    assert_eq!(moved.len(), 1);
    assert_ne!(moved[0], receive_outpoint);
    let unspent = b
        .list_unspents(None, false, true)
        .unwrap()
        .into_iter()
        .find(|u| OutPoint::from(u.utxo.outpoint.clone()) == moved[0])
        .unwrap();
    assert!(unspent.rgb_allocations.iter().any(|a| {
        a.asset_id.as_deref() == Some(asset_id.as_str()) && a.assignment == Assignment::LinkRight
    }));
}
