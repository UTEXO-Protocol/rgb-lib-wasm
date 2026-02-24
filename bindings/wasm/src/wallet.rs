use std::cell::RefCell;
use std::collections::HashMap;
use std::panic;
use wasm_bindgen::prelude::*;
use rgb_lib::wallet::{
    Wallet as RgbWallet, WalletData as RgbWalletData, DatabaseType,
    Online as RgbOnline, BtcBalance as RgbBtcBalance, Balance as RgbBalance,
    RefreshFilter, ReceiveData as RgbReceiveData,
};
use rgb_lib::{AssetSchema, Assignment};
use crate::error::RgbLibError;
use crate::utils::Network;

// -----------------------------------------------------------------------------
// Why we use a registry and why "returning the wallet from go_online" fails
// -----------------------------------------------------------------------------
//
// In wasm-bindgen, when a Rust async function returns a struct to JavaScript:
// - The struct is handed over to JS (moved into the wasm "slab" / table).
// - The JS object holds an index into that table.
//
// When you call an async method with &mut self (e.g. go_online(&mut self, ...)):
// - The Future captures a reference to self for the duration of the async.
// - After an .await, the runtime may not guarantee that the same "self" is used
//   when the method is invoked from JS, or the same table slot may not be
//   updated with the mutated state. So mutations (like setting online_data on
//   the inner RgbWallet) can be lost: the JS side may later call getBtcBalance
//   on a different Rust view of the object (e.g. a copy or a different slot)
//   that never had go_online applied.
//
// So we avoid passing the wallet through the async boundary at all:
// - The real RgbWallet lives in WALLET_REGISTRY (keyed by id).
// - The Wallet type in JS is just a handle { id: u32 }.
// - go_online: we remove the wallet from the registry by id, run go_online_async
//   (which sets online_data), then insert it back under the same id.
// - get_btc_balance: we look up by the same id and use that wallet.
// So the same RgbWallet instance is always used; no "loss" of the online state.
// The registry is still required: without it, get_btc_balance would see "Wallet is offline".
//
thread_local! {
    static WALLET_REGISTRY: RefCell<HashMap<u32, RgbWallet>> = RefCell::new(HashMap::new());
    static NEXT_WALLET_ID: RefCell<u32> = RefCell::new(0);
}

/// Puts the wallet back into the registry when dropped (e.g. after sync, including on panic).
/// This avoids "recursive use of an object" when sync() panics and the RefCell borrow is never released.
struct SyncGuard {
    id: u32,
    wallet: Option<RgbWallet>,
}

impl Drop for SyncGuard {
    fn drop(&mut self) {
        if let Some(w) = self.wallet.take() {
            WALLET_REGISTRY.with(|reg| reg.borrow_mut().insert(self.id, w));
        }
    }
}

fn next_wallet_id() -> u32 {
    NEXT_WALLET_ID.with(|cell| {
        let mut id = cell.borrow_mut();
        let out = *id;
        *id = id.saturating_add(1);
        out
    })
}

/// Sync wallet UTXOs from Esplora (async). Call from JS as syncWalletAsync(walletId, online).
/// Always exported so the test can use it; on non-WASM builds returns an error.
#[wasm_bindgen(js_name = syncWalletAsync)]
pub async fn sync_wallet_async(wallet_id: u32, online: &Online) -> Result<(), RgbLibError> {
    #[cfg(target_arch = "wasm32")]
    {
        let mut wallet = WALLET_REGISTRY
            .with(|reg| reg.borrow_mut().remove(&wallet_id))
            .ok_or_else(|| RgbLibError::new("Wallet not found".to_string()))?;
        wallet.sync_async(online.inner.clone()).await.map_err(RgbLibError::from)?;
        WALLET_REGISTRY.with(|reg| reg.borrow_mut().insert(wallet_id, wallet));
        Ok(())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (wallet_id, online);
        Err(RgbLibError::new("syncWalletAsync only available in WASM build".to_string()))
    }
}

// WASM Wallet bindings: direct rgb-lib dependency, in-memory mode via data_dir = ":memory:",
// persist via export_state() / from_state() and JS storage (IndexedDB, LocalStorage).

/// Online session for Esplora indexer (from go_online). Pass to get_btc_balance / refresh.
#[wasm_bindgen]
pub struct Online {
    inner: RgbOnline,
}

