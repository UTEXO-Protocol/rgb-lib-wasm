use std::collections::HashMap;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[path = "../../tests/utils/mod.rs"]
pub(super) mod utils;

use crate::wallet::idb_store::WalletSnapshot;
use crate::wallet::online::{MINT_AFTER_BLOCK_GRACE_SECS, MINT_FINALITY_GRACE_SECS};
use crate::wallet::{
    AssetFilter, MintBeginResult, Online, ReceiveData, Recipient, Wallet, WalletData, WitnessData,
};
use crate::{
    AssetSchema, Assignment, BitcoinNetwork, Error, OS_ASSET, OS_MINT, TransferKind,
    TransferStatus, generate_keys, script_buf_from_recipient_id,
};
use amplify::{ByteArray, Wrapper};
use bdk_wallet::bitcoin::{OutPoint, Psbt};
use rgbstd::{ContractId, OpId, Opout};
use serde_json::{Value, json};
use std::str::FromStr;
use utils::*;

const CHAIN_ID: u64 = 31337;
const BRIDGE: &str = "0x00000000000000000000000000000000000000b1";

pub(super) fn wallet_data(schemas: Vec<AssetSchema>) -> WalletData {
    let keys = generate_keys(BitcoinNetwork::Regtest);
    WalletData {
        data_dir: "/tmp/rgb_bfa".to_string(),
        bitcoin_network: BitcoinNetwork::Regtest,
        database_type: crate::wallet::DatabaseType::Sqlite,
        max_allocations_per_utxo: 5,
        account_xpub_vanilla: keys.account_xpub_vanilla,
        account_xpub_colored: keys.account_xpub_colored,
        mnemonic: Some(keys.mnemonic),
        master_fingerprint: keys.master_fingerprint,
        vanilla_keychain: None,
        supported_schemas: schemas,
        reuse_addresses: false,
    }
}

fn evm_rpc_urls() -> HashMap<u64, String> {
    HashMap::from([(CHAIN_ID, EVM_RPC_URL.to_string())])
}

fn transport_endpoint() -> String {
    format!("rpc://{}", PROXY_URL.trim_start_matches("http://"))
}

pub(super) async fn funded_wallet(wd: WalletData, utxos: u8) -> (Wallet, Online) {
    // a failed test can leave the mock set
    crate::api::evm::MOCK_MAX_MINT_SCAN_BLOCKS.replace(None);
    let mut wallet = Wallet::new(wd).unwrap();
    let online = wallet
        .go_online(false, ESPLORA_URL.to_string(), evm_rpc_urls())
        .await
        .unwrap();
    fund_address(&wallet.get_address().unwrap(), "1.0").await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();
    let unsigned = wallet
        .create_utxos_begin(online.clone(), true, Some(utxos), None, 1, true)
        .await
        .unwrap();
    let signed = wallet.sign_psbt(unsigned, None).unwrap();
    wallet
        .create_utxos_end(online.clone(), signed, true)
        .await
        .unwrap();
    mine_blocks(1).await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();
    (wallet, online)
}

pub(super) fn mint_rights(wallet: &mut Wallet, asset_id: &str) -> usize {
    wallet
        .list_unspents(None, false, true)
        .unwrap()
        .iter()
        .flat_map(|u| &u.rgb_allocations)
        .filter(|a| {
            a.asset_id.as_deref() == Some(asset_id) && a.assignment == Assignment::MintRight
        })
        .count()
}

/// Send `amount` of `asset_id` from `sender` to a blinded UTXO of `receiver`.
pub(super) async fn send(
    sender: &mut Wallet,
    sender_online: &Online,
    receiver: &mut Wallet,
    asset_id: &str,
    amount: u64,
) {
    let receive = receiver
        .blind_receive(
            None,
            Assignment::Fungible(amount),
            None,
            vec![transport_endpoint()],
            1,
        )
        .unwrap();
    let recipient = Recipient {
        recipient_id: receive.recipient_id,
        witness_data: None,
        assignment: Assignment::Fungible(amount),
        transport_endpoints: vec![transport_endpoint()],
        expiration_timestamp: None,
    };
    let recipient_map = HashMap::from([(asset_id.to_string(), vec![recipient])]);
    let unsigned = sender
        .send_begin(sender_online.clone(), recipient_map, false, 1, 1, None)
        .await
        .unwrap();
    let signed = sender.sign_psbt(unsigned, None).unwrap();
    sender
        .send_end(sender_online.clone(), signed, false)
        .await
        .unwrap();
}

