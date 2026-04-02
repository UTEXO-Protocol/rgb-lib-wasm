/**
 * shared.js — Shared ES module for rgb-lib-wasm example pages.
 *
 * Provides WASM binding re-exports, global state, constants, DOM helpers,
 * regtest utilities, and shared CSS so each example page stays minimal.
 */

// ---------------------------------------------------------------------------
// 1. WASM binding re-exports
// ---------------------------------------------------------------------------

import init, {
  generate_keys,
  restore_keys,
  check_proxy_url,
  validateConsignmentOffchain,
  WasmWallet,
  WasmInvoice,
} from '/pkg/rgb_lib_wasm_bindings.js';

export {
  generate_keys, restore_keys, check_proxy_url,
  validateConsignmentOffchain,
  WasmWallet, WasmInvoice,
};

// ---------------------------------------------------------------------------
// 2. Mutable application state (shared across a single page)
// ---------------------------------------------------------------------------

export const state = { wallet: null, online: null };

// ---------------------------------------------------------------------------
// 3. Environment constants (regtest defaults)
// ---------------------------------------------------------------------------

export const REGTEST_HELPER = 'http://127.0.0.1:8080';
export const ESPLORA_API   = 'http://127.0.0.1:8094/regtest/api';
export const PROXY_URL     = 'http://127.0.0.1:3000/json-rpc';
export const VSS_URL       = 'http://127.0.0.1:8082/vss';

// ---------------------------------------------------------------------------
// 4. Generic helper functions
// ---------------------------------------------------------------------------

/** JSON-stringify with BigInt support and pretty-printing. */
export const json = obj =>
  JSON.stringify(obj, (_, v) => (typeof v === 'bigint' ? v.toString() : v), 2);

/** Promise-based sleep. */
export const sleep = ms => new Promise(r => setTimeout(r, ms));

/**
 * Display a plain-text result inside a <pre> element.
 * @param {string} outputId  - id of the wrapper element (set to visible)
 * @param {string} resultId  - id of the <pre> element that holds the text
 * @param {string} text      - content to display
 */
export function showResult(outputId, resultId, text) {
  const output = document.getElementById(outputId);
  const result = document.getElementById(resultId);
  if (output) output.style.display = 'block';
  if (result) result.textContent = text;
}

/**
 * Display a JSON object inside a <pre> element (pretty-printed).
 * @param {string} outputId
 * @param {string} resultId
 * @param {*}      obj
 */
export function showJson(outputId, resultId, obj) {
  showResult(outputId, resultId, json(obj));
}

/**
 * Display an error message inside a <pre> element.
 * @param {string} outputId
 * @param {string} resultId
 * @param {string} prefix   - contextual label (e.g. "Issue failed")
 * @param {*}      e        - the caught error
 */
export function showError(outputId, resultId, prefix, e) {
  showResult(outputId, resultId, prefix + ': ' + e);
  log(prefix + ': ' + e, 'log-err');
}

/**
 * Guard: ensure a wallet exists in state. Shows an error and returns false
 * if state.wallet is null.
 */
export function requireWallet(outputId, resultId) {
  if (!state.wallet) {
    showResult(outputId, resultId, 'Create a wallet first');
    log('No wallet — create one first', 'log-warn');
    return false;
  }
  return true;
}

/**
 * Guard: ensure both wallet and online object exist in state.
 */
export function requireOnline(outputId, resultId) {
  if (!state.wallet || !state.online) {
    showResult(outputId, resultId, 'Create a wallet and go online first');
    log('No wallet/online — set up first', 'log-warn');
    return false;
  }
  return true;
}

/**
 * Read an optional integer from an <input> by id.
 * Returns the parsed number, or undefined if blank.
 */
export function optionalInt(id) {
  const el = document.getElementById(id);
  if (!el || el.value.trim() === '') return undefined;
  return parseInt(el.value, 10);
}

/**
 * Read an optional string from an <input> by id.
 * Returns the trimmed string, or undefined if blank.
 */
export function optionalString(id) {
  const el = document.getElementById(id);
  if (!el || el.value.trim() === '') return undefined;
  return el.value.trim();
}

/**
 * Parse a comma-separated list of amounts into an array of numbers.
 * e.g. "1000,500" -> [1000, 500]
 */
export function parseAmounts(str) {
  return str
    .split(',')
    .map(s => s.trim())
    .filter(s => s.length > 0)
    .map(Number);
}

// ---------------------------------------------------------------------------
// 5. Activity log
// ---------------------------------------------------------------------------

/**
 * Append a timestamped message to the #log element.
 * @param {string} msg - message text
 * @param {string} [cls='log-info'] - CSS class (log-ok, log-err, log-warn, log-info)
 */
