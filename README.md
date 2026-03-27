# rgb-lib-wasm

A WebAssembly port of [rgb-lib](https://github.com/UTEXO-Protocol/rgb-lib) for managing RGB protocol wallets in the browser.

> **Beta Software** — This library is under active development and has not been audited.
> APIs may change without notice between versions. Use at your own risk.

## What it does

rgb-lib-wasm lets you issue, send, and receive RGB assets from a web browser. It replaces the native Rust dependencies (filesystem, SQLite, TCP sockets) with browser-compatible alternatives:

- **In-memory database** with IndexedDB persistence
- **Esplora indexer** over HTTP
- **Encrypted backup** to a local file or a remote VSS server

Supported asset types:
- **NIA** (Non-Inflatable Asset)
- **IFA** (Inflatable Fungible Asset).

## Prerequisites

- Rust 1.85.0+ with the `wasm32-unknown-unknown` target
- [wasm-pack](https://rustwasm.github.io/wasm-pack/) 0.14+

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-pack@0.14.0
```

## Building

```bash
cd bindings/wasm
./build.sh
```

This produces the `pkg/` directory containing an npm-ready package (`rgb_lib_wasm.js`, `.d.ts` types, and the `.wasm` binary).

### Trying the examples

```bash
cd bindings/wasm
python3 -m http.server 8000
# Open http://localhost:8000/examples/index.html
```

The `examples/` folder contains interactive pages that cover the full API, split by topic:

| Page | Description |
|------|-------------|
| [`index.html`](bindings/wasm/examples/index.html) | Quick start guide + automated "Run All" flow |
| [`wallet.html`](bindings/wasm/examples/wallet.html) | Key management, wallet creation, go online, fund, create UTXOs, wallet state queries, fee estimation |
| [`bitcoin.html`](bindings/wasm/examples/bitcoin.html) | Send BTC, drain wallet, PSBT sign/finalize utilities |
| [`rgb.html`](bindings/wasm/examples/rgb.html) | Issue NIA/IFA, blind/witness receive, send RGB, inflate IFA, refresh/fail/delete transfers |
| [`backup.html`](bindings/wasm/examples/backup.html) | Local encrypted backup, VSS cloud backup, check proxy URL, error handling tests |

All pages share `examples/lib/shared.js` (WASM init, helpers, regtest utilities). Each page is self-contained with its own wallet setup section. Online operations (funding, sending, etc.) require the regtest Docker services — see [DEV-ENVIRONMENT.md](DEV-ENVIRONMENT.md) for setup.

## Usage

```javascript
import init, { generate_keys, restore_keys, check_proxy_url, WasmWallet, WasmInvoice } from './pkg/rgb_lib_wasm.js';

await init();
```

### Key generation

```javascript
const keys = generate_keys('Regtest');
// keys: { mnemonic, account_xpub_vanilla, account_xpub_colored, master_fingerprint }

const restored = restore_keys('Regtest', keys.mnemonic);
```

### Creating a wallet

```javascript
const walletData = {
  data_dir: ':memory:',
  bitcoin_network: 'Regtest',
  database_type: 'Sqlite',
  max_allocations_per_utxo: 5,
  account_xpub_vanilla: keys.account_xpub_vanilla,
  account_xpub_colored: keys.account_xpub_colored,
  mnemonic: keys.mnemonic,
  master_fingerprint: keys.master_fingerprint,
  vanilla_keychain: null,
  supported_schemas: ['Nia', 'Ifa'],
};

// create() restores state from IndexedDB if available
const wallet = await WasmWallet.create(JSON.stringify(walletData));
```

### Going online

```javascript
const online = await wallet.go_online(true, 'http://localhost:8094/regtest/api');
await wallet.sync(online);
```

### Creating UTXOs

RGB allocations live on UTXOs. Before issuing or receiving assets you need colored UTXOs:

```javascript
const unsignedPsbt = await wallet.create_utxos_begin(online, true, 5, 1000, 1n, false);
const signedPsbt = wallet.sign_psbt(unsignedPsbt);
const count = await wallet.create_utxos_end(online, signedPsbt, false);
```

### Issuing assets

```javascript
// NIA (fixed supply)
const nia = wallet.issue_asset_nia('TICK', 'My Token', 0, [1000]);

// IFA (inflatable supply)
const ifa = wallet.issue_asset_ifa('IFAT', 'Inflatable', 0, [1000], [500], 1, undefined);
```

### Sending RGB assets

All send operations follow a three-step PSBT workflow: **begin** (build unsigned PSBT) → **sign** → **end** (broadcast).

```javascript
const recipientMap = {
  [assetId]: [{
    recipient_id: invoiceString,
    witness_data: { amount_sat: 1000, blinding: null },
    amount: 100,
    transport_endpoints: ['rpc://localhost:3000/json-rpc'],
  }]
};
const psbt = await wallet.send_begin(online, recipientMap, false, 1n, 1);
const signed = wallet.sign_psbt(psbt);
const result = await wallet.send_end(online, signed, false);
// result: { txid, batch_transfer_idx }
```

### Sending BTC

```javascript
const psbt = await wallet.send_btc_begin(online, address, 50000n, 1n, false);
const signed = wallet.sign_psbt(psbt);
const txid = await wallet.send_btc_end(online, signed, false);
```

### Receiving assets

```javascript
// Blind receive (UTXO-based)
const blind = wallet.blind_receive(null, 'Any', null, ['rpc://localhost:3000/json-rpc'], 1);
// blind: { invoice, recipient_id, expiration_timestamp, ... }

// Witness receive (address-based)
const witness = wallet.witness_receive(null, { Fungible: 100 }, null, ['rpc://localhost:3000/json-rpc'], 1);
```

### Parsing invoices

```javascript
const invoice = new WasmInvoice(invoiceString);
const data = invoice.invoiceData();
// data: { recipient_id, asset_schema, asset_id, assignment, network, expiration_timestamp, transport_endpoints }
const str = invoice.invoiceString(); // original string
```

### Querying wallet state

```javascript
const btcBalance = wallet.get_btc_balance();
const assetBalance = wallet.get_asset_balance(assetId);
const metadata = wallet.get_asset_metadata(assetId);
// metadata: { asset_schema, name, ticker, precision, initial_supply, max_supply, known_circulating_supply, timestamp, ... }
const assets = wallet.list_assets([]);              // all schemas
const transfers = wallet.list_transfers(null);       // all assets
const unspents = wallet.list_unspents(false);
const transactions = wallet.list_transactions();
const address = wallet.get_address();
```

### Fee estimation

```javascript
const feeRate = await wallet.get_fee_estimation(online, 6); // target 6 blocks
```

### Draining the wallet

```javascript
const psbt = await wallet.drain_to_begin(online, destinationAddress, true, 1n);
const signed = wallet.sign_psbt(psbt);
const txid = await wallet.drain_to_end(online, signed);
```

### Inflating an IFA asset

```javascript
const psbt = await wallet.inflate_begin(online, assetId, [200], 1n, 1);
const signed = wallet.sign_psbt(psbt);
const result = await wallet.inflate_end(online, signed);
```

### Local encrypted backup

Wallet state is encrypted with Scrypt + XChaCha20Poly1305 and returned as bytes:

```javascript
// Backup
const bytes = wallet.backup('my-password');
// bytes is a Uint8Array — save it as a file download

// Check if backup is needed
const needed = wallet.backup_info(); // true if wallet changed since last backup

// Restore (wallet must be created first with the same mnemonic)
wallet.restore_backup(backupBytes, 'my-password');
```

### VSS cloud backup

[VSS](https://github.com/lightningdevkit/vss-server) (Versioned Storage Service) provides cloud backup with optimistic locking:

```javascript
// Configure (signing key is a 32-byte hex-encoded secp256k1 secret key)
wallet.configure_vss_backup('http://vss-server:8082/vss', 'my-store-id', signingKeyHex);

// Upload
const version = await wallet.vss_backup();

// Check status
const info = await wallet.vss_backup_info();
// info: { backup_exists, server_version, backup_required }

// Restore
await wallet.vss_restore_backup();

// Disable
wallet.disable_vss_backup();
```

### Transfer management

```javascript
const refreshResult = await wallet.refresh(online, null, [], false);
const failed = await wallet.fail_transfers(online, null, false, false);
const deleted = wallet.delete_transfers(null, false);
```

### Utilities

```javascript
// Validate an RGB proxy server
await check_proxy_url('http://localhost:3000/json-rpc');

// PSBT signing and finalization (standalone)
const signed = wallet.sign_psbt(unsignedPsbtBase64);
const finalized = wallet.finalize_psbt(signedPsbtBase64);
```

## API reference

### Standalone functions

| Function | Description |
|----------|-------------|
| `generate_keys(network)` | Generate a new BIP39 mnemonic and derive xpubs |
| `restore_keys(network, mnemonic)` | Restore xpubs from a mnemonic |
| `check_proxy_url(url)` | Validate an RGB proxy server URL |

### `WasmInvoice`

| Method | Description |
|--------|-------------|
| `new(invoiceString)` | Parse and validate an RGB invoice string |
| `invoiceData()` | Return parsed `InvoiceData` as a JS object |
| `invoiceString()` | Return the original invoice string |

### `WasmWallet` methods

| Method | Async | Description |
|--------|-------|-------------|
| `new(walletDataJson)` | no | Create wallet (no IndexedDB restore) |
| `create(walletDataJson)` | yes | Create wallet with IndexedDB restore |
| `get_wallet_data()` | no | Return WalletData as JS object |
| `get_address()` | no | Get a new Bitcoin address |
| `get_btc_balance()` | no | Get BTC balance |
| `get_asset_balance(assetId)` | no | Get balance for an RGB asset |
| `get_asset_metadata(assetId)` | no | Get metadata for an RGB asset (name, ticker, precision, supply, etc.) |
| `list_assets(schemas)` | no | List known RGB assets |
| `list_transfers(assetId?)` | no | List RGB transfers |
| `list_unspents(settledOnly)` | no | List unspent outputs with RGB allocations |
| `list_transactions()` | no | List Bitcoin transactions |
| `go_online(skipCheck, indexerUrl)` | yes | Connect to an Esplora indexer |
| `sync(online)` | yes | Sync wallet with indexer |
| `create_utxos_begin(...)` | yes | Prepare PSBT to create colored UTXOs |
| `create_utxos_end(online, psbt, skipSync)` | yes | Broadcast UTXO creation PSBT |
| `issue_asset_nia(ticker, name, precision, amounts)` | no | Issue a Non-Inflatable Asset |
| `issue_asset_ifa(...)` | no | Issue an Inflatable Fungible Asset |
| `blind_receive(...)` | no | Create a blind receive invoice |
| `witness_receive(...)` | no | Create a witness receive invoice |
| `send_begin(...)` | yes | Prepare RGB send PSBT |
| `send_end(online, psbt, skipSync)` | yes | Broadcast RGB send PSBT |
| `send_btc_begin(...)` | yes | Prepare BTC send PSBT |
| `send_btc_end(online, psbt, skipSync)` | yes | Broadcast BTC send PSBT |
| `get_fee_estimation(online, blocks)` | yes | Estimate fee rate for target blocks |
| `drain_to_begin(online, addr, destroyAssets, feeRate)` | yes | Prepare drain PSBT |
| `drain_to_end(online, psbt)` | yes | Broadcast drain PSBT |
| `inflate_begin(...)` | yes | Prepare IFA inflation PSBT |
| `inflate_end(online, psbt)` | yes | Broadcast IFA inflation PSBT |
| `list_unspents_vanilla(online, minConf, skipSync)` | yes | List non-colored UTXOs |
| `refresh(online, assetId?, filter, skipSync)` | yes | Refresh pending transfers |
| `fail_transfers(online, batchIdx?, noAssetOnly, skipSync)` | yes | Fail pending transfers |
| `delete_transfers(batchIdx?, noAssetOnly)` | no | Delete failed transfers |
| `sign_psbt(psbt)` | no | Sign a PSBT |
| `finalize_psbt(psbt)` | no | Finalize a signed PSBT |
| `backup(password)` | no | Create encrypted backup (returns bytes) |
| `restore_backup(bytes, password)` | no | Restore from encrypted backup |
| `backup_info()` | no | Check if backup is needed |
| `configure_vss_backup(url, storeId, keyHex)` | no | Configure VSS cloud backup |
| `disable_vss_backup()` | no | Disable VSS cloud backup |
| `vss_backup()` | yes | Upload backup to VSS server |
| `vss_restore_backup()` | yes | Restore from VSS server |
| `vss_backup_info()` | yes | Query VSS backup status |

## Cargo features

| Feature | Default | Description |
|---------|---------|-------------|
| `esplora` | yes | Online operations via Esplora HTTP indexer |
| `camel_case` | no | camelCase JSON serialization for all types |


## Running tests

```bash
# Native unit tests (94 tests)
cargo test --lib

# WASM unit tests (headless Firefox)
wasm-pack test --headless --firefox --release -- --test keys_wasm
wasm-pack test --headless --firefox --release -- --test wallet_wasm

# Integration tests (requires regtest Docker services)
cd tests && bash regtest.sh start
cd .. && wasm-pack test --headless --firefox --release -- --test integration_wasm
cd tests && bash regtest.sh stop
```

See [DEV-ENVIRONMENT.md](DEV-ENVIRONMENT.md) for the full regtest infrastructure setup, CI configuration, Docker Compose details, and troubleshooting.

## License

MIT
