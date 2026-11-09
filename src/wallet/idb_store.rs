//! IndexedDB persistence for WASM wallet snapshots.

use bdk_wallet::{ChangeSet, KeychainKind};
use serde::{Deserialize, Serialize};
use wasm_bindgen::{JsCast, JsValue, prelude::wasm_bindgen};

use std::collections::HashMap;

use rgbstd::persistence::Stock;

use crate::{
    database::{memory_db::InMemoryDb, rgb_store::MemRgbStore},
    utils::MemStock,
};

const DB_NAME: &str = "rgb_lib_wallet";
const DB_VERSION: u32 = 1;
const STORE_NAME: &str = "snapshots";

/// A serializable snapshot of wallet state for IndexedDB persistence.
#[derive(Serialize, Deserialize)]
pub struct WalletSnapshot {
    /// Random id of the wallet object that wrote this snapshot.
    pub writer: u64,
    /// Monotonic wallet-local snapshot sequence used to reject stale writes.
    pub sequence: u64,
    /// The in-memory RGB-lib database.
    pub db: InMemoryDb,
    /// The BDK wallet changeset.
    pub bdk_changeset: Option<ChangeSet>,
    /// Complete pending transfer state keyed by txid.
    pub(crate) transfer_artifacts: HashMap<String, super::online::TransferArtifacts>,
    /// Received consignment bytes keyed by recipient_id.
    #[serde(default)]
    pub received_consignments: HashMap<String, Vec<u8>>,
    /// Proxy data of each received consignment keyed by recipient_id.
    pub(crate) received_consignment_meta: HashMap<String, super::ReceivedConsignmentMeta>,
    /// Signed TXs released by the sender proxy keyed by recipient_id.
    pub received_signed_txs: HashMap<String, Vec<u8>>,
    /// Received mints keyed by recipient_id.
    pub(crate) received_mints: HashMap<String, super::ReceivedMint>,
    /// Encoded RGB store (base64).
    pub rgb_store: String,
    /// Pinned derivation index per keychain for address reuse.
    #[serde(default)]
    pub reuse_address_index: HashMap<KeychainKind, u32>,
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = IDBDatabase)]
    type Db;

    #[wasm_bindgen(catch, method, structural, js_name = transaction)]
    fn transaction_with_options(
        this: &Db,
        store: &str,
        mode: &str,
        options: &js_sys::Object,
    ) -> Result<web_sys::IdbTransaction, JsValue>;
}

async fn open_db() -> Result<idb::Database, String> {
    idb::Database::builder(DB_NAME)
        .version(DB_VERSION)
        .add_object_store(idb::ObjectStore::builder(STORE_NAME))
        .build()
        .await
        .map_err(|e| format!("IndexedDB open error: {e:?}"))
}

/// Save a wallet snapshot to IndexedDB under the given key.
///
/// `loaded` is the writer and sequence of the snapshot this wallet loaded from IndexedDB.
/// Fail when nothing is written: the stored snapshot is not from this wallet, or it is not older.
pub async fn save_snapshot(
    key: &str,
    snapshot: &WalletSnapshot,
    loaded: Option<(u64, u64)>,
) -> Result<(), String> {
    let db = web_sys::IdbDatabase::from(open_db().await?);
    let options = js_sys::Object::new();
    js_sys::Reflect::set(&options, &"durability".into(), &"strict".into())
        .map_err(|e| format!("IndexedDB transaction error: {e:?}"))?;
    let tx = idb::Transaction::from(
        db.unchecked_ref::<Db>()
            .transaction_with_options(STORE_NAME, "readwrite", &options)
            .map_err(|e| format!("IndexedDB transaction error: {e:?}"))?,
    );
    let store = tx
        .object_store(STORE_NAME)
        .map_err(|e| format!("IndexedDB store error: {e:?}"))?;

    let json_str = serde_json::to_string(snapshot).map_err(|e| format!("Serialize error: {e}"))?;
    let js_key = JsValue::from_str(key);
    if let Some(existing) = store
        .get(js_key.clone())
        .map_err(|e| format!("IndexedDB get error: {e:?}"))?
        .await
        .map_err(|e| format!("IndexedDB get error: {e:?}"))?
    {
        let json = existing
            .as_string()
            .ok_or_else(|| "IndexedDB value is not a string".to_string())?;
        let existing: WalletSnapshot =
            serde_json::from_str(&json).map_err(|e| format!("Deserialize error: {e}"))?;
        if (existing.writer != snapshot.writer
            && Some((existing.writer, existing.sequence)) != loaded)
            || existing.sequence >= snapshot.sequence
        {
            tx.await
                .map_err(|e| format!("IndexedDB commit error: {e:?}"))?;
            return Err(format!(
                "IndexedDB keeps snapshot {} of writer {}",
                existing.sequence, existing.writer
            ));
        }
    }
    let js_val = JsValue::from_str(&json_str);

    store
        .put(&js_val, Some(&js_key))
        .map_err(|e| format!("IndexedDB put error: {e:?}"))?
        .await
        .map_err(|e| format!("IndexedDB put error: {e:?}"))?;
    tx.await
        .map_err(|e| format!("IndexedDB commit error: {e:?}"))?;
    Ok(())
}