export function log(msg, cls) {
  const logEl = document.getElementById('log');
  if (!logEl) return;
  const ts = new Date().toLocaleTimeString();
  const span = document.createElement('span');
  span.className = cls || 'log-info';
  span.textContent = '[' + ts + '] ' + msg + '\n';
  logEl.appendChild(span);
  logEl.scrollTop = logEl.scrollHeight;
}

// ---------------------------------------------------------------------------
// 6. Copy-to-clipboard helper
// ---------------------------------------------------------------------------

/**
 * Copy the textContent of an element to the clipboard.
 * Attached to window so it can be used from inline onclick attributes.
 */
export function copyOutput(id) {
  const text = document.getElementById(id).textContent;
  navigator.clipboard.writeText(text).then(() => {}, () => {});
}
window.copyOutput = copyOutput;

// ---------------------------------------------------------------------------
// 7. Regtest helper functions
// ---------------------------------------------------------------------------

/**
 * Fund a Bitcoin address via the regtest helper service.
 * @param {string} address - the address to fund
 * @param {number} amount  - satoshi amount
 * @returns {Promise<Object>} parsed JSON response
 */
export async function fundAddress(address, amount) {
  const resp = await fetch(REGTEST_HELPER + '/fund', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ address, amount: String(amount) }),
  });
  return resp.json();
}

/**
 * Mine blocks via the regtest helper service.
 * @param {number} n - number of blocks to mine
 * @returns {Promise<Object>} parsed JSON response (includes .height)
 */
export async function mineBlocks(n) {
  const resp = await fetch(REGTEST_HELPER + '/mine', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ blocks: n }),
  });
  return resp.json();
}

/**
 * Poll the Esplora API until the tip reaches targetHeight (up to 30 s).
 * @param {number} targetHeight
 */
export async function waitForEsplora(targetHeight) {
  for (let i = 0; i < 30; i++) {
    const h = await fetch(ESPLORA_API + '/blocks/tip/height').then(r => r.text());
    if (parseInt(h) >= targetHeight) return;
    await sleep(1000);
  }
}

/**
 * Mine n blocks and wait for Esplora to catch up.
 * @param {number} n
 * @returns {Promise<Object>} result from mineBlocks
 */
export async function mineAndWait(n) {
  const result = await mineBlocks(n);
  await waitForEsplora(result.height);
  return result;
}

// ---------------------------------------------------------------------------
// 8. WASM initialisation
// ---------------------------------------------------------------------------

/**
 * Initialise the WASM module. Updates #status and the activity log.
 * @returns {Promise<boolean>} true on success
 */
export async function initWasm() {
  const statusEl = document.getElementById('status');
  try {
    await init();
    if (statusEl) {
      statusEl.textContent = 'WASM module loaded successfully';
      statusEl.className = 'status ready';
    }
    log('WASM initialized', 'log-ok');
    return true;
  } catch (e) {
    if (statusEl) {
      statusEl.textContent = 'Failed to load WASM module: ' + e;
      statusEl.className = 'status error';
    }
    log('WASM init failed: ' + e, 'log-err');
    return false;
  }
}

// ---------------------------------------------------------------------------
// 9. Bulk-enable buttons
// ---------------------------------------------------------------------------

/**
 * Enable a list of buttons by their ids.
 * @param {string[]} ids
 */
export function enableBtns(ids) {
  ids.forEach(id => {
    const el = document.getElementById(id);
    if (el) el.disabled = false;
  });
}

// ---------------------------------------------------------------------------
// 10. Wallet data helper
// ---------------------------------------------------------------------------

/**
 * Build a WalletData-compatible object from generated keys and fill the
 * #wallet-data textarea if present.
 * @param {Object} keys    - result of generate_keys / restore_keys
 * @param {string} network - e.g. "regtest"
 * @returns {Object} walletData
 */
export function fillWalletData(keys, network) {
  const netCapitalized = network.charAt(0).toUpperCase() + network.slice(1);
  const wd = {
    data_dir: ':memory:',
    bitcoin_network: netCapitalized,
    database_type: 'Sqlite',
    max_allocations_per_utxo: 5,
    account_xpub_vanilla: keys.account_xpub_vanilla,
    account_xpub_colored: keys.account_xpub_colored,
    mnemonic: keys.mnemonic,
    master_fingerprint: keys.master_fingerprint,
    vanilla_keychain: null,
    supported_schemas: ['Nia', 'Ifa'],
  };
  const el = document.getElementById('wallet-data');
  if (el) el.value = json(wd);
  return wd;
}

// ---------------------------------------------------------------------------
// 11. Shared CSS (inject into <head> with a <style> element)
// ---------------------------------------------------------------------------

