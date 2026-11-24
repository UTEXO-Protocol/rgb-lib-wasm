// RefCell borrows held across .await are safe on wasm32 (single-threaded runtime).
#![allow(clippy::await_holding_refcell_ref)]

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::panic;
use wasm_bindgen::prelude::*;

use rgb_lib_wasm::Wallet;
use rgb_lib_wasm::wallet::{
    Online, OnlineOptions, Recipient, RefreshFilter, SinglesigKeys, SyncOptions, WalletData,
};

/// Serialize to JsValue using BigInt for u64/i64 values that exceed Number.MAX_SAFE_INTEGER.
fn to_js<T: Serialize>(val: &T) -> Result<JsValue, JsValue> {
    let serializer =
        serde_wasm_bindgen::Serializer::new().serialize_large_number_types_as_bigints(true);
    val.serialize(&serializer)
        .map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Parse an optional encryption preference: "Required", "Unsupported" or "Optional".
fn parse_encryption(
    encryption: Option<String>,
) -> Result<Option<rgb_lib_wasm::Encryption>, JsValue> {
    encryption
        .map(|e| {
            serde_wasm_bindgen::from_value(JsValue::from_str(&e))
                .map_err(|_| JsValue::from_str(&format!("Invalid encryption: {e}")))
        })
        .transpose()
}

#[wasm_bindgen(typescript_custom_section)]
const SIGN_OPTIONS_TS: &'static str = r#"
export interface SignOptions {
    trustWitnessUtxo?: boolean;
    assumeHeight?: number | null;
    allowAllSighashes?: boolean;
    tryFinalize?: boolean;
    signWithTapInternalKey?: boolean;
    allowGrinding?: boolean;
}
"#;

#[wasm_bindgen]
extern "C" {
    /// JS sign options object (see the `SignOptions` TypeScript interface).
    #[wasm_bindgen(typescript_type = "SignOptions")]
    pub type JsSignOptions;
}

#[wasm_bindgen]
extern "C" {
    /// A JS `AbortSignal`.
    #[wasm_bindgen(typescript_type = "AbortSignal")]
    pub type JsAbortSignal;
}

#[wasm_bindgen(inline_js = r#"
export function listen_abort(signal, fire) {
    if (!(signal instanceof AbortSignal)) throw new TypeError("not an AbortSignal");
    if (signal.aborted) {
        fire();
        return () => {};
    }
    signal.addEventListener("abort", fire, { once: true });
    return () => signal.removeEventListener("abort", fire);
}
"#)]
extern "C" {
    #[wasm_bindgen(catch)]
    fn listen_abort(
        signal: &JsAbortSignal,
        fire: &Closure<dyn FnMut()>,
    ) -> Result<js_sys::Function, JsValue>;
}

/// A JS `AbortSignal` linked to a wallet signal. Drop removes the listener.
struct JsAbort {
    signal: rgb_lib_wasm::AbortSignal,
    remove: js_sys::Function,
    _fire: Closure<dyn FnMut()>,
}

impl JsAbort {
    fn new(signal: Option<JsAbortSignal>) -> Result<Option<Self>, JsValue> {
        let Some(js_signal) = signal else {
            return Ok(None);
        };
        let controller = rgb_lib_wasm::AbortController::new();
        let signal = controller.signal();
        let fire = Closure::new(move || controller.abort());
        let remove = listen_abort(&js_signal, &fire)
            .map_err(|_| JsValue::from_str("Invalid AbortSignal"))?;
        Ok(Some(Self {
            signal,
            remove,
            _fire: fire,
        }))
    }

    fn signal(abort: &Option<Self>) -> Option<rgb_lib_wasm::AbortSignal> {
        abort.as_ref().map(|a| a.signal.clone())
    }
}

impl Drop for JsAbort {
    fn drop(&mut self) {
        let _ = self.remove.call0(&JsValue::NULL);
    }
}

#[wasm_bindgen(typescript_custom_section)]
const BACKUP_STORE_TS: &'static str = r#"
/**
 * A versioned key-value store for the wallet backup. Each method may return a promise.
 * - A version is a safe integer of 1 or more.
 * - `read` returns null when the key is absent.
 * - `write` without `expectedVersion` creates the key and fails with a conflict when the key exists.
 * - `write` with `expectedVersion` writes only when the stored version is equal to it. Otherwise it
 *   fails with a conflict, also when the key is absent. It returns the new version: greater than
 *   `expectedVersion`, or 1 or more for a create. The library refuses any other numbering.
 * - `remove` with `version` removes only that version and fails with a conflict otherwise.
 *   Without `version` it removes the key, and an absent key is not an error.
 * - `list` returns the keys of the namespace.
 * To signal a conflict, throw an object with `code` equal to "CONFLICT".
 * The library keeps each wallet backup at key "wallet/<fingerprint>/<network>" of namespace "backup".
 */
export interface BackupStore {
    read(namespace: string, key: string): { value: Uint8Array; version: number } | null | Promise<{ value: Uint8Array; version: number } | null>;
    write(namespace: string, key: string, value: Uint8Array, expectedVersion?: number): number | Promise<number>;
    remove(namespace: string, key: string, version?: number): void | Promise<void>;
    list(namespace: string): string[] | Promise<string[]>;
}
"#;

#[wasm_bindgen]
extern "C" {
    /// JS backup store object (see the `BackupStore` TypeScript interface).
    #[wasm_bindgen(typescript_type = "BackupStore")]
    pub type JsBackupStore;
}

/// Parse a 32-byte secret key in hex.
fn parse_key_hex(
    key_hex: &str,
) -> Result<rgb_lib_wasm::bdk_wallet::bitcoin::secp256k1::SecretKey, JsValue> {
    if key_hex.len() != 64 {
        return Err(JsValue::from_str(&format!(
            "key must be exactly 64 hex chars (32 bytes), got {}",
            key_hex.len()
        )));
    }
    let key_bytes =
        hex::decode(key_hex).map_err(|e| JsValue::from_str(&format!("Invalid key hex: {e}")))?;
    rgb_lib_wasm::bdk_wallet::bitcoin::secp256k1::SecretKey::from_slice(&key_bytes)
        .map_err(|e| JsValue::from_str(&format!("Invalid key: {e}")))
}

/// Mirror of the BDK sign options: a missing field keeps the BDK default.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignOptions {
    trust_witness_utxo: Option<bool>,
    assume_height: Option<u32>,
    allow_all_sighashes: Option<bool>,
    try_finalize: Option<bool>,
    sign_with_tap_internal_key: Option<bool>,
    allow_grinding: Option<bool>,
}