/// Ask the browser to keep this origin's storage. A refusal is logged, not returned.
pub(crate) fn request_persistent_storage() {
    use web_sys::console;

    let promise = match js_sys::Reflect::get(&js_sys::global(), &"navigator".into())
        .and_then(|navigator| js_sys::Reflect::get(&navigator, &"storage".into()))
        .and_then(|storage| storage.dyn_into::<web_sys::StorageManager>())
        .and_then(|storage| storage.persist())
    {
        Ok(promise) => promise,
        Err(e) => {
            console::warn_1(&format!("persistent storage is not available: {e:?}").into());
            return;
        }
    };
    wasm_bindgen_futures::spawn_local(async move {
        match wasm_bindgen_futures::JsFuture::from(promise).await {
            Ok(granted) => console::log_1(
                &format!("persistent storage granted: {}", granted.is_truthy()).into(),
            ),
            Err(e) => console::warn_1(&format!("persistent storage request failed: {e:?}").into()),
        }
    });
}

/// Serialize the RGB store of a stock to base64 for snapshot persistence.
pub(crate) fn serialize_stock(stock: &MemStock) -> Option<String> {
    use base64::{Engine as _, engine::general_purpose};

    let bytes = stock.as_store().to_bytes().ok()?;
    Some(general_purpose::STANDARD.encode(bytes))
}

/// Deserialize an RGB Stock from base64.
pub(crate) fn deserialize_stock(store_b64: &str) -> Result<MemStock, String> {
    use base64::{Engine as _, engine::general_purpose};

    let bytes = general_purpose::STANDARD
        .decode(store_b64)
        .map_err(|e| e.to_string())?;
    Ok(Stock::with(MemRgbStore::from_bytes(&bytes)?))
}

