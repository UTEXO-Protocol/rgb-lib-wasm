//! IndexedDB persistence for WASM wallet snapshots.

use bdk_wallet::{ChangeSet, KeychainKind};
use rexie::{ObjectStore, Rexie, TransactionMode};
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;

use std::collections::HashMap;

use crate::database::memory_db::InMemoryDb;

const DB_NAME: &str = "rgb_lib_wallet";
const DB_VERSION: u32 = 1;
const STORE_NAME: &str = "snapshots";

/// A serializable snapshot of wallet state for IndexedDB persistence.
#[derive(Serialize, Deserialize)]
pub struct WalletSnapshot {
    /// The in-memory RGB-lib database.
    pub db: InMemoryDb,
    /// The BDK wallet changeset.
    pub bdk_changeset: Option<ChangeSet>,
    /// Signed PSBTs keyed by txid (for refresh after page reload).
    #[serde(default)]
    pub signed_psbts: HashMap<String, String>,
    /// Received consignment bytes keyed by recipient_id.
    #[serde(default)]
    pub received_consignments: HashMap<String, Vec<u8>>,
    /// Strict-encoded RGB Stock stash (base64).
    #[serde(default)]
    pub stock_stash_b64: Option<String>,
    /// Strict-encoded RGB Stock state (base64).
    #[serde(default)]
    pub stock_state_b64: Option<String>,
    /// Strict-encoded RGB Stock index (base64).
    #[serde(default)]
    pub stock_index_b64: Option<String>,
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

/// Serialize an RGB Stock into base64 components for snapshot persistence.
///
/// Returns `(stash_b64, state_b64, index_b64)` or `None` if serialization fails.
pub fn serialize_stock(stock: &rgbstd::persistence::Stock) -> Option<(String, String, String)> {
    use base64::{Engine as _, engine::general_purpose};
    use strict_encoding::StrictSerialize;

    let stash = stock
        .as_stash_provider()
        .to_strict_serialized::<{ u32::MAX as usize }>()
        .ok()?;
    let state = stock
        .as_state_provider()
        .to_strict_serialized::<{ u32::MAX as usize }>()
        .ok()?;
    let index = stock
        .as_index_provider()
        .to_strict_serialized::<{ u32::MAX as usize }>()
        .ok()?;

    Some((
        general_purpose::STANDARD.encode(stash.as_unconfined()),
        general_purpose::STANDARD.encode(state.as_unconfined()),
        general_purpose::STANDARD.encode(index.as_unconfined()),
    ))
}

/// Deserialize an RGB Stock from base64 components.
pub fn deserialize_stock(
    stash_b64: &str,
    state_b64: &str,
    index_b64: &str,
) -> Result<rgbstd::persistence::Stock, String> {
    use base64::{Engine as _, engine::general_purpose};
    use rgbstd::persistence::{MemIndex, MemStash, MemState};
    use strict_encoding::StrictDeserialize;

    const MAX: usize = u32::MAX as usize;

    let stash_bytes = general_purpose::STANDARD
        .decode(stash_b64)
        .map_err(|e| e.to_string())?;
    let state_bytes = general_purpose::STANDARD
        .decode(state_b64)
        .map_err(|e| e.to_string())?;
    let index_bytes = general_purpose::STANDARD
        .decode(index_b64)
        .map_err(|e| e.to_string())?;

    let stash = MemStash::from_strict_serialized::<MAX>(
        amplify::confinement::Confined::<Vec<u8>, 0, MAX>::try_from(stash_bytes)
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let state = MemState::from_strict_serialized::<MAX>(
        amplify::confinement::Confined::<Vec<u8>, 0, MAX>::try_from(state_bytes)
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let index = MemIndex::from_strict_serialized::<MAX>(
        amplify::confinement::Confined::<Vec<u8>, 0, MAX>::try_from(index_bytes)
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    Ok(rgbstd::persistence::Stock::with(stash, state, index))
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
    use rgbstd::persistence::Stock;
    use schemata::NonInflatableAsset;

    #[test]
    fn stock_serialize_round_trip_empty() {
        let stock = Stock::in_memory();
        let (s, st, i) = serialize_stock(&stock).expect("serialize empty stock");

        let restored = deserialize_stock(&s, &st, &i).expect("deserialize empty stock");
        assert_eq!(restored.contracts().unwrap().count(), 0);
    }

    #[test]
    fn stock_serialize_round_trip_with_contract() {
        use crate::{AssetSpec, ContractTerms, DumbResolver, Precision, RicardianContract};
        use rgbstd::contract::ContractBuilder;
        use rgbstd::txout::BlindSeal;
        use rgbstd::{Amount, ChainNet, Identity, Txid as RgbTxid};

        let mut stock = Stock::in_memory();

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
            NonInflatableAsset::schema(),
            NonInflatableAsset::types(),
            NonInflatableAsset::scripts(),
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
        assert!(stock.contracts().unwrap().any(|c| c.id == contract_id));

        // Serialize → JSON round-trip (simulates IndexedDB save/load)
        let (s, st, i) = serialize_stock(&stock).expect("serialize stock with contract");

        let snapshot = WalletSnapshot {
            db: InMemoryDb::new(),
            bdk_changeset: None,
            signed_psbts: HashMap::new(),
            received_consignments: HashMap::new(),
            stock_stash_b64: Some(s),
            stock_state_b64: Some(st),
            stock_index_b64: Some(i),
            reuse_address_index: HashMap::new(),
        };

        // JSON round-trip (what IndexedDB does)
        let json = serde_json::to_string(&snapshot).expect("serialize snapshot to JSON");
        let restored_snapshot: WalletSnapshot =
            serde_json::from_str(&json).expect("deserialize snapshot from JSON");

        let restored = deserialize_stock(
            restored_snapshot.stock_stash_b64.as_ref().unwrap(),
            restored_snapshot.stock_state_b64.as_ref().unwrap(),
            restored_snapshot.stock_index_b64.as_ref().unwrap(),
        )
        .expect("deserialize stock");

        // Contract must survive the round-trip
        assert!(
            restored.contracts().unwrap().any(|c| c.id == contract_id),
            "Contract should exist after snapshot round-trip"
        );
    }
}
