use std::collections::HashMap;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[path = "../../tests/utils/mod.rs"]
pub(super) mod utils;

use crate::wallet::{AssetFilter, EvmChain, Online, OnlineOptions, Recipient, Wallet, WalletData};
use crate::{
    AssetSchema, Assignment, BitcoinNetwork, Error, OS_ASSET, TransferStatus, generate_keys,
};
use amplify::{ByteArray, Wrapper};
use bdk_wallet::bitcoin::OutPoint;
use bdk_wallet::bitcoin::psbt::Psbt;
use rgbstd::containers::LockId;
use rgbstd::{ContractId, OpId, Opout, Txid as RgbTxid};
use serde_json::{Value, json};
use std::str::FromStr;
use utils::*;

pub(super) const CHAIN_ID: u64 = 31337;
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

pub(super) fn evm_chains(min: u64, max: u64) -> HashMap<u64, EvmChain> {
    HashMap::from([(
        CHAIN_ID,
        EvmChain {
            rpc_url: EVM_RPC_URL.to_string(),
            min_mint_lock_blocks: min,
            max_mint_lock_blocks: max,
        },
    )])
}

pub(super) async fn funded_wallet(wd: WalletData, utxos: u8) -> (Wallet, Online) {
    funded_wallet_with(wd, utxos, 2, 1_000_000).await
}

