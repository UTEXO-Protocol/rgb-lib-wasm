// RefCell borrows held across .await are safe on wasm32 (single-threaded runtime).
#![allow(clippy::await_holding_refcell_ref)]

use serde::Serialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::panic;
use wasm_bindgen::prelude::*;

use rgb_lib_wasm::Wallet;
use rgb_lib_wasm::wallet::{Online, Recipient, RefreshFilter, WalletData};

/// Serialize to JsValue using BigInt for u64/i64 values that exceed Number.MAX_SAFE_INTEGER.
fn to_js<T: Serialize>(val: &T) -> Result<JsValue, JsValue> {
    let serializer =
        serde_wasm_bindgen::Serializer::new().serialize_large_number_types_as_bigints(true);
    val.serialize(&serializer)
        .map_err(|e| JsValue::from_str(&e.to_string()))
}

#[wasm_bindgen(js_namespace = console)]
extern "C" {
    #[wasm_bindgen(js_name = error)]
    fn console_error(s: &str);
}

#[wasm_bindgen(start)]
pub fn init() {
    let _ = instant::Instant::now();
    panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| {
                format!(
                    "at {}:{}",
                    l.file().rsplit('/').next().unwrap_or("unknown"),
                    l.line()
                )
            })
            .unwrap_or_default();
        let msg = format!("[rgb-lib WASM panic] {location}");
        console_error(&msg);
    }));
}

#[wasm_bindgen(js_name = "generateKeys")]
pub fn generate_keys(network: &str) -> Result<JsValue, JsValue> {
    let bitcoin_network = network
        .parse::<rgb_lib_wasm::BitcoinNetwork>()
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let keys = rgb_lib_wasm::generate_keys(bitcoin_network);
    to_js(&keys)
}

#[wasm_bindgen(js_name = "restoreKeys")]
pub fn restore_keys(network: &str, mnemonic: &str) -> Result<JsValue, JsValue> {
    let bitcoin_network = network
        .parse::<rgb_lib_wasm::BitcoinNetwork>()
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let keys = rgb_lib_wasm::restore_keys(bitcoin_network, mnemonic.to_string())
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&keys)
}

#[wasm_bindgen]
pub struct WasmWallet {
    inner: RefCell<Wallet>,
}

#[wasm_bindgen]
impl WasmWallet {
    /// Create a new RGB wallet from a JSON-encoded WalletData.
    #[wasm_bindgen(constructor)]
    pub fn new(wallet_data_json: &str) -> Result<WasmWallet, JsValue> {
        let wd: WalletData = serde_json::from_str(wallet_data_json)
            .map_err(|e| JsValue::from_str(&format!("Invalid WalletData JSON: {e}")))?;
        let wallet = Wallet::new(wd).map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(WasmWallet {
            inner: RefCell::new(wallet),
        })
    }

    /// Create a new RGB wallet with IndexedDB state restoration.
    ///
    /// Like `new()`, but asynchronously checks IndexedDB for a previously saved
    /// snapshot and restores it, so wallet state survives page refreshes.
    #[wasm_bindgen(js_name = "create")]
    pub async fn create(wallet_data_json: &str) -> Result<WasmWallet, JsValue> {
        let wd: WalletData = serde_json::from_str(wallet_data_json)
            .map_err(|e| JsValue::from_str(&format!("Invalid WalletData JSON: {e}")))?;
        let mut wallet = Wallet::new(wd).map_err(|e| JsValue::from_str(&e.to_string()))?;
        let idb_key = wallet.idb_key();
        match rgb_lib_wasm::wallet::idb_store::load_snapshot(&idb_key).await {
            Ok(Some(snapshot)) => {
                wallet
                    .restore_from_snapshot(snapshot)
                    .map_err(|e| JsValue::from_str(&e.to_string()))?;
            }
            Ok(None) => {}
            Err(e) => {
                web_sys::console::warn_1(
                    &format!("IDB load warning (continuing fresh): {e}").into(),
                );
            }
        }
        Ok(WasmWallet {
            inner: RefCell::new(wallet),
        })
    }

