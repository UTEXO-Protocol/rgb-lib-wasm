use wasm_bindgen_test::*;

use super::test_bfa::utils::*;
use super::test_bfa::*;
use crate::wallet::{AssetFilter, Online, Outpoint, Recipient, Transfer, Wallet, WalletData};
use crate::{
    AssetSchema, Assignment, BitcoinNetwork, Error, Fascia, FileContent, RgbOutExt, RgbTransfer,
    TS_BURN, TransferKind, TransferStatus, wallet::rust_only::validate_consignment_offchain,
};
use amplify::confinement::{Confined, U32};
use bdk_wallet::bitcoin::psbt::Psbt;
use rgbstd::{KnownTransition, schema::TransitionType};
use schemata::{TS_TRANSFER, TS_TRANSFER_INFLATION};
use std::collections::{BTreeSet, HashMap};
use std::str::FromStr;
use strict_encoding::StrictDeserialize;

fn reason() -> String {
    "ab".repeat(32)
}

/// Mine and refresh three times, so that the miner gets the broadcast transaction.
async fn mine_and_refresh(wallet: &mut Wallet, online: &Online) {
    for _ in 0..3 {
        mine_blocks(1).await;
        wait_for_esplora_sync().await;
        wallet.sync(online.clone()).await.unwrap();
        wallet
            .refresh(online.clone(), None, vec![], false)
            .await
            .unwrap();
    }
}

fn balance(wallet: &Wallet, asset_id: &str) -> (u64, u64, u64) {
    let b = wallet.get_asset_balance(asset_id.to_string()).unwrap();
    (b.settled, b.future, b.spendable)
}

fn transfer(wallet: &Wallet, asset_id: &str, batch_transfer_idx: i32) -> Transfer {
    wallet
        .list_transfers(AssetFilter::Id(asset_id.to_string()), None)
        .unwrap()
        .into_iter()
        .find(|t| t.batch_transfer_idx == batch_transfer_idx)
        .unwrap()
}

fn burns(wallet: &Wallet, asset_id: &str) -> Vec<(TransferKind, Option<String>)> {
    wallet
        .list_transfers(AssetFilter::Id(asset_id.to_string()), None)
        .unwrap()
        .into_iter()
        .filter(|t| t.kind == TransferKind::Burn)
        .map(|t| (t.kind, t.burn_reason))
        .collect()
}

/// The known transitions of the bundle of `txid` in `consignment`.
fn known_transitions<'c>(consignment: &'c RgbTransfer, txid: &str) -> Vec<&'c KnownTransition> {
    consignment
        .bundles
        .iter()
        .find(|wb| wb.tx.compute_txid().to_string() == txid)
        .unwrap()
        .bundle
        .known_transitions
        .iter()
        .collect()
}

/// Check that the burn of `txid` spends only the split of a transfer in its bundle.
fn assert_burn_split(consignment: &RgbTransfer, txid: &str) {
    let transitions = known_transitions(consignment, txid);
    let of_type = |ty: TransitionType| {
        transitions
            .iter()
            .filter(|kt| kt.transition.transition_type == ty)
            .collect::<Vec<_>>()
    };
    let (burns, transfers) = (of_type(TS_BURN), of_type(TS_TRANSFER));
    assert_eq!((burns.len(), transfers.len()), (1, 1));
    assert!(
        burns[0]
            .transition
            .inputs
            .iter()
            .all(|i| i.op == transfers[0].opid)
    );
    assert_eq!(consignment.terminal_ops(), BTreeSet::from([burns[0].opid]));
}

fn assignments(wallet: &mut Wallet, asset_id: &str) -> Vec<Assignment> {
    wallet
        .list_unspents(None, false, true)
        .unwrap()
        .into_iter()
        .flat_map(|u| u.rgb_allocations)
        .filter(|a| a.asset_id.as_deref() == Some(asset_id))
        .map(|a| a.assignment)
        .collect()
}

async fn ifa(
    wallet_utxos: u8,
    reject_list_url: Option<String>,
) -> (Wallet, Online, String, WalletData) {
    let wd = wallet_data(vec![AssetSchema::Nia, AssetSchema::Ifa]);
    let (wallet, online) = funded_wallet(wd.clone(), wallet_utxos).await;
    let asset = wallet
        .issue_asset_ifa(
            s("IFA"),
            s("Inflatable"),
            0,
            vec![600, 400],
            vec![100],
            reject_list_url,
        )
        .unwrap();
    (wallet, online, asset.asset_id, wd)
}