#[wasm_bindgen]
impl Online {
    #[wasm_bindgen(getter)]
    pub fn id(&self) -> u64 {
        self.inner.id
    }
    #[wasm_bindgen(getter, js_name = indexerUrl)]
    pub fn indexer_url(&self) -> String {
        self.inner.indexer_url.clone()
    }
}

/// Balance (sats): settled, future, spendable.
#[wasm_bindgen]
pub struct Balance {
    inner: RgbBalance,
}

#[wasm_bindgen]
impl Balance {
    #[wasm_bindgen(getter)]
    pub fn settled(&self) -> u64 {
        self.inner.settled
    }
    #[wasm_bindgen(getter)]
    pub fn future(&self) -> u64 {
        self.inner.future
    }
    #[wasm_bindgen(getter)]
    pub fn spendable(&self) -> u64 {
        self.inner.spendable
    }
}

/// BTC balance: vanilla + colored wallets.
#[wasm_bindgen]
pub struct BtcBalance {
    inner: RgbBtcBalance,
}

#[wasm_bindgen]
impl BtcBalance {
    #[wasm_bindgen(getter)]
    pub fn vanilla(&self) -> Balance {
        Balance { inner: self.inner.vanilla.clone() }
    }
    #[wasm_bindgen(getter)]
    pub fn colored(&self) -> Balance {
        Balance { inner: self.inner.colored.clone() }
    }
}

/// Result of blind_receive: invoice string and receive operation id.
#[wasm_bindgen]
pub struct ReceiveData {
    inner: RgbReceiveData,
}

#[wasm_bindgen]
impl ReceiveData {
    /// RGB invoice string (share this to receive a transfer).
    #[wasm_bindgen(getter)]
    pub fn invoice(&self) -> String {
        self.inner.invoice.clone()
    }
    #[wasm_bindgen(js_name = recipientId)]
    pub fn recipient_id(&self) -> String {
        self.inner.recipient_id.clone()
    }
    #[wasm_bindgen(js_name = expirationTimestamp)]
    pub fn expiration_timestamp(&self) -> Option<i64> {
        self.inner.expiration_timestamp
    }
    #[wasm_bindgen(js_name = batchTransferIdx)]
    pub fn batch_transfer_idx(&self) -> i32 {
        self.inner.batch_transfer_idx
    }
}

/// WASM wallet handle: holds only an id; the real RgbWallet lives in WALLET_REGISTRY so that
/// go_online and get_btc_balance always use the same instance.
#[wasm_bindgen]
pub struct Wallet {
    id: u32,
}

#[wasm_bindgen]
impl Wallet {
    /// Registry id (for debugging: same id for goOnline and getBtcBalance). Use getWalletId() in JS.
    #[wasm_bindgen(getter, js_name = "walletId")]
    pub fn wallet_id(&self) -> u32 {
        self.id
    }

    /// Same as walletId getter; use if getter is undefined (e.g. after async return).
    #[wasm_bindgen(js_name = getWalletId)]
    pub fn get_wallet_id(&self) -> u32 {
        self.id
    }