    /// Return the WalletData as a JS object.
    #[wasm_bindgen(js_name = "getWalletData")]
    pub fn get_wallet_data(&self) -> Result<JsValue, JsValue> {
        let wd = self.inner.borrow().get_wallet_data();
        to_js(&wd)
    }

    /// Issue a new NIA (Non-Inflatable Asset).
    ///
    /// `amounts_js` is a JS array of u64 values.
    #[wasm_bindgen(js_name = "issueAssetNia")]
    pub fn issue_asset_nia(
        &self,
        ticker: &str,
        name: &str,
        precision: u8,
        amounts_js: JsValue,
    ) -> Result<JsValue, JsValue> {
        let amounts: Vec<u64> = serde_wasm_bindgen::from_value(amounts_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid amounts array: {e}")))?;
        let asset = self
            .inner
            .borrow()
            .issue_asset_nia(ticker.to_string(), name.to_string(), precision, amounts)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&asset)
    }

    /// Issue a new IFA (Inflatable Fungible Asset).
    ///
    /// `amounts_js` is a JS array of u64 values.
    /// `inflation_amounts_js` is a JS array of u64 values for inflation allowances.
    #[wasm_bindgen(js_name = "issueAssetIfa")]
    pub fn issue_asset_ifa(
        &self,
        ticker: &str,
        name: &str,
        precision: u8,
        amounts_js: JsValue,
        inflation_amounts_js: JsValue,
        reject_list_url: Option<String>,
    ) -> Result<JsValue, JsValue> {
        let amounts: Vec<u64> = serde_wasm_bindgen::from_value(amounts_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid amounts array: {e}")))?;
        let inflation_amounts: Vec<u64> = serde_wasm_bindgen::from_value(inflation_amounts_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid inflation_amounts array: {e}")))?;
        let asset = self
            .inner
            .borrow()
            .issue_asset_ifa(
                ticker.to_string(),
                name.to_string(),
                precision,
                amounts,
                inflation_amounts,
                reject_list_url,
            )
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&asset)
    }

    /// Return the BTC balance. Always skips sync on wasm32.
    #[wasm_bindgen(js_name = "getBtcBalance")]
    pub fn get_btc_balance(&self) -> Result<JsValue, JsValue> {
        let balance = self
            .inner
            .borrow_mut()
            .get_btc_balance(None, true)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&balance)
    }