#[wasm_bindgen_test]
async fn ifa_burn_with_and_without_reason_settles() {
    let url = format!("{REGTEST_HELPER_URL}/reject-list/test/burn");
    serve_reject_list(&url, "");
    let (mut wallet, online, asset_id, wd) = ifa(5, Some(url.clone())).await;

    // a burn only the rejected allocation could cover fails
    let rejected = opouts_holding(&mut wallet, &asset_id, &Assignment::Fungible(600)).concat();
    assert_eq!(rejected.len(), 1);
    serve_reject_list(&url, &rejected[0].to_string());
    let result = wallet
        .burn(online.clone(), asset_id.clone(), 401, Some(reason()), 1, 1)
        .await;
    assert!(
        matches!(result, Err(Error::InsufficientAssignments { asset_id: ref id, .. }) if id == &asset_id),
        "{result:?}"
    );

    // one-step burn with a reason, which leaves the rejected allocation out
    let res = wallet
        .burn(online.clone(), asset_id.clone(), 150, Some(reason()), 1, 1)
        .await
        .unwrap();
    mine_and_refresh(&mut wallet, &online).await;
    assert_eq!(balance(&wallet, &asset_id), (850, 850, 850));
    let burned = transfer(&wallet, &asset_id, res.batch_transfer_idx);
    assert_eq!(burned.kind, TransferKind::Burn);
    assert_eq!(burned.status, TransferStatus::Settled);
    assert_eq!(burned.burn_reason, Some(reason()));
    assert_eq!(burned.requested_assignment, Some(Assignment::Fungible(150)));
    assert_eq!(burned.assignments, vec![Assignment::Fungible(250)]);
    assert_eq!(burned.recipient_id, None);
    assert!(burned.change_utxo.is_some());
    let held = assignments(&mut wallet, &asset_id);
    assert!(held.contains(&Assignment::Fungible(600)) && held.contains(&Assignment::Fungible(250)));
    serve_reject_list(&url, "");

    // the kept consignment proves the burn with the signed transaction
    let bytes = wallet.transfer_artifacts[&res.txid].consignment_bytes[&asset_id].clone();
    assert!(
        validate_consignment_offchain(&bytes, &res.txid, BitcoinNetwork::Regtest)
            .unwrap()
            .valid
    );
    let consignment = RgbTransfer::load(&bytes[..]).unwrap();
    assert_burn_split(&consignment, &res.txid);
    let witness_bundle = consignment
        .bundles
        .iter()
        .find(|wb| wb.tx.compute_txid().to_string() == res.txid)
        .unwrap();
    assert!(
        witness_bundle
            .tx
            .input
            .iter()
            .all(|i| !i.witness.is_empty())
    );
    assert!(bytes.windows(32).any(|w| w == [0xab; 32]));
    let unspents = wallet.list_unspents(None, false, true).unwrap();
    assert!(
        unspents
            .iter()
            .flat_map(|u| &u.rgb_allocations)
            .all(|a| a.assignment != Assignment::Fungible(0))
    );

    // begin without dry run, then end
    let begin = wallet
        .burn_begin(online.clone(), asset_id.clone(), 700, None, 1, 1, false)
        .await
        .unwrap();
    let idx = begin.batch_transfer_idx.unwrap();
    // refresh skips the initiated burn and its inputs are not spendable
    wallet
        .refresh(online.clone(), None, vec![], false)
        .await
        .unwrap();
    assert_eq!(
        transfer(&wallet, &asset_id, idx).status,
        TransferStatus::Initiated
    );
    assert!(matches!(
        wallet
            .burn_begin(online.clone(), asset_id.clone(), 1, None, 1, 1, true)
            .await,
        Err(Error::InsufficientAssignments { .. })
    ));
    assert_eq!(begin.details.burn_reason, None);
    let psbt = Psbt::from_str(&begin.psbt).unwrap();
    let opret = psbt
        .unsigned_tx
        .output
        .iter()
        .position(|o| o.script_pubkey.is_op_return())
        .unwrap();
    assert_eq!(
        psbt.outputs[opret].mpc_entropy(),
        Some(begin.details.entropy)
    );
    let fascia = Fascia::from_strict_serialized::<U32>(
        Confined::try_from(begin.details.fascia.clone()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        fascia.witness_id().to_string(),
        psbt.unsigned_tx.compute_txid().to_string()
    );
    let signed = wallet.sign_psbt(begin.psbt, None).unwrap();
    let res = wallet.burn_end(online.clone(), signed).await.unwrap();
    assert_eq!(res.batch_transfer_idx, idx);
    assert_eq!(res.entropy, begin.details.entropy);
    mine_and_refresh(&mut wallet, &online).await;
    let burned = transfer(&wallet, &asset_id, idx);
    assert_eq!(burned.status, TransferStatus::Settled);
    assert_eq!(burned.burn_reason, None);
    assert_eq!(balance(&wallet, &asset_id), (150, 150, 150));

    // a reload and a restore keep the burns
    wallet.flush().await.unwrap();
    drop(wallet);
    let wallet = Wallet::restore(wd.clone()).await.unwrap();
    let backup = wallet.backup("password").unwrap();
    let mut restored = Wallet::new(wd).unwrap();
    restored.restore_backup(&backup, "password").unwrap();
    let expected = vec![
        (TransferKind::Burn, Some(reason())),
        (TransferKind::Burn, None),
    ];
    assert_eq!(burns(&wallet, &asset_id), expected);
    assert_eq!(burns(&restored, &asset_id), expected);
    assert_eq!(balance(&restored, &asset_id), (150, 150, 150));
}

#[wasm_bindgen_test]
async fn bfa_burn_settles_and_keeps_send_receive() {
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 3).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let bridge = new_evm_address();
    let reject_list = new_evm_address();
    deploy_bridge(&bridge).await;
    deploy_reject_list(&reject_list, &[]).await;
    let (asset_id, _, lock_id) =
        bfa_with_units(&mut a, &a_online, &bridge, Some(reject_list)).await;

    let res = a
        .burn(a_online.clone(), asset_id.clone(), 30, Some(reason()), 1, 1)
        .await
        .unwrap();
    mine_and_refresh(&mut a, &a_online).await;
    assert_eq!(balance(&a, &asset_id), (70, 70, 70));
    let burned = transfer(&a, &asset_id, res.batch_transfer_idx);
    assert_eq!(burned.status, TransferStatus::Settled);
    assert_eq!(burned.burn_reason, Some(reason()));

    // the history holds a mint, so only an online check can accept the consignment
    let bytes = &a.transfer_artifacts[&res.txid].consignment_bytes[&asset_id];
    let validated =
        validate_consignment_offchain(bytes, &res.txid, BitcoinNetwork::Regtest).unwrap();
    assert!(!validated.valid);
    assert_eq!(validated.error.as_deref(), Some("external_anchors"));
    assert_burn_split(&RgbTransfer::load(&bytes[..]).unwrap(), &res.txid);

    let begin = a
        .burn_begin(a_online.clone(), asset_id.clone(), 20, None, 1, 1, true)
        .await
        .unwrap();
    assert_eq!(begin.batch_transfer_idx, None);
    let signed = a.sign_psbt(begin.psbt, None).unwrap();
    let res = a.burn_end(a_online.clone(), signed).await.unwrap();
    mine_and_refresh(&mut a, &a_online).await;
    assert_eq!(balance(&a, &asset_id), (50, 50, 50));
    assert_eq!(
        transfer(&a, &asset_id, res.batch_transfer_idx).burn_reason,
        None
    );
    assert_eq!(mint_rights(&mut a, &asset_id), 1);

    record_lock(&bridge, &lock_id, 100, evm_block_number().await).await;
    finalize_evm().await;
    send(
        &mut a,
        &a_online,
        &mut b,
        &b_online,
        &asset_id,
        Assignment::Fungible(10),
    )
    .await;
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
    assert_eq!(balance(&b, &asset_id), (10, 10, 10));
    assert_eq!(balance(&a, &asset_id), (40, 40, 40));
}