impl From<SignOptions> for rgb_lib_wasm::bdk_wallet::SignOptions {
    fn from(o: SignOptions) -> Self {
        let d = Self::default();
        Self {
            trust_witness_utxo: o.trust_witness_utxo.unwrap_or(d.trust_witness_utxo),
            assume_height: o.assume_height.or(d.assume_height),
            allow_all_sighashes: o.allow_all_sighashes.unwrap_or(d.allow_all_sighashes),
            try_finalize: o.try_finalize.unwrap_or(d.try_finalize),
            sign_with_tap_internal_key: o
                .sign_with_tap_internal_key
                .unwrap_or(d.sign_with_tap_internal_key),
            allow_grinding: o.allow_grinding.unwrap_or(d.allow_grinding),
            ..d
        }
    }
}

/// Parse an optional Online object, `null` and `undefined` meaning none.
fn js_to_online_opt(value: JsValue) -> Result<Option<Online>, JsValue> {
    if value.is_null() || value.is_undefined() {
        return Ok(None);
    }
    serde_wasm_bindgen::from_value(value)
        .map(Some)
        .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))
}

fn js_to_sign_options_opt(
    value: Option<JsSignOptions>,
) -> Result<Option<rgb_lib_wasm::bdk_wallet::SignOptions>, JsValue> {
    value
        .map(|v| {
            serde_wasm_bindgen::from_value::<SignOptions>(v.into())
                .map(Into::into)
                .map_err(|e| JsValue::from_str(&format!("Invalid SignOptions object: {e}")))
        })
        .transpose()
}

/// Read a u64 from a JS number, BigInt or decimal string.
fn js_to_u64(value: JsValue) -> Result<u64, JsValue> {
    match value.as_string() {
        Some(s) => s.parse::<u64>().map_err(|e| e.to_string()),
        None => serde_wasm_bindgen::from_value::<u64>(value).map_err(|e| e.to_string()),
    }
    .map_err(|e| JsValue::from_str(&format!("Invalid expiration timestamp: {e}")))
}

#[wasm_bindgen(js_namespace = console)]
extern "C" {
    #[wasm_bindgen(js_name = error)]
    fn console_error(s: &str);
}

#[wasm_bindgen(start)]
pub fn init() {
    let _ = instant::Instant::now();
    static PANIC_HOOK: std::sync::Once = std::sync::Once::new();
    PANIC_HOOK.call_once(|| {
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
    });
}

/// Maximum seconds in the future an invoice or operation expiration can be.
#[wasm_bindgen(js_name = "maxExpirationSecs")]
pub fn max_expiration_secs() -> u32 {
    rgb_lib_wasm::wallet::MAX_EXPIRATION_SECS
}

#[wasm_bindgen(js_name = "generateKeys")]
pub fn generate_keys(network: &str, witness_version: &str) -> Result<JsValue, JsValue> {
    let bitcoin_network = network
        .parse::<rgb_lib_wasm::BitcoinNetwork>()
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let witness_version = witness_version
        .parse::<rgb_lib_wasm::keys::WitnessVersion>()
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let keys = rgb_lib_wasm::generate_keys(bitcoin_network, witness_version);
    to_js(&keys)
}

#[wasm_bindgen(js_name = "restoreKeys")]
pub fn restore_keys(
    network: &str,
    mnemonic: &str,
    witness_version: &str,
) -> Result<JsValue, JsValue> {
    let bitcoin_network = network
        .parse::<rgb_lib_wasm::BitcoinNetwork>()
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let witness_version = witness_version
        .parse::<rgb_lib_wasm::keys::WitnessVersion>()
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let keys = rgb_lib_wasm::restore_keys(bitcoin_network, mnemonic.to_string(), witness_version)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&keys)
}

#[wasm_bindgen]
pub struct WasmWallet {
    inner: RefCell<Wallet>,
}

#[wasm_bindgen]
impl WasmWallet {
    /// Create a new RGB wallet from a JSON-encoded WalletData and SinglesigKeys.
    /// It does not load IndexedDB and cannot save over a stored snapshot; use `create()`.
    #[wasm_bindgen(constructor)]
    pub fn new(wallet_data_json: &str, keys_json: &str) -> Result<WasmWallet, JsValue> {
        let wd: WalletData = serde_json::from_str(wallet_data_json)
            .map_err(|e| JsValue::from_str(&format!("Invalid WalletData JSON: {e}")))?;
        let keys: SinglesigKeys = serde_json::from_str(keys_json)
            .map_err(|e| JsValue::from_str(&format!("Invalid SinglesigKeys JSON: {e}")))?;
        let wallet = Wallet::new(wd, keys).map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(WasmWallet {
            inner: RefCell::new(wallet),
        })
    }

    /// Create a new RGB wallet with IndexedDB state restoration.
    ///
    /// Like `new()`, but asynchronously checks IndexedDB for a previously saved
    /// snapshot and restores it, so wallet state survives page refreshes.
    #[wasm_bindgen(js_name = "create")]
    pub async fn create(wallet_data_json: &str, keys_json: &str) -> Result<WasmWallet, JsValue> {
        let wd: WalletData = serde_json::from_str(wallet_data_json)
            .map_err(|e| JsValue::from_str(&format!("Invalid WalletData JSON: {e}")))?;
        let keys: SinglesigKeys = serde_json::from_str(keys_json)
            .map_err(|e| JsValue::from_str(&format!("Invalid SinglesigKeys JSON: {e}")))?;
        let wallet = Wallet::restore(wd, keys)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(WasmWallet {
            inner: RefCell::new(wallet),
        })
    }