/// Refresh both wallets and mine, three times. Return the new transfer statuses of the receiver.
pub(super) async fn settle(
    sender: &mut Wallet,
    sender_online: &Online,
    receiver: &mut Wallet,
    receiver_online: &Online,
) -> Vec<TransferStatus> {
    let mut statuses = vec![];
    for _ in 0..3 {
        for (i, (wallet, online)) in [
            (&mut *receiver, receiver_online),
            (&mut *sender, sender_online),
        ]
        .into_iter()
        .enumerate()
        {
            wallet.sync(online.clone()).await.unwrap();
            for (idx, refreshed) in wallet
                .refresh(online.clone(), None, vec![], false)
                .await
                .unwrap()
            {
                assert!(
                    refreshed.failure.is_none(),
                    "transfer {idx}: {:?}",
                    refreshed.failure
                );
                if i == 0 {
                    statuses.extend(refreshed.updated_status);
                }
            }
        }
        mine_blocks(1).await;
        wait_for_esplora_sync().await;
    }
    statuses
}

async fn send_and_settle(
    sender: &mut Wallet,
    sender_online: &Online,
    receiver: &mut Wallet,
    receiver_online: &Online,
    asset_id: &str,
    amount: u64,
) {
    send(sender, sender_online, receiver, asset_id, amount).await;
    settle(sender, sender_online, receiver, receiver_online).await;
}

#[wasm_bindgen_test]
async fn bfa_issue_list_and_backup() {
    let wd = wallet_data(vec![AssetSchema::Nia, AssetSchema::Bfa]);
    let (mut wallet, _online) = funded_wallet(wd.clone(), 3).await;

    assert!(matches!(
        wallet.issue_asset_bfa(s("BFA"), s("Bridged"), 2, 0, CHAIN_ID, s(BRIDGE), None),
        Err(Error::NoMintRights)
    ));
    assert!(matches!(
        wallet.issue_asset_bfa(s("BFA"), s("Bridged"), 2, 1, CHAIN_ID, s("0x12"), None),
        Err(Error::InvalidEvmContractAddress { .. })
    ));

    let bfa = wallet
        .issue_asset_bfa(s("BFA"), s("Bridged"), 2, 2, CHAIN_ID, s(BRIDGE), None)
        .unwrap();
    assert_eq!(bfa.initial_supply, 0);
    assert_eq!(bfa.balance.settled, 0);
    let listed = wallet.list_assets(vec![]).unwrap().bfa.unwrap();
    assert_eq!(listed, vec![bfa.clone()]);
    assert_eq!(
        wallet.get_asset_balance(bfa.asset_id.clone()).unwrap(),
        bfa.balance
    );
    assert_eq!(
        wallet
            .get_asset_metadata(bfa.asset_id.clone())
            .unwrap()
            .asset_schema,
        AssetSchema::Bfa
    );
    assert_eq!(mint_rights(&mut wallet, &bfa.asset_id), 2);

    let backup = wallet.backup("password").unwrap();
    let mut restored = Wallet::new(wd).unwrap();
    restored.restore_backup(&backup, "password").unwrap();
    let restored_bfa = restored
        .list_assets(vec![AssetSchema::Bfa])
        .unwrap()
        .bfa
        .unwrap();
    assert_eq!(restored_bfa.len(), 1);
    assert_eq!(restored_bfa[0].asset_id, bfa.asset_id);
    assert_eq!(mint_rights(&mut restored, &bfa.asset_id), 2);
}