    /// Create a new RGB wallet for WASM environment.
    /// 
    /// Uses in-memory mode (data_dir = ":memory:") which doesn't require
    /// file system access. Wallet state must be exported/imported for persistence.
    #[wasm_bindgen(constructor)]
    pub fn new(
        wallet_data_json: String,
    ) -> Result<Wallet, RgbLibError> {
        // Parse wallet data from JSON
        let mut wallet_data: RgbWalletData = serde_json::from_str(&wallet_data_json)
            .map_err(|e| RgbLibError::new(format!("Failed to parse wallet data: {}", e)))?;
        
        // Override data_dir to use in-memory mode
        // This tells rgb-lib to use:
        // - sqlite::memory: for SQLite database
        // - Stock::in_memory() for RGB runtime
        // - No file system operations
        wallet_data.data_dir = ":memory:".to_string();
        
        // Create wallet with in-memory mode. Catch panic so JS gets Err instead of "unreachable".
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            RgbWallet::new(wallet_data).map_err(RgbLibError::from)
        }));
        let wallet = match result {
            Ok(Ok(w)) => w,
            Ok(Err(e)) => return Err(e),
            Err(panic_payload) => {
                let msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                    format!("Wallet creation failed (panic): {}", s)
                } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                    format!("Wallet creation failed (panic): {}", s)
                } else {
                    "Wallet creation failed (panic). Check browser console for details.".to_string()
                };
                return Err(RgbLibError::new(msg));
            }
        };
        
        let id = next_wallet_id();
        WALLET_REGISTRY.with(|reg| reg.borrow_mut().insert(id, wallet));
        Ok(Wallet { id })
    }

    /// Create a new RGB wallet asynchronously (returns a Promise).
    /// Use this instead of `new Wallet()` in the browser to avoid "time not implemented" panic.
    #[wasm_bindgen(js_name = createWallet)]
    pub async fn create_wallet_async(wallet_data_json: String) -> Result<Wallet, JsValue> {
        let mut wallet_data: RgbWalletData = serde_json::from_str(&wallet_data_json)
            .map_err(|e| JsValue::from_str(&format!("Failed to parse wallet data: {}", e)))?;
        wallet_data.data_dir = ":memory:".to_string();
        let wallet = RgbWallet::new_async(wallet_data)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let id = next_wallet_id();
        WALLET_REGISTRY.with(|reg| reg.borrow_mut().insert(id, wallet));
        Ok(Wallet { id })
    }

    /// Get wallet data as JSON string
    #[wasm_bindgen]
    pub fn get_wallet_data(&self) -> Result<String, RgbLibError> {
        WALLET_REGISTRY.with(|reg| {
            let reg = reg.borrow();
            let wallet = reg.get(&self.id).ok_or_else(|| RgbLibError::new("Wallet not found".to_string()))?;
            let wallet_data = wallet.get_wallet_data();
            serde_json::to_string(&wallet_data).map_err(|e| RgbLibError::new(format!("Failed to serialize: {}", e)))
        })
    }

    /// Export wallet state for persistence (for in-memory wallets)
    #[wasm_bindgen]
    pub fn export_state(&self) -> Result<String, RgbLibError> {
        WALLET_REGISTRY.with(|reg| {
            let reg = reg.borrow();
            let wallet = reg.get(&self.id).ok_or_else(|| RgbLibError::new("Wallet not found".to_string()))?;
            wallet.export_state().map_err(RgbLibError::from)
        })
    }

    /// Restore wallet from exported state (for in-memory wallets)
    #[wasm_bindgen]
    pub fn from_state(state_json: String) -> Result<Wallet, RgbLibError> {
        let wallet = RgbWallet::from_state(state_json).map_err(RgbLibError::from)?;
        let id = next_wallet_id();
        WALLET_REGISTRY.with(|reg| reg.borrow_mut().insert(id, wallet));
        Ok(Wallet { id })
    }

    /// Connect to Esplora indexer (online mode). Returns a Promise. Same wallet instance is used for getBtcBalance.
    #[wasm_bindgen(js_name = goOnline)]
    pub async fn go_online(&mut self, skip_consistency_check: bool, indexer_url: String) -> Result<Online, RgbLibError> {
        let id = self.id;
        let mut wallet = WALLET_REGISTRY
            .with(|reg| reg.borrow_mut().remove(&id))
            .ok_or_else(|| RgbLibError::new("Wallet not found".to_string()))?;
        let online_inner = wallet.go_online_async(skip_consistency_check, indexer_url).await.map_err(RgbLibError::from)?;
        WALLET_REGISTRY.with(|reg| reg.borrow_mut().insert(id, wallet));
        Ok(Online { inner: online_inner })
    }

    /// Get BTC balance (sats). Pass the Online from go_online.
    /// On wasm32 use skip_sync=true: full sync uses std::time in the indexer and panics in the browser.
    #[wasm_bindgen(js_name = getBtcBalance)]
    pub fn get_btc_balance(&mut self, online: &Online, skip_sync: bool) -> Result<BtcBalance, RgbLibError> {
        WALLET_REGISTRY.with(|reg| {
            let mut reg = reg.borrow_mut();
            let wallet = reg.get_mut(&self.id).ok_or_else(|| RgbLibError::new("Wallet not found".to_string()))?;
            let balance = wallet.get_btc_balance(Some(online.inner.clone()), skip_sync).map_err(RgbLibError::from)?;
            Ok(BtcBalance { inner: balance })
        })
    }

    /// Get BTC balance (sats) from cached data only, no sync.
    #[wasm_bindgen(js_name = getBtcBalanceCached)]
    pub fn get_btc_balance_cached(&mut self, skip_sync: bool) -> Result<BtcBalance, RgbLibError> {
        WALLET_REGISTRY.with(|reg| {
            let mut reg = reg.borrow_mut();
            let wallet = reg.get_mut(&self.id).ok_or_else(|| RgbLibError::new("Wallet not found".to_string()))?;
            let balance = wallet.get_btc_balance(None, skip_sync).map_err(RgbLibError::from)?;
            Ok(BtcBalance { inner: balance })
        })
    }

    /// Get a new Bitcoin address (vanilla wallet, internal keychain).
    #[wasm_bindgen(js_name = getAddress)]
    pub fn get_address(&mut self) -> Result<String, RgbLibError> {
        WALLET_REGISTRY.with(|reg| {
            let mut reg = reg.borrow_mut();
            let wallet = reg.get_mut(&self.id).ok_or_else(|| RgbLibError::new("Wallet not found".to_string()))?;
            wallet.get_address().map_err(RgbLibError::from)
        })
    }

    /// Sync UTXOs from the indexer (Esplora). Call this after receiving BTC to update balance.
    /// In browser the indexer may panic ("time not implemented"). We remove the wallet from the
    /// registry before calling sync so that on panic no RefCell borrow is held; SyncGuard re-inserts
    /// the wallet on drop so getBtcBalance works after.
    #[wasm_bindgen]
    pub fn sync(&mut self, online: &Online) -> Result<(), RgbLibError> {
        let id = self.id;
        let wallet = WALLET_REGISTRY
            .with(|reg| reg.borrow_mut().remove(&id))
            .ok_or_else(|| RgbLibError::new("Wallet not found".to_string()))?;
        let mut _guard = SyncGuard {
            id,
            wallet: Some(wallet),
        };
        // Use the wallet from the guard; sync may panic in browser.
        let wallet_ref = _guard.wallet.as_mut().ok_or_else(|| RgbLibError::new("Wallet not found".to_string()))?;
        wallet_ref.sync(online.inner.clone()).map_err(RgbLibError::from)
    }

    /// Async sync for WASM: syncs UTXOs from Esplora without blocking; use this in the browser
    /// so balance updates after receiving BTC. Returns a Promise.
    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen(js_name = syncAsync)]
    pub async fn sync_async(&mut self, online: &Online) -> Result<(), RgbLibError> {
        sync_wallet_async(self.id, online).await
    }

    /// Refresh transfers. asset_id: optional asset ID; filter_json: "[]" for all.
    #[wasm_bindgen]
    pub fn refresh(
        &mut self,
        online: &Online,
        asset_id: Option<String>,
        filter_json: String,
        skip_sync: bool,
    ) -> Result<String, RgbLibError> {
        let filter: Vec<RefreshFilter> = if filter_json.trim().is_empty() || filter_json == "[]" {
            vec![]
        } else {
            serde_json::from_str(&filter_json).map_err(|e| RgbLibError::new(format!("Invalid refresh filter JSON: {}", e)))?
        };
        WALLET_REGISTRY.with(|reg| {
            let mut reg = reg.borrow_mut();
            let wallet = reg.get_mut(&self.id).ok_or_else(|| RgbLibError::new("Wallet not found".to_string()))?;
            let result = wallet.refresh(online.inner.clone(), asset_id, filter, skip_sync).map_err(RgbLibError::from)?;
            serde_json::to_string(&result).map_err(|e| RgbLibError::new(format!("Serialize: {}", e)))
        })
    }

    /// Create UTXOs for RGB allocations (sync path; uses blocking HTTP — fails in browser with "operation not supported").
    #[wasm_bindgen(js_name = createUtxos)]
    pub fn create_utxos(
        &mut self,
        online: &Online,
        up_to: bool,
        num: Option<u8>,
        size: Option<u32>,
        fee_rate: u64,
        skip_sync: bool,
    ) -> Result<u8, RgbLibError> {
        WALLET_REGISTRY.with(|reg| {
            let mut reg = reg.borrow_mut();
            let wallet = reg.get_mut(&self.id).ok_or_else(|| RgbLibError::new("Wallet not found".to_string()))?;
            wallet
                .create_utxos(online.inner.clone(), up_to, num, size, fee_rate, skip_sync)
                .map_err(RgbLibError::from)
        })
    }

    /// Create UTXOs for RGB allocations (async; use in browser — broadcasts via fetch instead of Minreq).
    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen(js_name = createUtxosAsync)]
    pub async fn create_utxos_async(
        &mut self,
        online: &Online,
        up_to: bool,
        num: Option<u8>,
        size: Option<u32>,
        fee_rate: u64,
        skip_sync: bool,
    ) -> Result<u8, RgbLibError> {
        let id = self.id;
        let mut wallet = WALLET_REGISTRY
            .with(|reg| reg.borrow_mut().remove(&id))
            .ok_or_else(|| RgbLibError::new("Wallet not found".to_string()))?;
        let result = wallet
            .create_utxos_async(
                online.inner.clone(),
                up_to,
                num,
                size,
                fee_rate,
                skip_sync,
            )
            .await;
        // Always re-insert wallet so later calls (e.g. blind_receive) don't get "Wallet not found".
        WALLET_REGISTRY.with(|reg| reg.borrow_mut().insert(id, wallet));
        result.map_err(RgbLibError::from)
    }

    /// Blind receive: create an RGB invoice (any asset/amount). Requires at least one UTXO in the wallet.
    /// assignment: "Any" for any asset/amount; min_confirmations: e.g. 1.
    #[wasm_bindgen(js_name = blindReceive)]
    pub fn blind_receive(
        &self,
        asset_id: Option<String>,
        assignment: String,
        duration_seconds: Option<u32>,
        transport_endpoints: Vec<String>,
        min_confirmations: u8,
    ) -> Result<ReceiveData, RgbLibError> {
        let assignment_enum = match assignment.trim().eq_ignore_ascii_case("Any") {
            true => Assignment::Any,
            false => {
                // Optional: "Fungible:123" for amount; for now only Any
                return Err(RgbLibError::new(format!(
                    "WASM blind_receive supports assignment \"Any\" only, got: {}",
                    assignment
                )));
            }
        };
        WALLET_REGISTRY.with(|reg| {
            let reg = reg.borrow();
            let wallet = reg.get(&self.id).ok_or_else(|| RgbLibError::new("Wallet not found".to_string()))?;
            let data = wallet
                .blind_receive(
                    asset_id,
                    assignment_enum,
                    duration_seconds,
                    transport_endpoints,
                    min_confirmations,
                )
                .map_err(RgbLibError::from)?;
            Ok(ReceiveData { inner: data })
        })
    }
}