    /// Load a saved wallet with its saved settings.
    /// A null `mnemonic` loads it watch-only.
    #[wasm_bindgen(js_name = "load")]
    pub async fn load(
        data_dir: &str,
        bitcoin_network: &str,
        master_fingerprint: &str,
        mnemonic: Option<String>,
    ) -> Result<WasmWallet, JsValue> {
        let bitcoin_network = bitcoin_network
            .parse::<rgb_lib_wasm::BitcoinNetwork>()
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let wallet = Wallet::load(data_dir, bitcoin_network, master_fingerprint, mnemonic)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(WasmWallet {
            inner: RefCell::new(wallet),
        })
    }

    /// Durably persist current wallet state to IndexedDB.
    #[wasm_bindgen(js_name = "flush")]
    pub async fn flush(&self) -> Result<(), JsValue> {
        self.inner
            .borrow()
            .flush()
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Return the error of the last failed IndexedDB save, until a save commits.
    #[wasm_bindgen(js_name = "lastSaveError")]
    pub fn last_save_error(&self) -> Option<String> {
        self.inner.borrow().last_save_error()
    }

    /// Return the WalletData as a JS object.
    #[wasm_bindgen(js_name = "getWalletData")]
    pub fn get_wallet_data(&self) -> Result<JsValue, JsValue> {
        to_js(&self.inner.borrow().get_wallet_data())
    }

    /// Return the SinglesigKeys as a JS object.
    /// The mnemonic is not returned: `mnemonic` is null.
    #[wasm_bindgen(js_name = "getKeys")]
    pub fn get_keys(&self) -> Result<JsValue, JsValue> {
        let mut keys = self.inner.borrow().get_keys();
        keys.mnemonic = None;
        let data = to_js(&keys)?;
        js_sys::Reflect::set(&data, &JsValue::from_str("mnemonic"), &JsValue::NULL)?;
        Ok(data)
    }

    /// Return the colored and vanilla descriptors as a JS object.
    /// The descriptors of a signing wallet hold its account private keys.
    #[wasm_bindgen(js_name = "getDescriptors")]
    pub fn get_descriptors(&self) -> Result<JsValue, JsValue> {
        to_js(&self.inner.borrow().get_descriptors())
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
    /// `link_right` issues the link right, `linked_from_contract` is the contract linked from,
    /// `skip_contract_checks` skips the checks on that contract.
    #[wasm_bindgen(js_name = "issueAssetIfa")]
    #[allow(clippy::too_many_arguments)]
    pub fn issue_asset_ifa(
        &self,
        ticker: &str,
        name: &str,
        precision: u8,
        amounts_js: JsValue,
        inflation_amounts_js: JsValue,
        reject_list_url: Option<String>,
        link_right: bool,
        linked_from_contract: Option<String>,
        skip_contract_checks: bool,
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
                link_right,
                linked_from_contract,
                skip_contract_checks,
            )
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&asset)
    }