#[wasm_bindgen_test]
async fn bfa_go_online_checks_evm_rpc_urls() {
    let (mut wallet, _online) =
        funded_wallet(wallet_data(vec![AssetSchema::Nia, AssetSchema::Bfa]), 1).await;

    let wrong_chain = HashMap::from([(1, EVM_RPC_URL.to_string())]);
    assert!(matches!(
        wallet
            .go_online(false, ESPLORA_URL.to_string(), wrong_chain)
            .await,
        Err(Error::EvmChainIdMismatch {
            expected: 1,
            actual: CHAIN_ID
        })
    ));
    let unreachable = HashMap::from([(CHAIN_ID, s("http://127.0.0.1:9"))]);
    assert!(matches!(
        wallet
            .go_online(false, ESPLORA_URL.to_string(), unreachable.clone())
            .await,
        Err(Error::InvalidEvmRpcUrl { .. })
    ));

    // the wallet works after a failure
    wallet
        .go_online(false, ESPLORA_URL.to_string(), evm_rpc_urls())
        .await
        .unwrap();
    wallet
        .issue_asset_nia(s("NIA"), s("Plain"), 0, vec![100])
        .unwrap();

    // a wallet without BFA does not check the URLs
    let mut other = Wallet::new(wallet_data(vec![AssetSchema::Nia])).unwrap();
    other
        .go_online(false, ESPLORA_URL.to_string(), unreachable)
        .await
        .unwrap();
}

#[wasm_bindgen_test]
async fn bfa_mint_right_moves_with_another_asset() {
    // every colored UTXO of A holds a mint right
    let (mut a, a_online) =
        funded_wallet(wallet_data(vec![AssetSchema::Nia, AssetSchema::Bfa]), 2).await;
    let bfa = a
        .issue_asset_bfa(s("BFA"), s("Bridged"), 0, 2, CHAIN_ID, s(BRIDGE), None)
        .unwrap();
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Nia]), 3).await;
    let nia = b
        .issue_asset_nia(s("NIA"), s("Plain"), 0, vec![100])
        .unwrap();

    // the NIA lands on a UTXO of A that holds a mint right
    send_and_settle(&mut b, &b_online, &mut a, &a_online, &nia.asset_id, 50).await;
    assert_eq!(
        a.get_asset_balance(nia.asset_id.clone()).unwrap().settled,
        50
    );
    let unspents = a.list_unspents(None, false, true).unwrap();
    assert!(unspents.iter().any(|u| {
        let holds = |id: &str| {
            u.rgb_allocations
                .iter()
                .any(|al| al.asset_id.as_deref() == Some(id))
        };
        holds(&nia.asset_id) && holds(&bfa.asset_id)
    }));

    // sending the NIA on moves the mint right with it
    send_and_settle(&mut a, &a_online, &mut b, &b_online, &nia.asset_id, 20).await;
    assert_eq!(
        a.get_asset_balance(nia.asset_id.clone()).unwrap().settled,
        30
    );
    assert_eq!(b.get_asset_balance(nia.asset_id).unwrap().settled, 70);
    assert_eq!(mint_rights(&mut a, &bfa.asset_id), 2);
}

/// Topic of `FundsIn(address indexed, uint256 indexed, uint64)`.
const FUNDS_IN_TOPIC: &str = "f1a18caea297591892fc07ea412a5e617d8e51e1155912d8871793e1d4e70f87";
/// The unlocked first account of anvil.
const ANVIL_ACCOUNT: &str = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266";

async fn evm_rpc(method: &str, params: Value) -> Value {
    let resp: Value = reqwest::Client::new()
        .post(EVM_RPC_URL)
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(resp.get("error").is_none(), "{method}: {resp}");
    resp["result"].clone()
}

/// A new EVM address for each call, so that runs on the same node do not share contracts.
pub(super) fn new_evm_address() -> String {
    let mut bytes = [0u8; 20];
    getrandom::getrandom(&mut bytes).unwrap();
    format!("0x{}", hex::encode(bytes))
}