export const SHARED_CSS = `
* { box-sizing: border-box; margin: 0; padding: 0; }
body { font-family: system-ui, sans-serif; background: #0d1117; color: #c9d1d9; padding: 2rem; max-width: 960px; margin: 0 auto; }
h1 { color: #58a6ff; margin-bottom: 0.5rem; }
.subtitle { color: #8b949e; margin-bottom: 1rem; font-size: 0.9rem; }
nav { margin-bottom: 2rem; }
nav a { color: #58a6ff; margin-right: 1.5rem; font-size: 0.9rem; text-decoration: none; }
nav a:hover { text-decoration: underline; }
nav a.active { font-weight: 700; border-bottom: 2px solid #58a6ff; padding-bottom: 2px; }
.status { padding: 0.5rem 1rem; border-radius: 6px; margin-bottom: 2rem; font-size: 0.85rem; }
.status.loading { background: #1c1f26; color: #d29922; }
.status.ready { background: #0d2818; color: #3fb950; }
.status.error { background: #2d1117; color: #f85149; }
.section { background: #161b22; border: 1px solid #30363d; border-radius: 8px; padding: 1.5rem; margin-bottom: 1.5rem; }
.section h2 { color: #58a6ff; font-size: 1.1rem; margin-bottom: 1rem; }
.section h3 { color: #8b949e; font-size: 0.95rem; margin-bottom: 0.75rem; margin-top: 0.75rem; }
.hint { color: #8b949e; font-size: 0.85rem; margin-bottom: 1rem; }
label { display: block; color: #8b949e; font-size: 0.85rem; margin-bottom: 0.3rem; }
select, input, textarea {
  width: 100%; padding: 0.5rem 0.75rem; background: #0d1117; border: 1px solid #30363d;
  border-radius: 6px; color: #c9d1d9; font-family: monospace; font-size: 0.9rem; margin-bottom: 1rem;
}
input[type="checkbox"] { width: auto; margin-bottom: 0; }
input[type="file"] { padding: 0.3rem; }
.checkbox-label {
  display: inline-flex; align-items: center; gap: 0.4rem;
  color: #8b949e; font-size: 0.85rem; margin-bottom: 1rem; cursor: pointer;
}
textarea { resize: vertical; min-height: 60px; }
select:focus, input:focus, textarea:focus { outline: none; border-color: #58a6ff; }
button {
  padding: 0.5rem 1.25rem; background: #238636; color: #fff; border: none;
  border-radius: 6px; cursor: pointer; font-size: 0.9rem; font-weight: 600;
}
button:hover { background: #2ea043; }
button:disabled { background: #21262d; color: #484f58; cursor: not-allowed; }
button.danger { background: #da3633; }
button.danger:hover { background: #f85149; }
button.danger:disabled { background: #21262d; color: #484f58; }
button.secondary { background: #30363d; }
button.secondary:hover { background: #484f58; }
button.secondary:disabled { background: #21262d; color: #484f58; }
button.accent { background: #1f6feb; }
button.accent:hover { background: #388bfd; }
button.accent:disabled { background: #21262d; color: #484f58; }
button.auto-btn { background: #8957e5; }
button.auto-btn:hover { background: #a371f7; }
button.auto-btn:disabled { background: #21262d; color: #484f58; }
.output { margin-top: 1rem; }
.output-label { color: #8b949e; font-size: 0.8rem; margin-bottom: 0.3rem; }
pre {
  background: #0d1117; border: 1px solid #30363d; border-radius: 6px;
  padding: 1rem; overflow-x: auto; font-size: 0.85rem; line-height: 1.5;
  max-height: 400px; overflow-y: auto; white-space: pre-wrap; word-break: break-all;
}
.field-row { display: flex; gap: 1rem; align-items: flex-end; }
.field-row > * { flex: 1; }
.field-row > button { flex: 0 0 auto; margin-bottom: 1rem; }
.btn-group { display: flex; gap: 0.5rem; margin-bottom: 1rem; flex-wrap: wrap; }
.copy-btn { background: #30363d; font-size: 0.75rem; padding: 0.3rem 0.6rem; margin-left: 0.5rem; }
.copy-btn:hover { background: #484f58; }
hr.divider { border: none; border-top: 1px solid #30363d; margin: 1rem 0; }
#log {
  background: #0d1117; border: 1px solid #30363d; border-radius: 6px;
  padding: 1rem; font-size: 0.8rem; line-height: 1.6; max-height: 400px;
  overflow-y: auto; white-space: pre-wrap; word-break: break-all; min-height: 60px;
}
.log-ok { color: #3fb950; }
.log-err { color: #f85149; }
.log-warn { color: #d29922; }
.log-info { color: #8b949e; }
.round-trip-result { margin-top: 0.5rem; font-size: 0.85rem; padding: 0.4rem 0.75rem; border-radius: 4px; }
.round-trip-result.pass { background: #0d2818; color: #3fb950; }
.round-trip-result.fail { background: #2d1117; color: #f85149; }
`;