    /// Issue a new BFA (Bridged Fungible Asset) with `mint_rights` mint rights and no units.
    ///
    /// `contract_address` is the bridge contract on the EVM chain `chain_id`.
    /// `reject_list_address` is an optional reject list contract on the same chain.
    /// `link_right` issues the link right, `linked_from_contract` is the contract linked from,
    /// `skip_contract_checks` skips the checks on that contract.
    #[wasm_bindgen(js_name = "issueAssetBfa")]
    #[allow(clippy::too_many_arguments)]
    pub fn issue_asset_bfa(
        &self,
        ticker: &str,
        name: &str,
        precision: u8,
        mint_rights: u8,
        chain_id: u64,
        contract_address: &str,
        reject_list_address: Option<String>,
        link_right: bool,
        linked_from_contract: Option<String>,
        skip_contract_checks: bool,
    ) -> Result<JsValue, JsValue> {
        let asset = self
            .inner
            .borrow()
            .issue_asset_bfa(
                ticker.to_string(),
                name.to_string(),
                precision,
                mint_rights,
                chain_id,
                contract_address.to_string(),
                reject_list_address,
                link_right,
                linked_from_contract,
                skip_contract_checks,
            )
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&asset)
    }

    /// Return the BTC balance. Syncs first unless `skipSync` is true.
    #[wasm_bindgen(js_name = "getBtcBalance")]
    pub async fn get_btc_balance(
        &self,
        online_js: JsValue,
        skip_sync: bool,
    ) -> Result<JsValue, JsValue> {
        let online = js_to_online_opt(online_js)?;
        let balance = self
            .inner
            .borrow_mut()
            .get_btc_balance(online, skip_sync)
            .await
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

    /// List the media of an RGB asset.
    #[wasm_bindgen(js_name = "listAssetMedia")]
    pub fn list_asset_media(&self, asset_id: &str) -> Result<JsValue, JsValue> {
        let medias = self
            .inner
            .borrow()
            .list_asset_media(asset_id.to_string())
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&medias)
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

    /// List RGB transfers matching an asset filter ("AnyOrNone", "None" or {"Id": "<asset_id>"})
    /// and a scope ("All", {"Txid": "<txid>"} or {"BatchTransfer": <idx>}).
    #[wasm_bindgen(js_name = "listTransfers")]
    pub fn list_transfers(
        &self,
        asset_filter_js: JsValue,
        scope_js: JsValue,
    ) -> Result<JsValue, JsValue> {
        let asset_filter: rgb_lib_wasm::wallet::AssetFilter =
            serde_wasm_bindgen::from_value(asset_filter_js)
                .map_err(|e| JsValue::from_str(&format!("Invalid asset filter: {e}")))?;
        let scope: rgb_lib_wasm::wallet::TransferScope =
            serde_wasm_bindgen::from_value(scope_js)
                .map_err(|e| JsValue::from_str(&format!("Invalid scope: {e}")))?;
        let transfers = self
            .inner
            .borrow()
            .list_transfers(asset_filter, scope)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&transfers)
    }

    /// List unspent outputs. Syncs first unless `skipSync` is true.
    #[wasm_bindgen(js_name = "listUnspents")]
    pub async fn list_unspents(
        &self,
        online_js: JsValue,
        settled_only: bool,
        skip_sync: bool,
    ) -> Result<JsValue, JsValue> {
        let online = js_to_online_opt(online_js)?;
        let unspents = self
            .inner
            .borrow_mut()
            .list_unspents(online, settled_only, skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&unspents)
    }

    /// List Bitcoin transactions. Syncs first unless `skipSync` is true.
    #[wasm_bindgen(js_name = "listTransactions")]
    pub async fn list_transactions(
        &self,
        online_js: JsValue,
        skip_sync: bool,
    ) -> Result<JsValue, JsValue> {
        let online = js_to_online_opt(online_js)?;
        let txs = self
            .inner
            .borrow_mut()
            .list_transactions(online, skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&txs)
    }

    /// List begun vanilla operations that are not ended or aborted.
    #[wasm_bindgen(js_name = "listPendingVanillaTxs")]
    pub fn list_pending_vanilla_txs(&self) -> Result<JsValue, JsValue> {
        let txs = self
            .inner
            .borrow()
            .list_pending_vanilla_txs()
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&txs)
    }

    /// Abort a begun vanilla operation: delete it and release its inputs.
    #[wasm_bindgen(js_name = "abortPendingVanillaTx")]
    pub fn abort_pending_vanilla_tx(&self, txid: &str) -> Result<(), JsValue> {
        self.inner
            .borrow()
            .abort_pending_vanilla_tx(txid.to_string())
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Sign a PSBT (base64-encoded) with optional sign options. Returns the signed PSBT string.
    #[wasm_bindgen(js_name = "signPsbt")]
    pub fn sign_psbt(
        &self,
        unsigned_psbt: &str,
        sign_options: Option<JsSignOptions>,
    ) -> Result<String, JsValue> {
        let sign_options = js_to_sign_options_opt(sign_options)?;
        self.inner
            .borrow()
            .sign_psbt(unsigned_psbt.to_string(), sign_options)
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

    /// Inspect a PSBT (base64-encoded) without signing or changing it. Returns a PsbtInspection object.
    #[wasm_bindgen(js_name = "inspectPsbt")]
    pub fn inspect_psbt(&self, psbt: &str) -> Result<JsValue, JsValue> {
        to_js(
            &self
                .inner
                .borrow()
                .inspect_psbt(psbt.to_string())
                .map_err(|e| JsValue::from_str(&e.to_string()))?,
        )
    }

    /// Inspect a PSBT and the fascia (`details.fascia` of a `*Begin` result) this wallet built. Returns an RgbInspection object.
    #[wasm_bindgen(js_name = "inspectFascia")]
    pub fn inspect_fascia(
        &self,
        psbt: &str,
        fascia: &[u8],
        entropy: Option<u64>,
    ) -> Result<JsValue, JsValue> {
        to_js(
            &self
                .inner
                .borrow()
                .inspect_fascia(psbt.to_string(), fascia, entropy)
                .map_err(|e| JsValue::from_str(&e.to_string()))?,
        )
    }

    /// Blind an UTXO to receive RGB assets. Returns ReceiveData as a JS object.
    ///
    /// `online_js` is the Online object returned by `goOnline` (the wallet reserves the invoice on
    /// its proxies).
    /// `assignment_js` is a JS object like `{ "Fungible": 100 }` or `"NonFungible"`, `"MintRight"`
    /// or `"Any"`.
    /// `expiration_timestamp_js` is a UTC timestamp in seconds, as a number, BigInt or decimal string.
    /// `transport_endpoints_js` is a JS array of endpoint strings.
    /// `encryption` is "Required" (default), "Unsupported" or "Optional".
    #[wasm_bindgen(js_name = "blindReceive")]
    #[allow(clippy::too_many_arguments)]
    pub async fn blind_receive(
        &self,
        online_js: JsValue,
        asset_id: Option<String>,
        assignment_js: JsValue,
        expiration_timestamp_js: JsValue,
        transport_endpoints_js: JsValue,
        min_confirmations: u8,
        encryption: Option<String>,
    ) -> Result<JsValue, JsValue> {
        let online = js_to_online_opt(online_js)?;
        let expiration_timestamp = js_to_u64(expiration_timestamp_js)?;
        let assignment: rgb_lib_wasm::Assignment = serde_wasm_bindgen::from_value(assignment_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid assignment: {e}")))?;
        let transport_endpoints: Vec<String> =
            serde_wasm_bindgen::from_value(transport_endpoints_js)
                .map_err(|e| JsValue::from_str(&format!("Invalid transport endpoints: {e}")))?;
        let encryption = parse_encryption(encryption)?;
        let mut wallet = self.inner.borrow_mut();
        let data = wallet
            .blind_receive(
                online,
                asset_id,
                assignment,
                expiration_timestamp,
                transport_endpoints,
                min_confirmations,
                encryption,
            )
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&data)
    }

    /// Create an address to receive RGB assets via witness TX. Returns ReceiveData as a JS object.
    ///
    /// `online_js` is the Online object returned by `goOnline`.
    /// `assignment_js` is a JS object like `{ "Fungible": 100 }`, `"MintRight"` or `"Any"`.
    /// `expiration_timestamp_js` is a UTC timestamp in seconds, as a number, BigInt or decimal string.
    /// `encryption` is "Required" (default), "Unsupported" or "Optional".
    #[wasm_bindgen(js_name = "witnessReceive")]
    #[allow(clippy::too_many_arguments)]
    pub async fn witness_receive(
        &self,
        online_js: JsValue,
        asset_id: Option<String>,
        assignment_js: JsValue,
        expiration_timestamp_js: JsValue,
        transport_endpoints_js: JsValue,
        min_confirmations: u8,
        encryption: Option<String>,
    ) -> Result<JsValue, JsValue> {
        let online = js_to_online_opt(online_js)?;
        let expiration_timestamp = js_to_u64(expiration_timestamp_js)?;
        let assignment: rgb_lib_wasm::Assignment = serde_wasm_bindgen::from_value(assignment_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid assignment: {e}")))?;
        let transport_endpoints: Vec<String> =
            serde_wasm_bindgen::from_value(transport_endpoints_js)
                .map_err(|e| JsValue::from_str(&format!("Invalid transport endpoints: {e}")))?;
        let encryption = parse_encryption(encryption)?;
        let mut wallet = self.inner.borrow_mut();
        let data = wallet
            .witness_receive(
                online,
                asset_id,
                assignment,
                expiration_timestamp,
                transport_endpoints,
                min_confirmations,
                encryption,
            )
            .await
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

    /// Rotate the pinned address for the given keychain.
    /// keychain: 0 = External (colored), 1 = Internal (vanilla)
    #[wasm_bindgen(js_name = "rotateAddress")]
    pub fn rotate_address(&self, keychain: u32) -> Result<String, JsValue> {
        let kc = match keychain {
            0 => rgb_lib_wasm::bdk_wallet::KeychainKind::External,
            1 => rgb_lib_wasm::bdk_wallet::KeychainKind::Internal,
            _ => {
                return Err(JsValue::from_str(
                    "Invalid keychain: use 0 (External) or 1 (Internal)",
                ));
            }
        };
        self.inner
            .borrow_mut()
            .rotate_address(kc)
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

    /// Go online: connect to an indexer. Takes the online options as a JS object and returns
    /// Online data as a JS object.
    #[wasm_bindgen(js_name = "goOnline")]
    pub async fn go_online(
        &self,
        online_options: JsValue,
        signal: Option<JsAbortSignal>,
    ) -> Result<JsValue, JsValue> {
        let online_options: OnlineOptions = serde_wasm_bindgen::from_value(online_options)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        let abort = JsAbort::new(signal)?;
        let mut wallet = self.inner.borrow_mut();
        let online = wallet
            .with_abort(JsAbort::signal(&abort), async move |w| {
                w.go_online(online_options).await
            })
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&online)
    }

    /// Sync one keychain (`{keychain: "Colored" | {Vanilla: {lookback}}, strategy: "FullScan" |
    /// "FullSync" | "FastSync"}`). Call once per keychain to sync both.
    pub async fn sync(
        &self,
        online_js: JsValue,
        options_js: JsValue,
        signal: Option<JsAbortSignal>,
    ) -> Result<(), JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let options: SyncOptions = serde_wasm_bindgen::from_value(options_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid sync options: {e}")))?;
        let abort = JsAbort::new(signal)?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .with_abort(JsAbort::signal(&abort), async move |w| {
                w.sync(online, options).await
            })
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Create UTXOs: begin, sign with the wallet keys and end. Returns the number of created UTXOs.
    /// If `exact` is true, create exactly the requested number of UTXOs, or fail with the
    /// insufficient-funds error and reserve nothing.
    #[wasm_bindgen(js_name = "createUtxos")]
    #[allow(clippy::too_many_arguments)]
    pub async fn create_utxos(
        &self,
        online_js: JsValue,
        up_to: bool,
        exact: bool,
        num: Option<u8>,
        size: Option<u32>,
        fee_rate: u64,
        skip_sync: bool,
    ) -> Result<u8, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .create_utxos(online, up_to, exact, num, size, fee_rate, skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Create UTXOs (begin): prepare a PSBT to create new UTXOs for RGB allocations.
    /// If `dry_run` is true, the wallet reserves no inputs and records no pending operation.
    /// If `exact` is true, create exactly the requested number of UTXOs, or fail with the
    /// insufficient-funds error and reserve nothing.
    /// Returns the unsigned PSBT string.
    #[wasm_bindgen(js_name = "createUtxosBegin")]
    #[allow(clippy::too_many_arguments)]
    pub async fn create_utxos_begin(
        &self,
        online_js: JsValue,
        up_to: bool,
        exact: bool,
        num: Option<u8>,
        size: Option<u32>,
        fee_rate: u64,
        skip_sync: bool,
        dry_run: bool,
        signal: Option<JsAbortSignal>,
    ) -> Result<String, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let abort = JsAbort::new(signal)?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .with_abort(JsAbort::signal(&abort), async move |w| {
                w.create_utxos_begin(
                    online, up_to, exact, num, size, fee_rate, skip_sync, dry_run,
                )
                .await
            })
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
    ) -> Result<u8, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .create_utxos_end(online, signed_psbt.to_string())
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Send RGB assets: begin, sign with the wallet keys and end. Returns an OperationResult JS
    /// object.
    ///
    /// The arguments are read as in `sendBegin`.
    #[wasm_bindgen(js_name = "send")]
    #[allow(clippy::too_many_arguments)]
    pub async fn send(
        &self,
        online_js: JsValue,
        recipient_map_js: JsValue,
        donation: bool,
        fee_rate: u64,
        min_confirmations: u8,
        expiration_timestamp_js: JsValue,
        sender_transport_endpoint: Option<String>,
    ) -> Result<JsValue, JsValue> {
        let expiration_timestamp = js_to_u64(expiration_timestamp_js)?;
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let recipient_map: HashMap<String, Vec<Recipient>> =
            serde_wasm_bindgen::from_value(recipient_map_js)
                .map_err(|e| JsValue::from_str(&format!("Invalid recipient map: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .send(
                online,
                recipient_map,
                donation,
                fee_rate,
                min_confirmations,
                expiration_timestamp,
                sender_transport_endpoint,
            )
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Send RGB assets (begin): prepare a PSBT. Returns a SendBeginResult JS object
    /// `{ psbt, batchTransferIdx, details }`.
    ///
    /// `recipient_map_js` is a JS object mapping asset IDs to arrays of Recipient objects.
    /// `expiration_timestamp_js` is a UTC timestamp in seconds, as a number, BigInt or decimal string.
    /// `sender_transport_endpoint` is the sender's own proxy (e.g. `rpc://127.0.0.1:3000/json-rpc`),
    /// where the recipients post their ACK.
    /// If `dry_run` is true, the wallet does not save the transfer as Initiated.
    #[wasm_bindgen(js_name = "sendBegin")]
    #[allow(clippy::too_many_arguments)]
    pub async fn send_begin(
        &self,
        online_js: JsValue,
        recipient_map_js: JsValue,
        donation: bool,
        fee_rate: u64,
        min_confirmations: u8,
        expiration_timestamp_js: JsValue,
        sender_transport_endpoint: Option<String>,
        lock_time: Option<u32>,
        dry_run: bool,
        signal: Option<JsAbortSignal>,
    ) -> Result<JsValue, JsValue> {
        let expiration_timestamp = js_to_u64(expiration_timestamp_js)?;
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let recipient_map: HashMap<String, Vec<Recipient>> =
            serde_wasm_bindgen::from_value(recipient_map_js)
                .map_err(|e| JsValue::from_str(&format!("Invalid recipient map: {e}")))?;
        let abort = JsAbort::new(signal)?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .with_abort(JsAbort::signal(&abort), async move |w| {
                w.send_begin(
                    online,
                    recipient_map,
                    donation,
                    fee_rate,
                    min_confirmations,
                    expiration_timestamp,
                    sender_transport_endpoint,
                    lock_time,
                    dry_run,
                )
                .await
            })
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Send RGB assets (end): broadcast a signed PSBT. Returns an OperationResult JS object.
    #[wasm_bindgen(js_name = "sendEnd")]
    pub async fn send_end(
        &self,
        online_js: JsValue,
        signed_psbt: &str,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .send_end(online, signed_psbt.to_string())
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Send BTC: begin, sign with the wallet keys and end. Returns the txid string.
    #[wasm_bindgen(js_name = "sendBtc")]
    pub async fn send_btc(
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
            .send_btc(online, address.to_string(), amount, fee_rate, skip_sync)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Send BTC (begin): prepare a PSBT. Returns the unsigned PSBT string.
    /// If `dry_run` is true, the wallet reserves no inputs and records no pending operation.
    #[wasm_bindgen(js_name = "sendBtcBegin")]
    #[allow(clippy::too_many_arguments)]
    pub async fn send_btc_begin(
        &self,
        online_js: JsValue,
        address: &str,
        amount: u64,
        fee_rate: u64,
        skip_sync: bool,
        dry_run: bool,
        signal: Option<JsAbortSignal>,
    ) -> Result<String, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let abort = JsAbort::new(signal)?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .with_abort(JsAbort::signal(&abort), async move |w| {
                w.send_btc_begin(
                    online,
                    address.to_string(),
                    amount,
                    fee_rate,
                    skip_sync,
                    dry_run,
                )
                .await
            })
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Send BTC (end): broadcast a signed PSBT. Returns the txid string.
    #[wasm_bindgen(js_name = "sendBtcEnd")]
    pub async fn send_btc_end(
        &self,
        online_js: JsValue,
        signed_psbt: &str,
    ) -> Result<String, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .send_btc_end(online, signed_psbt.to_string())
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
        signal: Option<JsAbortSignal>,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let filter: Vec<RefreshFilter> = serde_wasm_bindgen::from_value(filter_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid filter: {e}")))?;
        let abort = JsAbort::new(signal)?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .with_abort(JsAbort::signal(&abort), async move |w| {
                w.refresh(online, asset_id, filter, skip_sync).await
            })
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Import the asset of a transfer consignment (raw bytes) with its witness TXID, with zero
    /// balance, so the wallet can make invoices for it.
    #[wasm_bindgen(js_name = "saveNewAsset")]
    pub async fn save_new_asset(
        &self,
        online_js: JsValue,
        consignment_bytes: &[u8],
        txid: &str,
    ) -> Result<(), JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let wallet = self.inner.borrow();
        wallet
            .save_new_asset(online, consignment_bytes, txid)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
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

    /// Drain all wallet funds: begin, sign with the wallet keys and end. Returns the txid string.
    /// It spends the colored UTXOs too and destroys their assets.
    #[wasm_bindgen(js_name = "drainTo")]
    pub async fn drain_to(
        &self,
        online_js: JsValue,
        address: &str,
        fee_rate: u64,
    ) -> Result<String, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .drain_to(online, address.to_string(), fee_rate)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Drain all wallet funds (begin): prepare a PSBT. Returns the unsigned PSBT string.
    /// It spends the colored UTXOs too and destroys their assets.
    /// If `dry_run` is true, the wallet reserves no inputs and records no pending operation.
    #[wasm_bindgen(js_name = "drainToBegin")]
    pub async fn drain_to_begin(
        &self,
        online_js: JsValue,
        address: &str,
        fee_rate: u64,
        dry_run: bool,
        signal: Option<JsAbortSignal>,
    ) -> Result<String, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let abort = JsAbort::new(signal)?;
        let mut wallet = self.inner.borrow_mut();
        wallet
            .with_abort(JsAbort::signal(&abort), async move |w| {
                w.drain_to_begin(online, address.to_string(), fee_rate, dry_run)
                    .await
            })
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

    /// Inflate an IFA asset: begin, sign with the wallet keys and end. Returns an OperationResult
    /// JS object.
    ///
    /// `inflation_amounts_js` is a JS array of u64 values.
    #[wasm_bindgen(js_name = "inflate")]
    pub async fn inflate(
        &self,
        online_js: JsValue,
        asset_id: &str,
        inflation_amounts_js: JsValue,
        fee_rate: u64,
        min_confirmations: u8,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let inflation_amounts: Vec<u64> = serde_wasm_bindgen::from_value(inflation_amounts_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid inflation_amounts array: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .inflate(
                online,
                asset_id.to_string(),
                inflation_amounts,
                fee_rate,
                min_confirmations,
            )
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Inflate an IFA asset (begin): prepare a PSBT. Returns an InflateBeginResult JS object
    /// `{ psbt, batchTransferIdx, details }`. If `dry_run` is true, the wallet does not save the
    /// transfer as Initiated.
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
        dry_run: bool,
        signal: Option<JsAbortSignal>,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let inflation_amounts: Vec<u64> = serde_wasm_bindgen::from_value(inflation_amounts_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid inflation_amounts array: {e}")))?;
        let abort = JsAbort::new(signal)?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .with_abort(JsAbort::signal(&abort), async move |w| {
                w.inflate_begin(
                    online,
                    asset_id.to_string(),
                    inflation_amounts,
                    fee_rate,
                    min_confirmations,
                    dry_run,
                )
                .await
            })
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
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

    /// Burn RGB assets: begin, sign with the wallet keys and end. Returns an OperationResult JS
    /// object.
    ///
    /// `burn_reason` is an optional hex-encoded 32-byte value.
    #[wasm_bindgen(js_name = "burn")]
    pub async fn burn(
        &self,
        online_js: JsValue,
        asset_id: &str,
        amount: u64,
        burn_reason: Option<String>,
        fee_rate: u64,
        min_confirmations: u8,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .burn(
                online,
                asset_id.to_string(),
                amount,
                burn_reason,
                fee_rate,
                min_confirmations,
            )
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Burn RGB assets (begin): prepare a PSBT. Returns a BurnBeginResult JS object.
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = "burnBegin")]
    pub async fn burn_begin(
        &self,
        online_js: JsValue,
        asset_id: &str,
        amount: u64,
        burn_reason: Option<String>,
        fee_rate: u64,
        min_confirmations: u8,
        dry_run: bool,
        signal: Option<JsAbortSignal>,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let abort = JsAbort::new(signal)?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .with_abort(JsAbort::signal(&abort), async move |w| {
                w.burn_begin(
                    online,
                    asset_id.to_string(),
                    amount,
                    burn_reason,
                    fee_rate,
                    min_confirmations,
                    dry_run,
                )
                .await
            })
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Burn RGB assets (end): broadcast a signed PSBT. Returns an OperationResult JS object.
    #[wasm_bindgen(js_name = "burnEnd")]
    pub async fn burn_end(
        &self,
        online_js: JsValue,
        signed_psbt: &str,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .burn_end(online, signed_psbt.to_string())
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Return the consignment of an ended burn as a Uint8Array.
    #[wasm_bindgen(js_name = "getBurnConsignment")]
    pub fn get_burn_consignment(&self, asset_id: &str, txid: &str) -> Result<Vec<u8>, JsValue> {
        self.inner
            .borrow()
            .get_burn_consignment(asset_id.to_string(), txid.to_string())
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Link an IFA or BFA asset to another contract: begin, sign with the wallet keys and end.
    /// Returns an OperationResult JS object.
    #[wasm_bindgen(js_name = "link")]
    pub async fn link(
        &self,
        online_js: JsValue,
        asset_id: &str,
        linked_to_contract: &str,
        fee_rate: u64,
        min_confirmations: u8,
        skip_contract_checks: bool,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .link(
                online,
                asset_id.to_string(),
                linked_to_contract.to_string(),
                fee_rate,
                min_confirmations,
                skip_contract_checks,
            )
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Link an IFA or BFA asset to another contract (begin): prepare a PSBT. Returns a
    /// LinkBeginResult JS object.
    #[wasm_bindgen(js_name = "linkBegin")]
    #[allow(clippy::too_many_arguments)]
    pub async fn link_begin(
        &self,
        online_js: JsValue,
        asset_id: &str,
        linked_to_contract: &str,
        fee_rate: u64,
        min_confirmations: u8,
        skip_contract_checks: bool,
        dry_run: bool,
        signal: Option<JsAbortSignal>,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let abort = JsAbort::new(signal)?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .with_abort(JsAbort::signal(&abort), async move |w| {
                w.link_begin(
                    online,
                    asset_id.to_string(),
                    linked_to_contract.to_string(),
                    fee_rate,
                    min_confirmations,
                    skip_contract_checks,
                    dry_run,
                )
                .await
            })
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Link an IFA or BFA asset to another contract (end): broadcast a signed PSBT. Returns an
    /// OperationResult JS object.
    #[wasm_bindgen(js_name = "linkEnd")]
    pub async fn link_end(
        &self,
        online_js: JsValue,
        signed_psbt: &str,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .link_end(online, signed_psbt.to_string())
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Begin a BFA mint. Returns a MintBeginResult object.
    #[wasm_bindgen(js_name = "mintBegin")]
    #[allow(clippy::too_many_arguments)]
    pub async fn mint_begin(
        &self,
        online_js: JsValue,
        asset_id: &str,
        recipient_js: JsValue,
        before_block: u64,
        fee_rate: u64,
        min_confirmations: u8,
        sender_transport_endpoint: &str,
        signal: Option<JsAbortSignal>,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let recipient: Recipient = serde_wasm_bindgen::from_value(recipient_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Recipient object: {e}")))?;
        let abort = JsAbort::new(signal)?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .with_abort(JsAbort::signal(&abort), async move |w| {
                w.mint_begin(
                    online,
                    asset_id.to_string(),
                    recipient,
                    before_block,
                    fee_rate,
                    min_confirmations,
                    sender_transport_endpoint.to_string(),
                )
                .await
            })
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Complete a BFA mint. Returns an OperationResult object.
    #[wasm_bindgen(js_name = "mintEnd")]
    pub async fn mint_end(
        &self,
        online_js: JsValue,
        signed_psbt: &str,
    ) -> Result<JsValue, JsValue> {
        let online: Online = serde_wasm_bindgen::from_value(online_js)
            .map_err(|e| JsValue::from_str(&format!("Invalid Online object: {e}")))?;
        let mut wallet = self.inner.borrow_mut();
        let result = wallet
            .mint_end(online, signed_psbt.to_string())
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        to_js(&result)
    }

    /// Return the checked mint received on a recipient ID as a ReceivedMint object, or null.
    #[wasm_bindgen(js_name = "getReceivedMint")]
    pub fn get_received_mint(&self, recipient_id: &str) -> Result<JsValue, JsValue> {
        let received_mint = self
            .inner
            .borrow()
            .get_received_mint(recipient_id.to_string())
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        received_mint.map_or(Ok(JsValue::NULL), |m| to_js(&m))
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
        let signing_key = parse_key_hex(signing_key_hex)?;
        let config = rgb_lib_wasm::wallet::vss::VssBackupConfig::new(
            server_url.to_string(),
            store_id.to_string(),
            signing_key,
        );
        self.inner.borrow_mut().configure_vss_backup(&config);
        Ok(())
    }

    /// Configure an app backup store (see the `BackupStore` TypeScript interface).
    ///
    /// `storeId` names the store on this device. Use another `storeId` for another store.
    /// `encryptionKeyHex` is the 32-byte encryption key as a hex string (64 hex chars).
    /// The store receives only encrypted data.
    /// The library checks versioned writes on the first call and refuses a store that fails.
    /// A store written to lie on purpose can pass this check.
    ///
    /// **Security note:** The key crosses the JS/WASM boundary as a string, as in
    /// `configureVssBackup`.
    #[wasm_bindgen(js_name = "configureBackupStore")]
    pub fn configure_backup_store(
        &self,
        store: JsBackupStore,
        store_id: &str,
        encryption_key_hex: &str,
    ) -> Result<(), JsValue> {
        let key = parse_key_hex(encryption_key_hex)?;
        let store = rgb_lib_wasm::wallet::backup_store::JsKvStore::new(store.into())
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        self.inner
            .borrow_mut()
            .configure_backup_store(Box::new(store), store_id, key);
        Ok(())
    }

    /// Disable the configured backup store (VSS or the app's).
    #[wasm_bindgen(js_name = "disableVssBackup")]
    pub fn disable_vss_backup(&self) {
        self.inner.borrow_mut().disable_vss_backup();
    }

    /// Upload an encrypted backup to the configured backup store (VSS or the app's).
    /// Returns the store version.
    #[wasm_bindgen(js_name = "vssBackup")]
    pub async fn vss_backup(&self) -> Result<JsValue, JsValue> {
        let wallet = self.inner.borrow();
        let version = wallet
            .vss_backup()
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(JsValue::from_f64(version as f64))
    }

    /// Upload a backup when the wallet has changes not yet backed up.
    /// Returns the server version, or null when no upload is needed.
    #[wasm_bindgen(js_name = "ensureInitialBackup")]
    pub async fn ensure_initial_backup(&self) -> Result<JsValue, JsValue> {
        let wallet = self.inner.borrow();
        let version = wallet
            .ensure_initial_backup()
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(version.map_or(JsValue::NULL, |v| JsValue::from_f64(v as f64)))
    }

    /// Download and restore wallet state from the configured backup store (VSS or the app's).
    #[wasm_bindgen(js_name = "vssRestoreBackup")]
    pub async fn vss_restore_backup(&self) -> Result<(), JsValue> {
        self.inner
            .borrow_mut()
            .vss_restore_backup()
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Query the status of the configured backup store (VSS or the app's).
    /// Returns { backup_exists, server_version, backup_required }.
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
        // to_js keeps JS maps because refresh has integer keys. These keys are strings.
        let serializer = serde_wasm_bindgen::Serializer::new()
            .serialize_large_number_types_as_bigints(true)
            .serialize_maps_as_objects(true);
        self.inner
            .invoice_data()
            .serialize(&serializer)
            .map_err(|e| JsValue::from_str(&e.to_string()))
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

/// Check that the URL points to a valid indexer for the network. Return its protocol.
#[wasm_bindgen(js_name = "checkIndexerUrl")]
pub async fn check_indexer_url(indexer_url: &str, network: &str) -> Result<JsValue, JsValue> {
    let bitcoin_network = network
        .parse::<rgb_lib_wasm::BitcoinNetwork>()
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let protocol = rgb_lib_wasm::wallet::check_indexer_url(indexer_url, bitcoin_network)
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&protocol)
}

/// Fill in the SPV proofs a transfer consignment misses, from an esplora indexer.
/// Returns { withoutProof: [{ txid, details }], rewritten, consignment: Uint8Array | null }.
#[wasm_bindgen(js_name = "fillSpvProofs")]
pub async fn fill_spv_proofs(
    consignment_bytes: &[u8],
    indexer_url: &str,
) -> Result<JsValue, JsValue> {
    let (fill, bytes) = rgb_lib_wasm::no_wallet::fill_spv_proofs(consignment_bytes, indexer_url)
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let obj = to_js(&fill)?;
    let consignment = bytes.map_or(JsValue::NULL, |b| js_sys::Uint8Array::from(&b[..]).into());
    js_sys::Reflect::set(&obj, &"consignment".into(), &consignment)?;
    Ok(obj)
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

/// Inspect an RGB consignment (raw bytes) without a wallet.
///
/// The bundle filter is `"All"`, `"Terminals"` or `{ Witness: "<txid>" }`.
#[wasm_bindgen(js_name = "inspectConsignment")]
pub fn inspect_consignment(
    consignment_bytes: &[u8],
    bundle_filter: JsValue,
) -> Result<JsValue, JsValue> {
    let bundle_filter: rgb_lib_wasm::no_wallet::BundleFilter =
        serde_wasm_bindgen::from_value(bundle_filter)
            .map_err(|e| JsValue::from_str(&format!("Invalid bundle filter: {e}")))?;
    let inspection = rgb_lib_wasm::no_wallet::inspect_consignment(consignment_bytes, bundle_filter)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&inspection)
}

/// Inspect a PSBT and the fascia of its RGB operation without a wallet.
#[wasm_bindgen(js_name = "inspectFascia")]
pub fn inspect_fascia(
    psbt: &str,
    fascia: &[u8],
    entropy: Option<u64>,
    network: &str,
) -> Result<JsValue, JsValue> {
    let bitcoin_network = network
        .parse::<rgb_lib_wasm::BitcoinNetwork>()
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let inspection =
        rgb_lib_wasm::no_wallet::inspect_fascia(psbt.to_string(), fascia, entropy, bitcoin_network)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
    to_js(&inspection)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(json: &str) -> rgb_lib_wasm::bdk_wallet::SignOptions {
        serde_json::from_str::<SignOptions>(json).unwrap().into()
    }

    #[test]
    fn sign_options_conversion() {
        let d = rgb_lib_wasm::bdk_wallet::SignOptions::default();
        let o = convert("{}");
        assert_eq!(o.trust_witness_utxo, d.trust_witness_utxo);
        assert_eq!(o.assume_height, d.assume_height);
        assert_eq!(o.allow_all_sighashes, d.allow_all_sighashes);
        assert_eq!(o.try_finalize, d.try_finalize);
        assert_eq!(o.sign_with_tap_internal_key, d.sign_with_tap_internal_key);
        assert_eq!(o.allow_grinding, d.allow_grinding);

        let o = convert(
            r#"{"trustWitnessUtxo":true,"assumeHeight":7,"allowAllSighashes":true,
            "tryFinalize":false,"signWithTapInternalKey":false,"allowGrinding":false}"#,
        );
        assert!(o.trust_witness_utxo);
        assert_eq!(o.assume_height, Some(7));
        assert!(o.allow_all_sighashes);
        assert!(!o.try_finalize);
        assert!(!o.sign_with_tap_internal_key);
        assert!(!o.allow_grinding);
    }
}