/// A field of the EVM block with the given tag.
async fn evm_block_field(tag: &str, field: &str) -> u64 {
    let block = evm_rpc("eth_getBlockByNumber", json!([tag, false])).await;
    u64::from_str_radix(block[field].as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

async fn finalized_block() -> u64 {
    evm_block_field("finalized", "number").await
}

async fn latest_evm_timestamp() -> i64 {
    evm_block_field("latest", "timestamp").await as i64
}

/// Mine enough EVM blocks to finalize the current ones.
pub(super) async fn finalize_evm() {
    evm_rpc("anvil_mine", json!(["0x50"])).await;
}

/// Put at `address` a bridge that emits `FundsIn(word 0, word 1, word 2)` from its calldata.
pub(super) async fn deploy_bridge(address: &str) {
    let code =
        format!("0x60206040600037602035600035 7f{FUNDS_IN_TOPIC} 60206000a300").replace(' ', "");
    evm_rpc("anvil_setCode", json!([address, code])).await;
}

pub(super) async fn emit_funds_in(bridge: &str, opid: OpId, amount: u64) {
    let data = format!(
        "0x{:064x}{}{amount:064x}",
        0,
        hex::encode(opid.to_byte_array())
    );
    evm_rpc(
        "eth_sendTransaction",
        json!([{"from": ANVIL_ACCOUNT, "to": bridge, "data": data}]),
    )
    .await;
}

/// Put at `address` a reject list that rejects the asset opouts `rejected`.
pub(super) async fn deploy_reject_list(address: &str, rejected: &[(OpId, u16)]) {
    let opouts: Vec<Opout> = rejected
        .iter()
        .map(|(opid, no)| Opout::new(*opid, OS_ASSET, *no))
        .collect();
    deploy_reject_list_opouts(address, &opouts).await;
}

/// Put at `address` a reject list that returns `rejected` from `entries` and its count from
/// `length`, whatever the range.
async fn deploy_reject_list_opouts(address: &str, rejected: &[Opout]) {
    let mut entries = format!("{:064x}{:064x}", 0x20, rejected.len());
    for opout in rejected {
        let (ty, no) = (opout.ty.to_inner(), opout.no);
        entries += &format!(
            "{}{ty:064x}{no:064x}{:064x}",
            hex::encode(opout.op.to_byte_array()),
            1
        );
    }
    let len = entries.len() / 2;
    // the selector of length() jumps to 0x1e, any other returns the code from 0x29
    let code = format!(
        "0x6000 35 60e0 1c 631f7b6d32 14 601e 57 61{len:04x} 610029 6000 39 61{len:04x} 6000 f3 \
         5b 60{:02x} 6000 52 6020 6000 f3 {entries}",
        rejected.len()
    )
    .replace(' ', "");
    evm_rpc("anvil_setCode", json!([address, code])).await;
}

/// Issue a BFA with `bridge` and mint 100 units. The minter does not check the `FundsIn` event.
pub(super) async fn bfa_with_units(
    wallet: &mut Wallet,
    online: &Online,
    bridge: &str,
    reject_list: Option<String>,
) -> (String, OpId) {
    let bfa = wallet
        .issue_asset_bfa(
            s("BFA"),
            s("Bridged"),
            0,
            2,
            CHAIN_ID,
            s(bridge),
            reject_list,
        )
        .unwrap();
    let after_block = finalized_block().await;
    let (unsigned, opid) = wallet
        .test_mint_begin(online.clone(), bfa.asset_id.clone(), vec![100], after_block)
        .await
        .unwrap();
    let signed = wallet.sign_psbt(unsigned, None).unwrap();
    wallet.inflate_end(online.clone(), signed).await.unwrap();
    for _ in 0..3 {
        mine_blocks(1).await;
        wait_for_esplora_sync().await;
        wallet.sync(online.clone()).await.unwrap();
        wallet
            .refresh(online.clone(), None, vec![], false)
            .await
            .unwrap();
    }
    let balance = wallet.get_asset_balance(bfa.asset_id.clone()).unwrap();
    assert_eq!((balance.settled, balance.spendable), (100, 100));
    assert_eq!(mint_rights(wallet, &bfa.asset_id), 1);
    (bfa.asset_id, opid)
}

#[wasm_bindgen_test]
async fn bfa_send_receive_waits_for_funds_in() {
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 3).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let bridge = new_evm_address();
    deploy_bridge(&bridge).await;
    let (asset_id, opid) = bfa_with_units(&mut a, &a_online, &bridge, None).await;

    // no FundsIn event: the receiver does not accept the transfer
    send(&mut a, &a_online, &mut b, &asset_id, 30).await;
    let statuses = settle(&mut a, &a_online, &mut b, &b_online).await;
    assert!(statuses.is_empty(), "{statuses:?}");
    assert!(b.list_assets(vec![]).unwrap().bfa.unwrap().is_empty());

    // an event for another amount, then a non-finalized one, do not back the mint
    emit_funds_in(&bridge, opid, 99).await;
    finalize_evm().await;
    emit_funds_in(&bridge, opid, 100).await;
    let statuses = settle(&mut a, &a_online, &mut b, &b_online).await;
    assert!(statuses.is_empty(), "{statuses:?}");

    // the finalized event backs the mint
    finalize_evm().await;
    let statuses = settle(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
    let a_balance = a.get_asset_balance(asset_id.clone()).unwrap();
    assert_eq!((a_balance.settled, a_balance.spendable), (70, 70));
    let b_balance = b.get_asset_balance(asset_id.clone()).unwrap();
    assert_eq!((b_balance.settled, b_balance.spendable), (30, 30));
    assert_eq!(
        b.list_assets(vec![]).unwrap().bfa.unwrap()[0].asset_id,
        asset_id
    );
}

#[wasm_bindgen_test]
async fn bfa_receive_refuses_rejected_allocation() {
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 3).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let bridge = new_evm_address();
    let reject_list = new_evm_address();
    deploy_bridge(&bridge).await;
    deploy_reject_list(&reject_list, &[]).await;
    let (asset_id, opid) =
        bfa_with_units(&mut a, &a_online, &bridge, Some(reject_list.clone())).await;
    emit_funds_in(&bridge, opid, 100).await;
    finalize_evm().await;

    // the list rejects the minted allocation after the send
    send(&mut a, &a_online, &mut b, &asset_id, 30).await;
    deploy_reject_list(&reject_list, &[(opid, 0)]).await;
    let statuses = settle(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses, vec![TransferStatus::Failed]);
    assert!(b.list_assets(vec![]).unwrap().bfa.unwrap().is_empty());
}

/// The ways a `MaliciousRejectList` contract breaks the reject list read model. This mirrors the
/// `Mode` enum of `tests/contracts/MaliciousRejectList.sol`.
#[derive(Clone, Copy, Debug)]
enum MaliciousRejectListMode {
    LengthReverts,
    EntriesReverts,
    ShortPage,
    BadBool,
    LengthOverflow,
    Shrinking,
    HugeLength,
}

/// Deploy a `MaliciousRejectList` behaving as `mode` and put its code at `address`.
async fn deploy_malicious_reject_list(address: &str, mode: MaliciousRejectListMode) {
    // regenerate with `forge inspect --use 0.8.35 MaliciousRejectList.sol:MaliciousRejectList bytecode`
    let creation_code = include_str!("../../tests/contracts/MaliciousRejectList.bin").trim();
    let data = format!("{creation_code}{:064x}", mode as u8);
    let tx = evm_rpc(
        "eth_sendTransaction",
        json!([{"from": ANVIL_ACCOUNT, "data": data}]),
    )
    .await;
    let receipt = evm_rpc("eth_getTransactionReceipt", json!([tx])).await;
    let code = evm_rpc("eth_getCode", json!([receipt["contractAddress"], "latest"])).await;
    evm_rpc("anvil_setCode", json!([address, code])).await;
}

#[wasm_bindgen_test]
async fn bfa_receive_refuses_malicious_reject_list() {
    use MaliciousRejectListMode::*;

    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 3).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let bridge = new_evm_address();
    let reject_list = new_evm_address();
    deploy_bridge(&bridge).await;
    deploy_reject_list(&reject_list, &[]).await;
    let (asset_id, opid) =
        bfa_with_units(&mut a, &a_online, &bridge, Some(reject_list.clone())).await;
    emit_funds_in(&bridge, opid, 100).await;
    finalize_evm().await;
    send(&mut a, &a_online, &mut b, &asset_id, 30).await;

    // the receiver cannot read the list: the transfer stays pending and the asset is not imported
    let cases = [
        (None, "no contract at the reject list address"),
        (Some(LengthReverts), "eth_call error"),
        (Some(EntriesReverts), "eth_call error"),
        (Some(ShortPage), "unexpected reject list page size"),
        (Some(BadBool), "bad bool"),
        (
            Some(LengthOverflow),
            "reject list length doesn't fit in a u64",
        ),
        (
            Some(Shrinking),
            "shorter at the latest block than at the finalized one",
        ),
        (Some(HugeLength), "reject list too long"),
    ];
    for (mode, expected) in cases {
        match mode {
            Some(mode) => deploy_malicious_reject_list(&reject_list, mode).await,
            None => {
                evm_rpc("anvil_setCode", json!([reject_list, "0x"])).await;
            }
        }
        finalize_evm().await;
        let refreshed = b
            .refresh(b_online.clone(), None, vec![], false)
            .await
            .unwrap();
        let refreshed = refreshed.values().next().unwrap();
        assert!(
            refreshed.updated_status.is_none(),
            "{mode:?}: {refreshed:?}"
        );
        assert!(
            matches!(refreshed.failure, Some(Error::Network { ref details }) if details.contains(expected)),
            "{mode:?}: {refreshed:?}"
        );
        let transfers = b
            .list_transfers(crate::wallet::AssetFilter::Any, None)
            .unwrap();
        assert_eq!(
            transfers.last().unwrap().status,
            TransferStatus::WaitingCounterparty,
            "{mode:?}"
        );
        assert!(
            b.list_assets(vec![]).unwrap().bfa.unwrap().is_empty(),
            "{mode:?}"
        );
    }

    // a benign list again: the transfer settles
    deploy_reject_list(&reject_list, &[]).await;
    finalize_evm().await;
    let statuses = settle(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
}

#[wasm_bindgen_test]
async fn bfa_receive_refuses_mint_out_of_scan_window() {
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 3).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let bridge = new_evm_address();
    deploy_bridge(&bridge).await;
    let (asset_id, opid) = bfa_with_units(&mut a, &a_online, &bridge, None).await;
    emit_funds_in(&bridge, opid, 100).await;
    finalize_evm().await;
    send(&mut a, &a_online, &mut b, &asset_id, 30).await;

    // the event follows the block the mint commits to, so no scan of 0 blocks finds it
    crate::api::evm::MOCK_MAX_MINT_SCAN_BLOCKS.replace(Some(0));
    let res = b.refresh(b_online.clone(), None, vec![], false).await;
    crate::api::evm::MOCK_MAX_MINT_SCAN_BLOCKS.replace(None);
    let refreshed = res.unwrap();
    let refreshed = refreshed.values().next().unwrap();
    assert_eq!(
        refreshed.updated_status,
        Some(TransferStatus::Failed),
        "{refreshed:?}"
    );
    assert!(b.list_assets(vec![]).unwrap().bfa.unwrap().is_empty());
}