/// A funded wallet whose mint lock blocks are between `min` and `max`.
pub(super) async fn funded_wallet_with(
    wd: WalletData,
    utxos: u8,
    min: u64,
    max: u64,
) -> (Wallet, Online) {
    let mut wallet = Wallet::new(wd).unwrap();
    let online = wallet
        .go_online(OnlineOptions {
            evm_chains: evm_chains(min, max),
            ..go_online_options(false)
        })
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

/// Send `assignment` of `asset_id` from `sender` to a blinded UTXO of `receiver`.
pub(super) async fn send(
    sender: &mut Wallet,
    sender_online: &Online,
    receiver: &mut Wallet,
    receiver_online: &Online,
    asset_id: &str,
    assignment: Assignment,
) {
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
    let recipient = Recipient {
        assignment: assignment.clone(),
        ..recipient(
            &receive.recipient_id,
            assignment.main_amount(),
            None,
            PROXY_URL,
        )
    };
    let recipient_map = HashMap::from([(asset_id.to_string(), vec![recipient])]);
    let begin = sender
        .send_begin(
            sender_online.clone(),
            recipient_map,
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
    let signed = sender.sign_psbt(begin.psbt, None).unwrap();
    sender
        .send_end(sender_online.clone(), signed, false)
        .await
        .unwrap();
}

/// Refresh both wallets and mine, three times. Return the new transfer statuses of the receiver.
pub(super) async fn settle_both(
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

pub(super) async fn send_and_settle(
    sender: &mut Wallet,
    sender_online: &Online,
    receiver: &mut Wallet,
    receiver_online: &Online,
    asset_id: &str,
    amount: u64,
) {
    send(
        sender,
        sender_online,
        receiver,
        receiver_online,
        asset_id,
        Assignment::Fungible(amount),
    )
    .await;
    settle_both(sender, sender_online, receiver, receiver_online).await;
}

/// The opouts of `asset_id`, by UTXO, of the UTXOs that hold `assignment`.
pub(super) fn opouts_holding(
    wallet: &mut Wallet,
    asset_id: &str,
    assignment: &Assignment,
) -> Vec<Vec<Opout>> {
    let unspents = wallet.list_unspents(None, false, true).unwrap();
    let contract_id = ContractId::from_str(asset_id).unwrap();
    let runtime = wallet.rgb_runtime().unwrap();
    unspents
        .into_iter()
        .filter(|u| {
            u.rgb_allocations
                .iter()
                .any(|a| a.asset_id.as_deref() == Some(asset_id) && &a.assignment == assignment)
        })
        .map(|u| {
            runtime
                .contract_assignments_for(contract_id, [OutPoint::from(u.utxo.outpoint)])
                .unwrap()
                .into_values()
                .flat_map(|state| state.into_keys())
                .collect()
        })
        .collect()
}

#[wasm_bindgen_test]
async fn bfa_issue_list_and_backup() {
    let wd = wallet_data(vec![AssetSchema::Nia, AssetSchema::Bfa]);
    let keys = generate_keys(BitcoinNetwork::Mainnet);
    let mainnet = WalletData {
        bitcoin_network: BitcoinNetwork::Mainnet,
        account_xpub_vanilla: keys.account_xpub_vanilla,
        account_xpub_colored: keys.account_xpub_colored,
        mnemonic: Some(keys.mnemonic),
        master_fingerprint: keys.master_fingerprint,
        ..wd.clone()
    };
    assert!(matches!(
        Wallet::new(mainnet),
        Err(Error::CannotUseBfaOnMainnet)
    ));

    let (mut wallet, _online) = funded_wallet(wd.clone(), 3).await;

    assert!(matches!(
        wallet.issue_asset_bfa(
            s("BFA"),
            s("Bridged"),
            2,
            0,
            CHAIN_ID,
            s(BRIDGE),
            None,
            false,
            None
        ),
        Err(Error::NoMintRights)
    ));
    assert!(matches!(
        wallet.issue_asset_bfa(
            s("BFA"),
            s("Bridged"),
            2,
            1,
            CHAIN_ID,
            s("0x12"),
            None,
            false,
            None
        ),
        Err(Error::InvalidEvmContractAddress { .. })
    ));

    let ticker = s("₿FA");
    let bfa = wallet
        .issue_asset_bfa(
            ticker.clone(),
            s("Bridged"),
            2,
            2,
            CHAIN_ID,
            s(BRIDGE),
            None,
            false,
            None,
        )
        .unwrap();
    assert_eq!(bfa.ticker, ticker);
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
    assert_eq!(restored_bfa[0].ticker, ticker);
    assert_eq!(mint_rights(&mut restored, &bfa.asset_id), 2);
}

#[wasm_bindgen_test]
async fn bfa_go_online_checks_evm_chains() {
    let mut wallet = Wallet::new(wallet_data(vec![AssetSchema::Nia, AssetSchema::Bfa])).unwrap();
    let go_online = |evm_chains: HashMap<u64, EvmChain>| OnlineOptions {
        evm_chains,
        ..go_online_options(false)
    };
    let chain = |chain_id: u64, rpc_url: &str| {
        HashMap::from([(
            chain_id,
            EvmChain {
                rpc_url: s(rpc_url),
                min_mint_lock_blocks: 2,
                max_mint_lock_blocks: 1_000_000,
            },
        )])
    };

    let unreachable = chain(CHAIN_ID, "http://127.0.0.1:9");
    assert!(matches!(
        wallet.go_online(go_online(unreachable.clone())).await,
        Err(Error::InvalidEvmRpcUrl { .. })
    ));
    let result = wallet.go_online(go_online(evm_chains(11, 10))).await;
    assert!(
        matches!(result, Err(Error::InvalidEvmChain { ref details })
            if details.contains("min_mint_lock_blocks (11) is greater than max_mint_lock_blocks (10)")),
        "{result:?}"
    );
    assert_eq!(
        wallet
            .go_online(go_online(chain(CHAIN_ID + 1, EVM_RPC_URL)))
            .await
            .unwrap_err(),
        Error::EvmChainIdMismatch {
            expected: CHAIN_ID + 1,
            actual: CHAIN_ID
        }
    );

    // the wallet works after a failure
    wallet
        .go_online(go_online(evm_chains(2, 1_000_000)))
        .await
        .unwrap();

    // a wallet without BFA does not check the chains
    let mut other = Wallet::new(wallet_data(vec![AssetSchema::Nia])).unwrap();
    other.go_online(go_online(unreachable)).await.unwrap();
}

#[wasm_bindgen_test]
async fn bfa_mint_right_moves_with_another_asset() {
    // every colored UTXO of A holds a mint right
    let (mut a, a_online) =
        funded_wallet(wallet_data(vec![AssetSchema::Nia, AssetSchema::Bfa]), 2).await;
    let bfa = a
        .issue_asset_bfa(
            s("BFA"),
            s("Bridged"),
            0,
            2,
            CHAIN_ID,
            s(BRIDGE),
            None,
            false,
            None,
        )
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

pub(super) async fn evm_block_number() -> u64 {
    let block = evm_rpc("eth_getBlockByNumber", json!(["latest", false])).await;
    u64::from_str_radix(
        block["number"].as_str().unwrap().trim_start_matches("0x"),
        16,
    )
    .unwrap()
}

pub(super) async fn evm_mine(blocks: u64) {
    evm_rpc("anvil_mine", json!([format!("0x{blocks:x}")])).await;
}

/// The lock ID of the mint `opid` in the TX of `psbt`.
pub(super) fn lock_id_of(psbt: &str, opid: OpId) -> LockId {
    let txid = Psbt::from_str(psbt).unwrap().unsigned_tx.compute_txid();
    LockId::with(opid, RgbTxid::from_str(&txid.to_string()).unwrap())
}

/// Mine enough EVM blocks to finalize the current ones.
pub(super) async fn finalize_evm() {
    evm_rpc("anvil_mine", json!(["0x50"])).await;
}

/// Put at `address` a bridge that keeps a word per lock ID and amount. A call of 100 bytes
/// (selector, lock ID, amount, word) stores the word. Any other call returns the word of the lock
/// ID and amount after the selector, as `getFundsInBlock` does.
pub(super) async fn deploy_bridge(address: &str) {
    let code = "0x36606414601c5760406004600037604060002054600052602060 00f3 5b604435604060046000376040600020 55 00"
        .replace(' ', "");
    evm_rpc("anvil_setCode", json!([address, code])).await;
}

/// Record on `bridge` the lock `lock_id` of `amount` in `block`.
pub(super) async fn record_lock(bridge: &str, lock_id: &LockId, amount: u64, block: u64) {
    let key = format!("{}{amount:064x}", hex::encode(lock_id.to_byte_array()));
    let data = format!("0x00000000{key}{:064x}", block + 1);
    evm_rpc(
        "eth_sendTransaction",
        json!([{"from": ANVIL_ACCOUNT, "to": bridge, "data": data}]),
    )
    .await;
    let read = evm_rpc(
        "eth_call",
        json!([{"to": bridge, "data": format!("0x2ce06e2d{key}")}, "latest"]),
    )
    .await;
    assert_eq!(read, json!(format!("0x{:064x}", block + 1)));
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
pub(super) async fn deploy_reject_list_opouts(address: &str, rejected: &[Opout]) {
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

/// Issue a BFA with `bridge` and mint 100 units. The minter does not check the lock.
pub(super) async fn bfa_with_units(
    wallet: &mut Wallet,
    online: &Online,
    bridge: &str,
    reject_list: Option<String>,
) -> (String, OpId, LockId) {
    let bfa = wallet
        .issue_asset_bfa(
            s("₿FA"),
            s("Bridged"),
            2,
            2,
            CHAIN_ID,
            s(bridge),
            reject_list,
            false,
            None,
        )
        .unwrap();
    let before_block = evm_block_number().await + 1000;
    let (unsigned, opid) = wallet
        .test_mint_begin(online.clone(), bfa.asset_id.clone(), 100, before_block)
        .await
        .unwrap();
    let lock_id = lock_id_of(&unsigned, opid);
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
    (bfa.asset_id, opid, lock_id)
}

#[wasm_bindgen_test]
async fn bfa_send_receive_waits_for_mint_lock() {
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 3).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let bridge = new_evm_address();
    deploy_bridge(&bridge).await;
    let (asset_id, _, lock_id) = bfa_with_units(&mut a, &a_online, &bridge, None).await;

    // no lock: the receiver does not accept the transfer
    send(
        &mut a,
        &a_online,
        &mut b,
        &b_online,
        &asset_id,
        Assignment::Fungible(30),
    )
    .await;
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert!(statuses.is_empty(), "{statuses:?}");
    assert!(b.list_assets(vec![]).unwrap().bfa.unwrap().is_empty());

    // a lock of another amount, then one not finalized, do not back the mint
    record_lock(&bridge, &lock_id, 99, evm_block_number().await).await;
    finalize_evm().await;
    record_lock(&bridge, &lock_id, 100, evm_block_number().await).await;
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert!(statuses.is_empty(), "{statuses:?}");

    // the finalized lock backs the mint
    finalize_evm().await;
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
    let a_balance = a.get_asset_balance(asset_id.clone()).unwrap();
    assert_eq!((a_balance.settled, a_balance.spendable), (70, 70));
    let b_balance = b.get_asset_balance(asset_id.clone()).unwrap();
    assert_eq!((b_balance.settled, b_balance.spendable), (30, 30));
    let b_bfa = b.list_assets(vec![]).unwrap().bfa.unwrap().remove(0);
    assert_eq!(b_bfa.asset_id, asset_id);
    assert_eq!(
        (b_bfa.ticker.as_str(), b_bfa.name.as_str(), b_bfa.precision),
        ("₿FA", "Bridged", 2)
    );
    assert_eq!(b_bfa.details, None);

    // a witness receive of the backed units
    let receive = witness_rcv(
        &mut b,
        &b_online,
        None,
        Assignment::Fungible(30),
        default_rcv_expiration(),
        vec![transport_endpoint(PROXY_URL)],
    )
    .await
    .unwrap();
    let recipient = recipient(&receive.recipient_id, 30, Some(1000), PROXY_URL);
    let begin = a
        .send_begin(
            a_online.clone(),
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
        .unwrap();
    let signed = a.sign_psbt(begin.psbt, None).unwrap();
    a.send_end(a_online.clone(), signed, false).await.unwrap();
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
    let a_balance = a.get_asset_balance(asset_id.clone()).unwrap();
    assert_eq!((a_balance.settled, a_balance.spendable), (40, 40));
    let b_balance = b.get_asset_balance(asset_id.clone()).unwrap();
    assert_eq!((b_balance.settled, b_balance.spendable), (60, 60));
}

#[wasm_bindgen_test]
async fn bfa_receive_refuses_rejected_allocation() {
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 3).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let bridge = new_evm_address();
    let reject_list = new_evm_address();
    deploy_bridge(&bridge).await;
    deploy_reject_list(&reject_list, &[]).await;
    let (asset_id, opid, lock_id) =
        bfa_with_units(&mut a, &a_online, &bridge, Some(reject_list.clone())).await;
    record_lock(&bridge, &lock_id, 100, evm_block_number().await).await;
    finalize_evm().await;

    // the list rejects the minted allocation after the send
    send(
        &mut a,
        &a_online,
        &mut b,
        &b_online,
        &asset_id,
        Assignment::Fungible(30),
    )
    .await;
    deploy_reject_list(&reject_list, &[(opid, 0)]).await;
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses, vec![TransferStatus::Failed]);
    assert!(b.list_assets(vec![]).unwrap().bfa.unwrap().is_empty());
}

#[wasm_bindgen_test]
async fn bfa_receive_refuses_malicious_reject_list() {
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 3).await;
    let (mut b, b_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let bridge = new_evm_address();
    let reject_list = new_evm_address();
    deploy_bridge(&bridge).await;
    deploy_reject_list(&reject_list, &[]).await;
    let (asset_id, _, lock_id) =
        bfa_with_units(&mut a, &a_online, &bridge, Some(reject_list.clone())).await;
    record_lock(&bridge, &lock_id, 100, evm_block_number().await).await;
    finalize_evm().await;
    send(
        &mut a,
        &a_online,
        &mut b,
        &b_online,
        &asset_id,
        Assignment::Fungible(30),
    )
    .await;

    // the receiver cannot read the list: the transfer stays pending and the asset is not imported
    evm_rpc("anvil_setCode", json!([reject_list, "0x"])).await;
    finalize_evm().await;
    let refreshed = b
        .refresh(b_online.clone(), None, vec![], false)
        .await
        .unwrap();
    let refreshed = refreshed.values().next().unwrap();
    assert!(refreshed.updated_status.is_none(), "{refreshed:?}");
    assert!(
        matches!(refreshed.failure, Some(Error::Network { ref details }) if details.contains("no contract at the reject list address")),
        "{refreshed:?}"
    );
    let transfers = b.list_transfers(AssetFilter::Any, None).unwrap();
    assert_eq!(
        transfers.last().unwrap().status,
        TransferStatus::WaitingCounterparty
    );
    assert!(b.list_assets(vec![]).unwrap().bfa.unwrap().is_empty());

    // a benign list again: the transfer settles
    deploy_reject_list(&reject_list, &[]).await;
    finalize_evm().await;
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
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

#[wasm_bindgen_test]
async fn bfa_reject_list_rpc_config() {
    let (mut a, a_online) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 3).await;
    let (mut b, _) = funded_wallet(wallet_data(vec![AssetSchema::Bfa]), 1).await;
    let bridge = new_evm_address();
    let reject_list = new_evm_address();
    deploy_bridge(&bridge).await;
    deploy_reject_list(&reject_list, &[]).await;
    let (asset_id, _, lock_id) =
        bfa_with_units(&mut a, &a_online, &bridge, Some(reject_list)).await;
    record_lock(&bridge, &lock_id, 100, evm_block_number().await).await;
    finalize_evm().await;
    let b_no_evm = b.go_online(go_online_options(false)).await.unwrap();
    let receive = blind_rcv(
        &mut b,
        &b_no_evm,
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
    // the sender needs the EVM RPC URL of the chain of the reject list
    let a_no_evm = a.go_online(go_online_options(false)).await.unwrap();
    let result = a
        .send_begin(
            a_no_evm,
            recipient_map.clone(),
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
        matches!(result, Err(Error::NoEvmRpcUrl { chain_id: CHAIN_ID })),
        "{result:?}"
    );

    // the receiver waits for one
    let a_online = a
        .go_online(OnlineOptions {
            evm_chains: evm_chains(2, 1_000_000),
            ..go_online_options(false)
        })
        .await
        .unwrap();
    let begin = a
        .send_begin(
            a_online.clone(),
            recipient_map.clone(),
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
    a.send_end(a_online.clone(), signed, false).await.unwrap();
    b.refresh(b_no_evm, None, vec![], false).await.unwrap();
    let transfers = b.list_transfers(AssetFilter::Any, None).unwrap();
    assert_eq!(
        transfers.last().unwrap().status,
        TransferStatus::WaitingCounterparty
    );

    // and settles with it
    let b_online = b
        .go_online(OnlineOptions {
            evm_chains: evm_chains(2, 1_000_000),
            ..go_online_options(false)
        })
        .await
        .unwrap();
    let statuses = settle_both(&mut a, &a_online, &mut b, &b_online).await;
    assert_eq!(statuses.last(), Some(&TransferStatus::Settled));
}

pub(super) fn s(value: &str) -> String {
    value.to_string()
}
