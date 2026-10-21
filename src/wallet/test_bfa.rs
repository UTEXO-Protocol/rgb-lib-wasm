use std::collections::HashMap;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[path = "../../tests/utils/mod.rs"]
pub(super) mod utils;

use crate::wallet::{Online, Recipient, Wallet, WalletData};
use crate::{
    AssetSchema, Assignment, BitcoinNetwork, Error, OS_ASSET, TransferStatus, generate_keys,
};
use amplify::{ByteArray, Wrapper};
use rgbstd::OpId;
use serde_json::{Value, json};
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

async fn finalized_block() -> u64 {
    let block = evm_rpc("eth_getBlockByNumber", json!(["finalized", false])).await;
    u64::from_str_radix(
        block["number"].as_str().unwrap().trim_start_matches("0x"),
        16,
    )
    .unwrap()
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

/// Put at `address` a reject list that returns `rejected` from `entries` and its count from
/// `length`, whatever the range.
pub(super) async fn deploy_reject_list(address: &str, rejected: &[(OpId, u16)]) {
    let mut entries = format!("{:064x}{:064x}", 0x20, rejected.len());
    for (opid, no) in rejected {
        let ty = OS_ASSET.to_inner();
        entries += &format!(
            "{}{ty:064x}{no:064x}{:064x}",
            hex::encode(opid.to_byte_array()),
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

pub(super) fn s(value: &str) -> String {
    value.to_string()
}