#[wasm_bindgen_test]
async fn burn_end_refuses_failed_ended_and_other_operations() {
    let (mut wallet, online, asset_id, _) = ifa(5, None).await;
    let before = balance(&wallet, &asset_id);

    // a failed burn
    let begin = wallet
        .burn_begin(online.clone(), asset_id.clone(), 700, None, 1, 1, false)
        .await
        .unwrap();
    let idx = begin.batch_transfer_idx.unwrap();
    let txid = Psbt::from_str(&begin.psbt)
        .unwrap()
        .unsigned_tx
        .compute_txid()
        .to_string();
    assert!(
        wallet
            .fail_transfers(online.clone(), Some(idx), false, false)
            .await
            .unwrap()
    );
    assert_eq!(balance(&wallet, &asset_id), before);
    let signed = wallet.sign_psbt(begin.psbt, None).unwrap();
    assert!(matches!(
        wallet.burn_end(online.clone(), signed).await,
        Err(Error::CannotEndTransfer { .. })
    ));
    mine_and_refresh(&mut wallet, &online).await;
    assert_eq!(
        transfer(&wallet, &asset_id, idx).status,
        TransferStatus::Failed
    );
    assert_eq!(
        wallet.indexer().get_tx_confirmations(&txid).await.unwrap(),
        None
    );

    // a burn PSBT in the end method of another operation
    let begin = wallet
        .burn_begin(online.clone(), asset_id.clone(), 100, None, 1, 1, false)
        .await
        .unwrap();
    let idx = begin.batch_transfer_idx.unwrap();
    let txid = Psbt::from_str(&begin.psbt)
        .unwrap()
        .unsigned_tx
        .compute_txid()
        .to_string();

    let signed = wallet.sign_psbt(begin.psbt.clone(), None).unwrap();
    let wrong = |r: &Result<(), Error>| matches!(r, Err(Error::WrongOperation { .. }));
    let results = [
        wallet
            .send_end(online.clone(), signed.clone(), false)
            .await
            .map(|_| ()),
        wallet
            .inflate_end(online.clone(), signed.clone())
            .await
            .map(|_| ()),
        wallet
            .create_utxos_end(online.clone(), signed.clone(), false)
            .await
            .map(|_| ()),
        wallet
            .drain_to_end(online.clone(), signed.clone())
            .await
            .map(|_| ()),
        wallet
            .send_btc_end(online.clone(), signed.clone(), false)
            .await
            .map(|_| ()),
        wallet
            .mint_end(online.clone(), signed.clone())
            .await
            .map(|_| ()),
    ];
    assert!(results.iter().all(wrong), "{results:?}");

    // a failed broadcast keeps the burn to end again
    assert!(wallet.burn_end(online.clone(), begin.psbt).await.is_err());
    assert_eq!(
        transfer(&wallet, &asset_id, idx).status,
        TransferStatus::Initiated
    );
    assert!(wallet.transfer_artifacts[&txid].fascia.is_some());
    wallet
        .burn_end(online.clone(), signed.clone())
        .await
        .unwrap();

    // an ended burn
    assert!(matches!(
        wallet.burn_end(online.clone(), signed.clone()).await,
        Err(Error::CannotEndTransfer { .. })
    ));
    assert!(wrong(
        &wallet
            .create_utxos_end(online.clone(), signed.clone(), false)
            .await
            .map(|_| ())
    ));

    // an inflate PSBT in burn_end
    let unsigned = wallet
        .inflate_begin(online.clone(), asset_id.clone(), vec![50], 1, 1)
        .await
        .unwrap();
    let inflate = wallet.sign_psbt(unsigned, None).unwrap();
    assert!(wrong(
        &wallet
            .burn_end(online.clone(), inflate.clone())
            .await
            .map(|_| ())
    ));
    wallet.inflate_end(online.clone(), inflate).await.unwrap();

    // a burn of another wallet
    let mut other = Wallet::new(wallet_data(vec![AssetSchema::Ifa])).unwrap();
    let other_online = other.go_online(go_online_options(false)).await.unwrap();
    assert!(matches!(
        other.burn_end(other_online, signed).await,
        Err(Error::UnknownTransfer { .. })
    ));
}