    /// Return metadata for a specific asset (name, ticker, precision, supply, etc.).
    #[wasm_bindgen(js_name = "getAssetMetadata")]
    pub fn get_asset_metadata(&self, asset_id: &str) -> Result<JsValue, JsValue> {
        let metadata = self
            .inner
            .borrow()
            .get_asset_metadata(asset_id.to_string())
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&metadata)
    }

    /// Return the balance for a specific asset.
    #[wasm_bindgen(js_name = "getAssetBalance")]
    pub fn get_asset_balance(&self, asset_id: &str) -> Result<JsValue, JsValue> {
        let balance = self
            .inner
            .borrow()
            .get_asset_balance(asset_id.to_string())
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&balance)
    }

    /// List known RGB assets. Pass a JS array of schema strings to filter, or empty for all.
    #[wasm_bindgen(js_name = "listAssets")]
    pub fn list_assets(&self, filter_asset_schemas_js: JsValue) -> Result<JsValue, JsValue> {
        let schemas: Vec<rgb_lib_wasm::AssetSchema> =
            serde_wasm_bindgen::from_value(filter_asset_schemas_js)
                .map_err(|e| JsValue::from_str(&format!("Invalid schemas: {e}")))?;
        let assets = self
            .inner
            .borrow()
            .list_assets(schemas)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&assets)
    }

    /// List RGB transfers, optionally filtered by asset ID.
    #[wasm_bindgen(js_name = "listTransfers")]
    pub fn list_transfers(&self, asset_id: Option<String>) -> Result<JsValue, JsValue> {
        let transfers = self
            .inner
            .borrow()
            .list_transfers(asset_id)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&transfers)
    }

    /// List unspent outputs. Always skips sync on wasm32.
    #[wasm_bindgen(js_name = "listUnspents")]
    pub fn list_unspents(&self, settled_only: bool) -> Result<JsValue, JsValue> {
        let unspents = self
            .inner
            .borrow_mut()
            .list_unspents(None, settled_only, true)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&unspents)
    }

    /// List Bitcoin transactions. Always skips sync on wasm32.
    #[wasm_bindgen(js_name = "listTransactions")]
    pub fn list_transactions(&self) -> Result<JsValue, JsValue> {
        let txs = self
            .inner
            .borrow_mut()
            .list_transactions(None, true)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&txs)
    }

    /// Sign a PSBT (base64-encoded). Returns the signed PSBT string.
    #[wasm_bindgen(js_name = "signPsbt")]
    pub fn sign_psbt(&self, unsigned_psbt: &str) -> Result<String, JsValue> {
        self.inner
            .borrow()
            .sign_psbt(unsigned_psbt.to_string(), None)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Finalize a signed PSBT (base64-encoded). Returns the finalized PSBT string.
    #[wasm_bindgen(js_name = "finalizePsbt")]
    pub fn finalize_psbt(&self, signed_psbt: &str) -> Result<String, JsValue> {
        self.inner
            .borrow()
            .finalize_psbt(signed_psbt.to_string(), None)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Blind an UTXO to receive RGB assets. Returns ReceiveData as a JS object.
    ///
    /// `assignment_js` is a JS object like `{ "Fungible": 100 }` or `"NonFungible"` or `"Any"`.
    /// `transport_endpoints_js` is a JS array of endpoint strings.
    #[wasm_bindgen(js_name = "blindReceive")]
    pub fn blind_receive(
        &self,
        asset_id: Option<String>,
        assignment_js: JsValue,
        duration_seconds: Option<u32>,
        transport_endpoints_js: JsValue,
        min_confirmations: u8,
    ) -> Result<JsValue, JsValue> {
        let assignment: rgb_lib_wasm::Assignment = serde_wasm_bindgen::from_value(assignment_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid assignment: {e}")))?;
        let transport_endpoints: Vec<String> =
            serde_wasm_bindgen::from_value(transport_endpoints_js)
                .map_err(|e| JsValue::from_str(&format!("Invalid transport endpoints: {e}")))?;
        let data = self
            .inner
            .borrow()
            .blind_receive(
                asset_id,
                assignment,
                duration_seconds,
                transport_endpoints,
                min_confirmations,
            )
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&data)
    }

    /// Create an address to receive RGB assets via witness TX. Returns ReceiveData as a JS object.
    #[wasm_bindgen(js_name = "witnessReceive")]
    pub fn witness_receive(
        &self,
        asset_id: Option<String>,
        assignment_js: JsValue,
        duration_seconds: Option<u32>,
        transport_endpoints_js: JsValue,
        min_confirmations: u8,
    ) -> Result<JsValue, JsValue> {
        let assignment: rgb_lib_wasm::Assignment = serde_wasm_bindgen::from_value(assignment_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid assignment: {e}")))?;
        let transport_endpoints: Vec<String> =
            serde_wasm_bindgen::from_value(transport_endpoints_js)
                .map_err(|e| JsValue::from_str(&format!("Invalid transport endpoints: {e}")))?;
        let data = self
            .inner
            .borrow_mut()
            .witness_receive(
                asset_id,
                assignment,
                duration_seconds,
                transport_endpoints,
                min_confirmations,
            )
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&data)
    }

    /// Return a new Bitcoin address from the vanilla wallet.
    #[wasm_bindgen(js_name = "getAddress")]
    pub fn get_address(&self) -> Result<String, JsValue> {
        self.inner
            .borrow_mut()
            .get_address()
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Delete failed transfers. Returns true if any were deleted.
    #[wasm_bindgen(js_name = "deleteTransfers")]
    pub fn delete_transfers(
        &self,
        batch_transfer_idx: Option<i32>,
        no_asset_only: bool,
    ) -> Result<bool, JsValue> {
        self.inner
            .borrow()
            .delete_transfers(batch_transfer_idx, no_asset_only)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Go online: connect to an indexer. Returns Online data as a JS object.
    #[wasm_bindgen(js_name = "goOnline")]
    pub async fn go_online(
        &self,
        skip_consistency_check: bool,
        indexer_url: &str,
    ) -> Result<JsValue, JsValue> {
        let mut wallet = self.inner.borrow_mut();
        let online = wallet
            .go_online(skip_consistency_check, indexer_url.to_string())
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&online)
    }

    /// Sync the wallet with the indexer.
    pub async fn sync(&self, online_js: JsValue) -> Result<(), JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .sync(online)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Create UTXOs (begin): prepare a PSBT to create new UTXOs for RGB allocations.
    /// Returns the unsigned PSBT string.
    #[wasm_bindgen(js_name = "createUtxosBegin")]
    pub async fn create_utxos_begin(
        &self,
        online_js: JsValue,
        up_to: bool,
        num: Option<u8>,
        size: Option<u32>,
        fee_rate: u64,
        skip_sync: bool,
    ) -> Result<String, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .create_utxos_begin(online, up_to, num, size, fee_rate, skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Create UTXOs (end): broadcast a signed PSBT to create new UTXOs.
    /// Returns the number of created UTXOs.
    #[wasm_bindgen(js_name = "createUtxosEnd")]
    pub async fn create_utxos_end(
        &self,
        online_js: JsValue,
        signed_psbt: &str,
        skip_sync: bool,
    ) -> Result<u8, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .create_utxos_end(online, signed_psbt.to_string(), skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Send RGB assets (begin): prepare a PSBT. Returns the unsigned PSBT string.
    ///
    /// `recipient_map_js` is a JS object mapping asset IDs to arrays of Recipient objects.
    #[wasm_bindgen(js_name = "sendBegin")]
    pub async fn send_begin(
        &self,
        online_js: JsValue,
        recipient_map_js: JsValue,
        donation: bool,
        fee_rate: u64,
        min_confirmations: u8,
    ) -> Result<String, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let recipient_map: HashMap<String, Vec<Recipient>> =
            serde_wasm_bindgen::from_value(recipient_map_js)
                .map_err(|e| JsValue::from_str(&format!("Invalid recipient map: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .send_begin(online, recipient_map, donation, fee_rate, min_confirmations)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Send RGB assets (end): broadcast a signed PSBT. Returns an OperationResult JS object.
    #[wasm_bindgen(js_name = "sendEnd")]
    pub async fn send_end(
        &self,
        online_js: JsValue,
        signed_psbt: &str,
        skip_sync: bool,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .send_end(online, signed_psbt.to_string(), skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Send BTC (begin): prepare a PSBT. Returns the unsigned PSBT string.
    #[wasm_bindgen(js_name = "sendBtcBegin")]
    pub async fn send_btc_begin(
        &self,
        online_js: JsValue,
        address: &str,
        amount: u64,
        fee_rate: u64,
        skip_sync: bool,
    ) -> Result<String, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .send_btc_begin(online, address.to_string(), amount, fee_rate, skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Send BTC (end): broadcast a signed PSBT. Returns the txid string.
    #[wasm_bindgen(js_name = "sendBtcEnd")]
    pub async fn send_btc_end(
        &self,
        online_js: JsValue,
        signed_psbt: &str,
        skip_sync: bool,
    ) -> Result<String, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .send_btc_end(online, signed_psbt.to_string(), skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Refresh pending transfers. Returns a RefreshResult JS object.
    ///
    /// `filter_js` is a JS array of RefreshFilter objects (or empty array for all).
    pub async fn refresh(
        &self,
        online_js: JsValue,
        asset_id: Option<String>,
        filter_js: JsValue,
        skip_sync: bool,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let filter: Vec<RefreshFilter> = serde_wasm_bindgen::from_value(filter_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid filter: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .refresh(online, asset_id, filter, skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Fail pending transfers. Returns true if any transfers were failed.
    #[wasm_bindgen(js_name = "failTransfers")]
    pub async fn fail_transfers(
        &self,
        online_js: JsValue,
        batch_transfer_idx: Option<i32>,
        no_asset_only: bool,
        skip_sync: bool,
    ) -> Result<bool, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .fail_transfers(online, batch_transfer_idx, no_asset_only, skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Get fee estimation for a target number of blocks.
    #[wasm_bindgen(js_name = "getFeeEstimation")]
    pub async fn get_fee_estimation(
        &self,
        online_js: JsValue,
        blocks: u16,
    ) -> Result<f64, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let wallet = self.inner.borrow();
        wallet
            .get_fee_estimation(online, blocks)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Drain all wallet funds (begin): prepare a PSBT. Returns the unsigned PSBT string.
    #[wasm_bindgen(js_name = "drainToBegin")]
    pub async fn drain_to_begin(
        &self,
        online_js: JsValue,
        address: &str,
        destroy_assets: bool,
        fee_rate: u64,
    ) -> Result<String, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .drain_to_begin(online, address.to_string(), destroy_assets, fee_rate)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Drain all wallet funds (end): broadcast a signed PSBT. Returns the txid string.
    #[wasm_bindgen(js_name = "drainToEnd")]
    pub async fn drain_to_end(
        &self,
        online_js: JsValue,
        signed_psbt: &str,
    ) -> Result<String, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .drain_to_end(online, signed_psbt.to_string())
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Inflate an IFA asset (begin): prepare a PSBT. Returns the unsigned PSBT string.
    ///
    /// `inflation_amounts_js` is a JS array of u64 values.
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = "inflateBegin")]
    pub async fn inflate_begin(
        &self,
        online_js: JsValue,
        asset_id: &str,
        inflation_amounts_js: JsValue,
        fee_rate: u64,
        min_confirmations: u8,
    ) -> Result<String, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let inflation_amounts: Vec<u64> = serde_wasm_bindgen::from_value(inflation_amounts_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid inflation_amounts array: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .inflate_begin(
                online,
                asset_id.to_string(),
                inflation_amounts,
                fee_rate,
                min_confirmations,
            )
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Inflate an IFA asset (end): broadcast a signed PSBT. Returns an OperationResult JS object.
    #[wasm_bindgen(js_name = "inflateEnd")]
    pub async fn inflate_end(
        &self,
        online_js: JsValue,
        signed_psbt: &str,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .inflate_end(online, signed_psbt.to_string())
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// List vanilla (non-colored) unspent outputs. Returns a JS array of LocalOutput objects.
    #[wasm_bindgen(js_name = "listUnspentsVanilla")]
    pub async fn list_unspents_vanilla(
        &self,
        online_js: JsValue,
        min_confirmations: u8,
        skip_sync: bool,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let unspents = wallet
            .list_unspents_vanilla(online, min_confirmations, skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&unspents)
    }

    /// Create an encrypted backup of the wallet state. Returns backup bytes as Uint8Array.
    pub fn backup(&self, password: &str) -> Result<Vec<u8>, JsValue> {
        self.inner
            .borrow()
            .backup(password)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Restore wallet state from an encrypted backup.
    #[wasm_bindgen(js_name = "restoreBackup")]
    pub fn restore_backup(&self, backup_bytes: &[u8], password: &str) -> Result<(), JsValue> {
        self.inner
            .borrow_mut()
            .restore_backup(backup_bytes, password)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Check if the wallet needs a backup. Returns true if modified since last backup.
    #[wasm_bindgen(js_name = "backupInfo")]
    pub fn backup_info(&self) -> Result<bool, JsValue> {
        self.inner
            .borrow()
            .backup_info()
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Configure VSS (cloud) backup for this wallet.
    ///
    /// `signing_key_hex` is the 32-byte secret key as a hex string (64 hex chars).
    ///
    /// **Security note:** The signing key crosses the JS/WASM boundary as a string. It will
    /// exist in V8's string pool and cannot be zeroed from Rust. Callers should avoid storing
    /// the key in JS longer than necessary (e.g., don't keep it in a global variable).
    #[wasm_bindgen(js_name = "configureVssBackup")]
    pub fn configure_vss_backup(
        &self,
        server_url: &str,
        store_id: &str,
        signing_key_hex: &str,
    ) -> Result<(), JsValue> {
        if signing_key_hex.len() != 64 {
            return Err(JsValue::from_str(&format!(
                "signing_key_hex must be exactly 64 hex chars (32 bytes), got {}",
                signing_key_hex.len()
            )));
        }
        let key_bytes = hex::decode(signing_key_hex)
            .map_err(|e| JsValue::from_str(&format!("Invalid signing key hex: {e}")))?;
        let signing_key =
            rgb_lib_wasm::bdk_wallet::bitcoin::secp256k1::SecretKey::from_slice(&key_bytes)
                .map_err(|e| JsValue::from_str(&format!("Invalid signing key: {e}")))?;
        let config = rgb_lib_wasm::wallet::vss::VssBackupConfig::new(
            server_url.to_string(),
            store_id.to_string(),
            signing_key,
        );
        self.inner.borrow_mut().configure_vss_backup(&config);
        Ok(())
    }

    /// Disable VSS (cloud) backup.
    #[wasm_bindgen(js_name = "disableVssBackup")]
    pub fn disable_vss_backup(&self) {
        self.inner.borrow_mut().disable_vss_backup();
    }

    /// Upload an encrypted backup to the configured VSS server. Returns the server version.
    #[wasm_bindgen(js_name = "vssBackup")]
    pub async fn vss_backup(&self) -> Result<JsValue, JsValue> {
        let wallet = self.inner.borrow();
        let version = wallet
            .vss_backup()
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(JsValue::from_f64(version as f64))
    }

    /// Download and restore wallet state from VSS server.
    #[wasm_bindgen(js_name = "vssRestoreBackup")]
    pub async fn vss_restore_backup(&self) -> Result<(), JsValue> {
        self.inner
            .borrow_mut()
            .vss_restore_backup()
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Query VSS backup status. Returns { backup_exists, server_version, backup_required }.
    #[wasm_bindgen(js_name = "vssBackupInfo")]
    pub async fn vss_backup_info(&self) -> Result<JsValue, JsValue> {
        let wallet = self.inner.borrow();
        let info = wallet
            .vss_backup_info()
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&info)
    }
}

/// An RGB invoice parsed from a string. Exposes structured invoice data to JavaScript.
#[wasm_bindgen]
pub struct WasmInvoice {
    inner: rgb_lib_wasm::wallet::Invoice,
}

#[wasm_bindgen]
impl WasmInvoice {
    /// Parse an RGB invoice string. Throws if the string is not a valid RGB invoice.
    #[wasm_bindgen(constructor)]
    pub fn new(invoice_string: &str) -> Result<WasmInvoice, JsValue> {
        let invoice = rgb_lib_wasm::wallet::Invoice::new(invoice_string.to_string())
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(WasmInvoice { inner: invoice })
    }

    /// Return the parsed invoice data as a JS object.
    #[wasm_bindgen(js_name = "invoiceData")]
    pub fn invoice_data(&self) -> Result<JsValue, JsValue> {
        to_js(&self.inner.invoice_data())
    }

    /// Return the original invoice string.
    #[wasm_bindgen(js_name = "invoiceString")]
    pub fn invoice_string(&self) -> String {
        self.inner.invoice_string()
    }
}

/// Check whether the provided URL points to a valid RGB proxy server.
#[wasm_bindgen(js_name = "checkProxyUrl")]
pub async fn check_proxy_url(proxy_url: &str) -> Result<(), JsValue> {
    rgb_lib_wasm::wallet::rust_only::check_proxy_url(proxy_url)
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Validate an RGB consignment using witness data bundled in the consignment (offchain).
///
/// Works before the witness transaction is broadcast. Takes the raw consignment
/// bytes (strict-encoded, not base64), the witness transaction ID, and the Bitcoin network.
///
/// Returns a JS object: `{ valid: boolean, warnings?: string[], error?: string, details?: string }`
#[wasm_bindgen(js_name = "validateConsignmentOffchain")]
pub fn validate_consignment_offchain(
    consignment_bytes: &[u8],
    txid: &str,
    network: &str,
) -> Result<JsValue, JsValue> {
    let bitcoin_network = network
        .parse::<rgb_lib_wasm::BitcoinNetwork>()
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let result = rgb_lib_wasm::wallet::rust_only::validate_consignment_offchain(
        consignment_bytes,
        txid,
        bitcoin_network,
    )
    .map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&result)
}
