//! IndexedDB persistence for WASM wallet snapshots.

use bdk_wallet::{ChangeSet, KeychainKind};
use rexie::{ObjectStore, Rexie, TransactionMode};
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;

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
    /// Received mints keyed by recipient_id.
    #[serde(default)]
    pub(crate) received_mints: HashMap<String, super::ReceivedMint>,
    /// Scanned EVM blocks of the received mints keyed by recipient_id.
    #[serde(default)]
    pub mint_scanned_blocks: HashMap<String, HashMap<String, u64>>,
    /// Encoded RGB store (base64).
    #[serde(default)]
    pub rgb_store: Option<String>,
    /// Pinned derivation index per keychain for address reuse.
    #[serde(default)]
    pub reuse_address_index: HashMap<KeychainKind, u32>,
}

async fn open_db() -> Result<Rexie, String> {
    let rexie = Rexie::builder(DB_NAME)
        .version(DB_VERSION)
        .add_object_store(ObjectStore::new(STORE_NAME))
        .build()
        .await
        .map_err(|e| format!("IndexedDB open error: {e:?}"))?;
    Ok(rexie)
}

/// Save a wallet snapshot to IndexedDB under the given key.
pub async fn save_snapshot(key: &str, snapshot: &WalletSnapshot) -> Result<(), String> {
    let db = open_db().await?;
    let tx = db
        .transaction(&[STORE_NAME], TransactionMode::ReadWrite)
        .map_err(|e| format!("IndexedDB transaction error: {e:?}"))?;
    let store = tx
        .store(STORE_NAME)
        .map_err(|e| format!("IndexedDB store error: {e:?}"))?;

    let json_str = serde_json::to_string(snapshot).map_err(|e| format!("Serialize error: {e}"))?;
    let js_key = JsValue::from_str(key);
    if let Some(existing) = store
        .get(js_key.clone())
        .await
        .map_err(|e| format!("IndexedDB get error: {e:?}"))?
    {
        let json = existing
            .as_string()
            .ok_or_else(|| "IndexedDB value is not a string".to_string())?;
        let existing: WalletSnapshot =
            serde_json::from_str(&json).map_err(|e| format!("Deserialize error: {e}"))?;
        if existing.sequence >= snapshot.sequence {
            tx.done()
                .await
                .map_err(|e| format!("IndexedDB commit error: {e:?}"))?;
            return Ok(());
        }
    }
    let js_val = JsValue::from_str(&json_str);

    store
        .put(&js_val, Some(&js_key))
        .await
        .map_err(|e| format!("IndexedDB put error: {e:?}"))?;
    tx.done()
        .await
        .map_err(|e| format!("IndexedDB commit error: {e:?}"))?;
    Ok(())
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
        .transaction(&[STORE_NAME], TransactionMode::ReadOnly)
        .map_err(|e| format!("IndexedDB transaction error: {e:?}"))?;
    let store = tx
        .store(STORE_NAME)
        .map_err(|e| format!("IndexedDB store error: {e:?}"))?;

    let js_key = JsValue::from_str(key);
    let result = store
        .get(js_key)
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
        let stock = Stock::with(MemRgbStore::default());
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

        let mut stock = Stock::with(MemRgbStore::default());
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
        let snapshot = WalletSnapshot {
            sequence: 1,
            db: InMemoryDb::new(),
            bdk_changeset: None,
            transfer_artifacts: HashMap::from([("funding-txid".to_owned(), transfer_artifact)]),
            received_consignments: HashMap::new(),
            received_mints: HashMap::new(),
            mint_scanned_blocks: HashMap::new(),
            rgb_store: Some(s),
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

        let restored = deserialize_stock(restored_snapshot.rgb_store.as_ref().unwrap())
            .expect("deserialize stock");

        assert_eq!(
            serialize_stock(&restored).as_deref(),
            restored_snapshot.rgb_store.as_deref(),
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