#[wasm_bindgen_test]
async fn burn_begin_reserves_inputs_until_end_or_fail() {
    let (mut wallet, online, asset_id, _) = ifa(5, None).await;
    let inputs = |psbt: &str| -> Vec<Outpoint> {
        Psbt::from_str(psbt)
            .unwrap()
            .unsigned_tx
            .input
            .iter()
            .map(|i| i.previous_output.into())
            .collect()
    };

    // a dry run reserves nothing
    wallet
        .burn_begin(online.clone(), asset_id.clone(), 100, None, 1, 1, true)
        .await
        .unwrap();
    assert!(wallet.get_reserved_outpoints().unwrap().is_empty());

    // a failed burn releases its inputs
    let begin = wallet
        .burn_begin(online.clone(), asset_id.clone(), 100, None, 1, 1, false)
        .await
        .unwrap();
    let reserved = wallet.get_reserved_outpoints().unwrap();
    assert!(inputs(&begin.psbt).iter().all(|i| reserved.contains(i)));
    wallet
        .fail_transfers(
            online.clone(),
            Some(begin.batch_transfer_idx.unwrap()),
            false,
            false,
        )
        .await
        .unwrap();
    assert!(wallet.get_reserved_outpoints().unwrap().is_empty());

    // an ended burn releases its inputs
    let begin = wallet
        .burn_begin(online.clone(), asset_id.clone(), 100, None, 1, 1, false)
        .await
        .unwrap();
    let reserved = wallet.get_reserved_outpoints().unwrap();
    assert!(inputs(&begin.psbt).iter().all(|i| reserved.contains(i)));
    let signed = wallet.sign_psbt(begin.psbt, None).unwrap();
    wallet.burn_end(online.clone(), signed).await.unwrap();
    assert!(wallet.get_reserved_outpoints().unwrap().is_empty());
}