const MINT_AMOUNT: u64 = 100;

fn mint_recipient(receive: &ReceiveData, amount: u64) -> Recipient {
    Recipient {
        recipient_id: receive.recipient_id.clone(),
        witness_data: None,
        assignment: Assignment::Fungible(amount),
        transport_endpoints: vec![transport_endpoint()],
        expiration_timestamp: receive.expiration_timestamp.map(|e| e as u64),
    }
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
            transport_endpoint(),
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
) -> Result<crate::wallet::OperationResult, Error> {
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
    let receive = receiver
        .blind_receive(
            None,
            Assignment::Fungible(amount),
            None,
            vec![transport_endpoint()],
            1,
        )
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
    let statuses = settle(minter, minter_online, receiver, receiver_online).await;
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

    let receive = b
        .blind_receive(
            None,
            Assignment::Fungible(MINT_AMOUNT),
            None,
            vec![transport_endpoint()],
            1,
        )
        .unwrap();
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
    let statuses = settle(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));

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
    let receive = b
        .witness_receive(
            None,
            Assignment::Fungible(MINT_AMOUNT),
            None,
            vec![transport_endpoint()],
            1,
        )
        .unwrap();
    let mut recipient = mint_recipient(&receive, MINT_AMOUNT);
    recipient.witness_data = Some(WitnessData {
        amount_sat: 1000,
        blinding: None,
    });
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
    let statuses = settle(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
    assert_eq!(
        b.get_asset_balance(asset_id).unwrap().settled,
        70 + MINT_AMOUNT
    );
}