/// Load a wallet snapshot from IndexedDB for the given key.
pub async fn load_snapshot(key: &str) -> Result<Option<WalletSnapshot>, String> {
    let db = open_db().await?;
    let tx = db
        .transaction(&[STORE_NAME], idb::TransactionMode::ReadOnly)
        .map_err(|e| format!("IndexedDB transaction error: {e:?}"))?;
    let store = tx
        .object_store(STORE_NAME)
        .map_err(|e| format!("IndexedDB store error: {e:?}"))?;

    let js_key = JsValue::from_str(key);
    let result = store
        .get(js_key)
        .map_err(|e| format!("IndexedDB get error: {e:?}"))?
        .await
        .map_err(|e| format!("IndexedDB get error: {e:?}"))?;

    match result {
        Some(js_val) => {
            let json_str = js_val
                .as_string()
                .ok_or_else(|| "IndexedDB value is not a string".to_string())?;
            let snapshot: WalletSnapshot =
                serde_json::from_str(&json_str).map_err(|e| format!("Deserialize error: {e}"))?;
            Ok(Some(snapshot))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rgbstd::containers::ConsignmentExt;
    use rgbstd::contract::IssuerWrapper;
    use schemata::NonInflatableAsset;

    #[test]
    fn stock_serialize_round_trip_empty() {
        let stock = MemStock::default();
        let s = serialize_stock(&stock).expect("serialize empty stock");

        let restored = deserialize_stock(&s).expect("deserialize empty stock");
        assert_eq!(restored.contracts().count(), 0);
    }

    #[test]
    fn stock_serialize_round_trip_with_contract() {
        use crate::{AssetSpec, ContractTerms, DumbResolver, Precision, RicardianContract};
        use rgbstd::contract::ContractBuilder;
        use rgbstd::txout::BlindSeal;
        use rgbstd::{Amount, ChainNet, Identity, Txid as RgbTxid};

        let mut stock = MemStock::default();
        stock
            .import_schema_definition(NonInflatableAsset::schema_definition())
            .expect("import schema");

        let spec = AssetSpec {
            ticker: crate::Ticker::try_from("TEST".to_string()).unwrap(),
            name: crate::Name::try_from("Test Token".to_string()).unwrap(),
            details: None,
            precision: Precision::try_from(0u8).unwrap(),
        };
        let terms = ContractTerms {
            text: RicardianContract::default(),
            media: None,
        };

        // Dummy seal for the asset assignment
        let dummy_txid = "0000000000000000000000000000000000000000000000000000000000000000"
            .parse::<RgbTxid>()
            .unwrap();
        let seal = BlindSeal::new_random(dummy_txid, 0);

        let builder = ContractBuilder::with(
            Identity::default(),
            NonInflatableAsset::schema_rules(),
            ChainNet::from(crate::BitcoinNetwork::Regtest),
        )
        .add_global_state("spec", spec)
        .expect("invalid spec")
        .add_global_state("terms", terms)
        .expect("invalid terms")
        .add_global_state("issuedSupply", Amount::from(1000u64))
        .expect("invalid issuedSupply")
        .add_fungible_state("assetOwner", seal, 1000u64)
        .expect("invalid assignment");

        let contract = builder.issue_contract().expect("issue contract");
        let contract_id = contract.contract_id();

        stock
            .import_contract(contract, &DumbResolver)
            .expect("import contract");

        // Verify contract exists before serialization
        assert!(
            stock
                .contracts()
                .any(|c| c.is_ok_and(|c| c.id == contract_id))
        );

        // Serialize → JSON round-trip (simulates IndexedDB save/load)
        let s = serialize_stock(&stock).expect("serialize stock with contract");

        let mut transfer_artifact = crate::wallet::online::TransferArtifacts::default();
        transfer_artifact
            .consignment_bytes
            .insert("asset-id".to_owned(), vec![1, 2, 3, 4]);
        transfer_artifact.signed_psbt = Some("signed-psbt".to_owned());
        transfer_artifact.fascia = Some(vec![7, 8]);
        let snapshot = WalletSnapshot {
            writer: 1,
            sequence: 1,
            db: InMemoryDb::new(),
            bdk_changeset: None,
            transfer_artifacts: HashMap::from([("funding-txid".to_owned(), transfer_artifact)]),
            received_consignments: HashMap::from([("rid".to_owned(), vec![5])]),
            received_consignment_meta: HashMap::from([(
                "rid".to_owned(),
                super::super::ReceivedConsignmentMeta {
                    proxy_url: Some("proxy".to_owned()),
                    transfer_id: Some("tid".to_owned()),
                    sender_transport_endpoint: "rpc://proxy".to_owned(),
                    sender_pubkey: "pk".to_owned(),
                    consignment_hash: "hash".to_owned(),
                    batch: true,
                },
            )]),
            received_signed_txs: HashMap::from([("rid".to_owned(), vec![6])]),
            received_mints: HashMap::from([(
                "rid".to_owned(),
                super::super::ReceivedMint {
                    opid: "opid".to_owned(),
                    lock_id: "lock".to_owned(),
                    amount: 1,
                    before_block: 2,
                    evm_chain_id: 3,
                    bridge_contract: "bridge".to_owned(),
                },
            )]),
            rgb_store: s,
            reuse_address_index: HashMap::new(),
        };

        // JSON round-trip (what IndexedDB does)
        let json = serde_json::to_string(&snapshot).expect("serialize snapshot to JSON");
        let restored_snapshot: WalletSnapshot =
            serde_json::from_str(&json).expect("deserialize snapshot from JSON");
        let restored_artifact = restored_snapshot
            .transfer_artifacts
            .get("funding-txid")
            .expect("restore pending transfer artifact");
        assert_eq!(
            restored_artifact.consignment_bytes.get("asset-id"),
            Some(&vec![1, 2, 3, 4])
        );
        assert_eq!(
            restored_artifact.signed_psbt.as_deref(),
            Some("signed-psbt")
        );
        assert_eq!(restored_artifact.fascia, Some(vec![7, 8]));

        assert_eq!(
            restored_snapshot.received_consignments,
            snapshot.received_consignments
        );
        assert_eq!(
            restored_snapshot.received_consignment_meta,
            snapshot.received_consignment_meta
        );
        assert_eq!(
            restored_snapshot.received_signed_txs,
            snapshot.received_signed_txs
        );
        assert_eq!(restored_snapshot.received_mints, snapshot.received_mints);

        let restored = deserialize_stock(&restored_snapshot.rgb_store).expect("deserialize stock");

        assert_eq!(
            serialize_stock(&restored).as_deref(),
            Some(restored_snapshot.rgb_store.as_str()),
            "every stored table must survive the round-trip"
        );

        // Contract must survive the round-trip
        assert!(
            restored
                .contracts()
                .any(|c| c.is_ok_and(|c| c.id == contract_id)),
            "Contract should exist after snapshot round-trip"
        );
    }
}