#[wasm_bindgen_test]
async fn ifa_send_moves_inflation_rights() {
    let url = format!("{REGTEST_HELPER_URL}/reject-list/test/send");
    serve_reject_list(&url, "");
    let (mut a, a_online, asset_id, _) = ifa(3, Some(url.clone())).await;
    let (mut b, b_online) =
        funded_wallet(wallet_data(vec![AssetSchema::Nia, AssetSchema::Ifa]), 3).await;
    let send_to_b = async |a: &mut Wallet, b: &mut Wallet, assignments: Vec<Assignment>| {
        let mut recipients = vec![];
        for assignment in assignments {
            let receive = blind_rcv(
                b,
                &b_online,
                None,
                Assignment::Any,
                default_rcv_expiration(),
                vec![transport_endpoint(PROXY_URL)],
            )
            .await
            .unwrap();
            recipients.push(Recipient {
                assignment: assignment.clone(),
                ..recipient(
                    &receive.recipient_id,
                    assignment.main_amount(),
                    None,
                    PROXY_URL,
                )
            });
        }
        let begin = a
            .send_begin(
                a_online.clone(),
                HashMap::from([(asset_id.clone(), recipients)]),
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
        let txid = a
            .send_end(a_online.clone(), signed, false)
            .await
            .unwrap()
            .txid;
        let bytes = &a.transfer_artifacts[&txid].consignment_bytes[&asset_id];
        let consignment = RgbTransfer::load(&bytes[..]).unwrap();
        let mut types: Vec<_> = known_transitions(&consignment, &txid)
            .iter()
            .map(|kt| kt.transition.transition_type)
            .collect();
        types.sort();
        settle_both(a, &a_online, b, &b_online).await;
        types
    };

    // the transfer moves the asset, a transfer of inflation rights moves the rights
    let types = send_to_b(
        &mut a,
        &mut b,
        vec![Assignment::Fungible(66), Assignment::InflationRight(30)],
    )
    .await;
    assert_eq!(types, vec![TS_TRANSFER_INFLATION, TS_TRANSFER]);
    let b_assignments = assignments(&mut b, &asset_id);
    assert!(b_assignments.contains(&Assignment::Fungible(66)));
    assert!(b_assignments.contains(&Assignment::InflationRight(30)));
    assert_eq!(balance(&a, &asset_id).0, 934);
    assert!(assignments(&mut a, &asset_id).contains(&Assignment::InflationRight(70)));

    // a send of rights only has no main transfer
    let types = send_to_b(&mut a, &mut b, vec![Assignment::InflationRight(20)]).await;
    assert_eq!(types, vec![TS_TRANSFER_INFLATION]);
    assert_eq!(balance(&a, &asset_id).0, 934);
    assert!(assignments(&mut b, &asset_id).contains(&Assignment::InflationRight(20)));

    // a send that selects a rejected right retries with the other one and gets the list once
    let r20 = opouts_holding(&mut b, &asset_id, &Assignment::InflationRight(20)).concat();
    assert_eq!(r20.len(), 1);
    serve_reject_list(&url, &r20[0].to_string());
    let fetches = reject_list_fetches(&url);
    let right = Assignment::InflationRight(10);
    send(&mut b, &b_online, &mut a, &a_online, &asset_id, right).await;
    assert_eq!(reject_list_fetches(&url) - fetches, 1);
    settle_both(&mut b, &b_online, &mut a, &a_online).await;
    assert_eq!(inflation_rights(&mut b, &asset_id), vec![20, 20]);
}

fn inflation_rights(wallet: &mut Wallet, asset_id: &str) -> Vec<u64> {
    let mut rights: Vec<u64> = assignments(wallet, asset_id)
        .into_iter()
        .filter_map(|a| match a {
            Assignment::InflationRight(amount) => Some(amount),
            _ => None,
        })
        .collect();
    rights.sort();
    rights
}

async fn inflate_and_settle(b: &mut Wallet, b_online: &Online, asset_id: &str, amount: u64) {
    let unsigned = b
        .inflate_begin(b_online.clone(), asset_id.to_string(), vec![amount], 1, 1)
        .await
        .unwrap();
    let signed = b.sign_psbt(unsigned, None).unwrap();
    let res = b.inflate_end(b_online.clone(), signed).await.unwrap();
    mine_and_refresh(b, b_online).await;
    assert_eq!(
        transfer(b, asset_id, res.batch_transfer_idx).status,
        TransferStatus::Settled
    );
}

#[wasm_bindgen_test]
async fn ifa_inflate_reject_list() {
    let url = format!("{REGTEST_HELPER_URL}/reject-list/test/inflate");
    serve_reject_list(&url, "");
    let (mut a, a_online, asset_id, _) = ifa(3, Some(url.clone())).await;
    let (mut b, b_online) =
        funded_wallet(wallet_data(vec![AssetSchema::Nia, AssetSchema::Ifa]), 4).await;
    for amount in [60, 40] {
        let right = Assignment::InflationRight(amount);
        send(&mut a, &a_online, &mut b, &b_online, &asset_id, right).await;
        settle_both(&mut a, &a_online, &mut b, &b_online).await;
    }
    let r60 = opouts_holding(&mut b, &asset_id, &Assignment::InflationRight(60)).concat();
    let r40 = opouts_holding(&mut b, &asset_id, &Assignment::InflationRight(40)).concat();
    assert_eq!((r60.len(), r40.len()), (1, 1));

    // an inflation only the rejected right could cover fails
    serve_reject_list(&url, &r60[0].to_string());
    let result = b
        .inflate_begin(b_online.clone(), asset_id.clone(), vec![41], 1, 1)
        .await;
    assert!(
        matches!(result, Err(Error::InsufficientAssignments { asset_id: ref id, .. }) if id == &asset_id),
        "{result:?}"
    );

    // an inflation the allowed right covers spends it and leaves the rejected one out
    inflate_and_settle(&mut b, &b_online, &asset_id, 30).await;
    assert_eq!(inflation_rights(&mut b, &asset_id), vec![10, 60]);
    assert!(assignments(&mut b, &asset_id).contains(&Assignment::Fungible(30)));

    // an inflation that selects a rejected right retries with the other one and gets the list once
    serve_reject_list(&url, &r40[0].to_string());
    let fetches = reject_list_fetches(&url);
    inflate_and_settle(&mut b, &b_online, &asset_id, 5).await;
    assert_eq!(reject_list_fetches(&url) - fetches, 1);
    assert_eq!(inflation_rights(&mut b, &asset_id), vec![10, 55]);
    assert!(assignments(&mut b, &asset_id).contains(&Assignment::Fungible(5)));

    // a right with a rejected allocation in its history is refused
    serve_reject_list(&url, &format!("{}\n{}", r60[0], r40[0]));
    let result = b
        .inflate_begin(b_online.clone(), asset_id.clone(), vec![5], 1, 1)
        .await;
    assert!(
        matches!(result, Err(Error::InsufficientAssignments { asset_id: ref id, .. }) if id == &asset_id),
        "{result:?}"
    );
}