#[wasm_bindgen_test]
async fn bfa_mint_begin_refusals() {
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 5).await;
    let (b, _b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let bridge = new_evm_address();
    let receive = |b: &Wallet, duration| {
        b.blind_receive(
            None,
            Assignment::Fungible(MINT_AMOUNT),
            duration,
            vec![transport_endpoint()],
            1,
        )
        .unwrap()
    };
    let rcv = receive(&b, None);

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
    let short = receive(&b, Some(1800));
    let result = mint_begin(&mut a, &a_online, id, mint_recipient(&short, 100)).await;
    assert!(matches!(result, Err(Error::InvoiceExpired)), "{result:?}");
    let mut recipient = mint_recipient(&rcv, 100);
    recipient.expiration_timestamp = None;
    let result = mint_begin(&mut a, &a_online, id, recipient).await;
    assert!(
        matches!(result, Err(Error::InvalidRecipientData { ref details }) if details.contains("expiration")),
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
    let receive = b
        .blind_receive(
            None,
            Assignment::Fungible(MINT_AMOUNT),
            None,
            vec![transport_endpoint()],
            1,
        )
        .unwrap();
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
    let (c, _c_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
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
    let receive = c
        .blind_receive(
            None,
            Assignment::Fungible(30),
            None,
            vec![transport_endpoint()],
            1,
        )
        .unwrap();
    let recipient_map = HashMap::from([(
        asset_id.clone(),
        vec![Recipient {
            recipient_id: receive.recipient_id,
            witness_data: None,
            assignment: Assignment::Fungible(30),
            transport_endpoints: vec![transport_endpoint()],
            expiration_timestamp: None,
        }],
    )]);
    let result = b
        .send_begin(b_online.clone(), recipient_map, false, 1, 1, None)
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
    let receive = c
        .blind_receive(
            None,
            Assignment::Fungible(MINT_AMOUNT),
            None,
            vec![transport_endpoint()],
            1,
        )
        .unwrap();
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
    let snapshot = WalletSnapshot {
        sequence: 0,
        db: serde_json::from_value(db).unwrap(),
        bdk_changeset: None,
        transfer_artifacts: wallet.transfer_artifacts.clone(),
        received_consignments: wallet.received_consignments.clone(),
        received_mints: wallet.received_mints.clone(),
        mint_scanned_blocks: wallet.mint_scanned_blocks.clone(),
        rgb_store: None,
        reuse_address_index: wallet.reuse_address_index.clone(),
    };
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
    let receive = |b: &Wallet| {
        b.blind_receive(
            None,
            Assignment::Fungible(MINT_AMOUNT),
            None,
            vec![transport_endpoint()],
            1,
        )
        .unwrap()
    };

    // a mint of another amount than the requested one is refused before the lock
    let rcv = receive(&b);
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
    let rcv = receive(&b);
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
    let rcv = receive(&b);
    mint_begin(
        &mut a,
        &a_online,
        &asset_id,
        mint_recipient(&rcv, MINT_AMOUNT),
    )
    .await
    .unwrap();
    let no_evm = b
        .go_online(false, ESPLORA_URL.to_string(), HashMap::new())
        .await
        .unwrap();
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
        .go_online(false, ESPLORA_URL.to_string(), evm_rpc_urls())
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

    // with no EVM RPC URL and no lock, a mint past its deadline and grace fails on refresh
    let expiration = crate::utils::now().unix_timestamp() - MINT_FINALITY_GRACE_SECS - 1;
    edit_batch_transfer(&mut b, rcv.batch_transfer_idx, "expiration", expiration);
    let no_evm = b
        .go_online(false, ESPLORA_URL.to_string(), HashMap::new())
        .await
        .unwrap();
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
    let receive = receiver
        .blind_receive(
            None,
            Assignment::Fungible(MINT_AMOUNT),
            None,
            vec![transport_endpoint()],
            1,
        )
        .unwrap();
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
    let statuses = settle(&mut a, &a_online, &mut b, &b_online).await;
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
async fn bfa_reject_list_url() {
    use crate::api::reject_list::RejectListClient;

    let get = |path: &str| {
        let url = format!("{REGTEST_HELPER_URL}/reject-list/{path}");
        async move { RejectListClient::new(&url).unwrap().get_reject_list().await }
    };
    let list = (0..10_000)
        .map(|i| format!("{i:064x}/1/0\n"))
        .collect::<String>();
    assert_eq!(get("valid").await.unwrap(), list);
    assert!(matches!(
        get("redirect").await,
        Err(Error::RejectListService { .. })
    ));
    assert!(matches!(
        get("missing").await,
        Err(Error::RejectListService { ref details }) if details.contains("404")
    ));
    assert!(matches!(
        get("oversized").await,
        Err(Error::RejectListService { ref details }) if details.contains("reject list too large")
    ));
}

pub(super) fn s(value: &str) -> String {
    value.to_string()
}