// Helper function to create WalletData from components
#[wasm_bindgen]
pub fn create_wallet_data(
    network: Network,
    account_xpub_vanilla: String,
    account_xpub_colored: String,
    mnemonic: Option<String>,
    master_fingerprint: String,
    max_allocations_per_utxo: u32,
    supported_schemas: Vec<String>, // AssetSchema names as strings
) -> Result<String, RgbLibError> {
    let schemas: Result<Vec<AssetSchema>, _> = supported_schemas
        .iter()
        .map(|s| {
            match s.as_str() {
                "NIA" => Ok(AssetSchema::Nia),
                "CFA" => Ok(AssetSchema::Cfa),
                "IFA" => Ok(AssetSchema::Ifa),
                "UDA" => Ok(AssetSchema::Uda),
                _ => Err(format!("Unknown schema: {}", s)),
            }
        })
        .collect();
    
    let schemas = schemas.map_err(|e| RgbLibError::new(e))?;
    
    if schemas.is_empty() {
        return Err(RgbLibError::new("At least one supported schema is required".to_string()));
    }

    let wallet_data = RgbWalletData {
        data_dir: ":memory:".to_string(), // In-memory mode for WASM
        bitcoin_network: network.into(),
        database_type: DatabaseType::Sqlite,
        max_allocations_per_utxo,
        account_xpub_vanilla,
        account_xpub_colored,
        mnemonic,
        master_fingerprint,
        vanilla_keychain: None,
        supported_schemas: schemas,
    };

    serde_json::to_string(&wallet_data)
        .map_err(|e| RgbLibError::new(format!("Failed to serialize wallet data: {}", e)))
}
