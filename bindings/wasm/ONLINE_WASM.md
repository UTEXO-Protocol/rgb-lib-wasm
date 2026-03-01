# Fully online in WASM test — what is needed

To go "fully online" in the browser test (sync with blockchain, balances, refresh), the following is required.

## Enabled (step 1)

- **rgb-lib**: added **esplora-wasm** feature (no rustls): `bdk_esplora`, `bp-esplora`, `reqwest`, `rgb-ops/esplora_blocking`.
- **Target deps for wasm32**: in `[target.'cfg(target_arch = "wasm32")'.dependencies]` added `bdk_esplora` (async-https), `reqwest` (json, wasm), `bp-esplora` — for wasm32 these are used instead of blocking/rustls.
- **Code**: all `#[cfg(feature = "esplora")]` replaced with `#[cfg(any(feature = "esplora", feature = "esplora-wasm"))]` in `lib.rs`, `utils.rs`, `wallet/online.rs`, `error.rs`.
- **bindings/wasm**: in `Cargo.toml` rgb-lib is used with `features = ["esplora-wasm"]`.

**Check:** from `bindings/wasm` run:
```bash
cargo check --target wasm32-unknown-unknown --no-default-features --features esplora-wasm
```
or full build:
```bash
./test-now.sh
```
If errors appear (e.g. blocking Esplora API on wasm32), add an async path or `#[cfg(not(target_arch = "wasm32"))]` for blocking code (steps 2–3 in the checklist below).

---

## What "fully online" means

- **go_online** — connect wallet to indexer (Esplora URL), sync UTXO and tx graph.
- **refresh** — update transfer statuses (pending → settled etc.).
- **get_btc_balance** — get BTC balance (after sync).
- **list_assets / get_asset_balance** — assets and their balances.

Currently in WASM only **offline** flows are available: key generation, wallet creation (in-memory), export/import state. Methods `go_online`, `refresh`, `get_btc_balance` are not in bindings, and in rgb-lib they depend on **esplora** (or electrum), which we do not enable for WASM.

---

## Step-by-step requirements

### 1. Enable Esplora for wasm32 in rgb-lib

- **Current:** in root `Cargo.toml` feature `esplora` pulls `bdk_esplora` with `blocking-https-rustls`, `reqwest`, `rustls`. In WASM build rgb-lib is used **without** `esplora` (`default-features = false`, `features = []` in `bindings/wasm`) to avoid rustls/ring and blocking HTTP.
- **Needed:** separate config for `target_arch = "wasm32"`:
  - Use `bdk_esplora` with **async-https** feature (and WASM-compatible TLS if needed).
  - Use `bp-esplora` with async API for WASM (if such option exists).
  - Use an HTTP client that works in the browser: e.g. `reqwest` with `wasm` feature or similar (gloo-net etc.).
  - Runtime: `wasm-bindgen-futures` and possibly `gloo-timers` (as in bdk-wasm).

Result: rgb-lib `Cargo.toml` needs conditional deps like `[target.'cfg(target_arch = "wasm32")'.dependencies]` for esplora/reqwest with wasm features, without the current blocking rustls used on desktop.

### 2. Async path in rgb-lib for WASM

- **Current:** `go_online`, `refresh`, `get_btc_balance` in rgb-lib use **blocking** Esplora client (`BlockingClient`, `esplora_blocking`).
- **Needed:** for `#[cfg(target_arch = "wasm32")]` call **async** API (e.g. `EsploraAsyncExt` in bdk_esplora) and expose async through the public API. So either:
  - add async variants (`go_online_async`, `refresh_async`, `get_btc_balance_async`), or
  - make the existing methods async where they hit the indexer (then rgb-lib signatures change for WASM).

Also, WASM must not use a blocking runtime (blocking thread pool), only `wasm-bindgen-futures` and the browser event loop.

### 3. Export in WASM bindings (bindings/wasm)

- **Current:** from the wallet in WASM only creation from JSON, `get_wallet_data`, `export_state`, `from_state` are exported.
- **Needed:**
  - Export **Online** type (or its JS equivalent: e.g. id + indexer URL).
  - Export **go_online**(wallet, indexer_url, skip_consistency_check) — preferably async, since browser network sync should be async.
  - Export **refresh**(wallet, online, asset_id, filter, skip_sync).
  - Export **get_btc_balance**(wallet, online, skip_sync).
  - Optionally **list_assets** / **get_asset_balance** etc. if needed in the test.

These methods exist in rgb-lib but are behind `#[cfg(feature = "esplora")]` and are blocking; in bindings they must be called from async wrappers and only when esplora is enabled for wasm32.

### 4. Test page (fully online)

- Create wallet (as now: keys + `create_wallet_data` + `Wallet::new`).
- Call **go_online** with a public Esplora URL (e.g. `https://blockstream.info/api` for mainnet or testnet).
- Wait for completion (async).
- Call **refresh** (and **get_btc_balance** if needed).
- Show in UI: "online", balance, asset list (if exported).

Note: CORS — a public Esplora may reject requests from another origin; then you need your own proxy or an Esplora with CORS.

---

## Short checklist

| Step | Action | Where |
|------|--------|-------|
| 1 | Enable esplora for wasm32 (async-https, reqwest wasm, no blocking rustls) | rgb-lib `Cargo.toml` |
| 2 | Implement async path for go_online/refresh/get_btc_balance on wasm32 | rgb-lib `src/wallet/online.rs` etc. |
| 3 | Export Online, go_online, refresh, get_btc_balance to WASM | `bindings/wasm/src/wallet.rs` |
| 4 | Add "fully online" test (go_online → refresh → balance) | `examples/simple-test.html` or new example |

---

## Why you can't "just enable" online today

- In rgb-lib Esplora is enabled via **esplora** feature with **blocking** HTTP and **rustls**. In WASM blocking calls and current rustls/ring are not suitable.
- A separate path for wasm32 is needed: **async** Esplora + browser HTTP client + export of async methods to JS (e.g. via `#[wasm_bindgen]` and `Promise`).

After steps 1–4, the browser test can go "fully online": go_online → refresh → show balance and assets.
