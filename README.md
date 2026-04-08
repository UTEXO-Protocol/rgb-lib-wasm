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
import init, {
  generateKeys, restoreKeys, checkProxyUrl,
  validateConsignmentOffchain,
  WasmWallet, WasmInvoice,
} from './pkg/rgb_lib_wasm.js';

await init();
```

### Key generation

```javascript
const keys = generateKeys('Regtest');
// keys: { mnemonic, accountXpubVanilla, accountXpubColored, masterFingerprint }

const restored = restoreKeys('Regtest', keys.mnemonic);
```

### Creating a wallet

```javascript
const walletData = {
  dataDir: ':memory:',
  bitcoinNetwork: 'Regtest',
  databaseType: 'Sqlite',
  maxAllocationsPerUtxo: 5,
  accountXpubVanilla: keys.accountXpubVanilla,
  accountXpubColored: keys.accountXpubColored,
  mnemonic: keys.mnemonic,
  masterFingerprint: keys.masterFingerprint,
  vanillaKeychain: null,
  supportedSchemas: ['Nia', 'Ifa'],
};

// create() restores state from IndexedDB if available
const wallet = await WasmWallet.create(JSON.stringify(walletData));
```

### Going online

```javascript
const online = await wallet.goOnline(true, 'http://localhost:8094/regtest/api');
await wallet.sync(online);
```

### Creating UTXOs

RGB allocations live on UTXOs. Before issuing or receiving assets you need colored UTXOs:

```javascript
const unsignedPsbt = await wallet.createUtxosBegin(online, true, 5, 1000, 1n, false);
const signedPsbt = wallet.signPsbt(unsignedPsbt);
const count = await wallet.createUtxosEnd(online, signedPsbt, false);
```

### Issuing assets

```javascript
// NIA (fixed supply)
const nia = wallet.issueAssetNia('TICK', 'My Token', 0, [1000]);

// IFA (inflatable supply)
const ifa = wallet.issueAssetIfa('IFAT', 'Inflatable', 0, [1000], [500], undefined);
```

### Sending RGB assets

All send operations follow a three-step PSBT workflow: **begin** (build unsigned PSBT) → **sign** → **end** (broadcast).

```javascript
const recipientMap = {
  [assetId]: [{
    recipientId: invoiceString,
    witnessData: { amountSat: 1000, blinding: null },
    amount: 100,
    transportEndpoints: ['rpc://localhost:3000/json-rpc'],
  }]
};
const psbt = await wallet.sendBegin(online, recipientMap, false, 1n, 1);
const signed = wallet.signPsbt(psbt);
const result = await wallet.sendEnd(online, signed, false);
// result: { txid, batchTransferIdx }
```

### Sending BTC

```javascript
const psbt = await wallet.sendBtcBegin(online, address, 50000n, 1n, false);
const signed = wallet.signPsbt(psbt);
const txid = await wallet.sendBtcEnd(online, signed, false);
```

### Receiving assets

```javascript
// Blind receive (UTXO-based)
const blind = wallet.blindReceive(null, 'Any', null, ['rpc://localhost:3000/json-rpc'], 1);
// blind: { invoice, recipientId, expirationTimestamp, ... }

// Witness receive (address-based)
const witness = wallet.witnessReceive(null, { Fungible: 100 }, null, ['rpc://localhost:3000/json-rpc'], 1);
```

### Parsing invoices

```javascript
const invoice = new WasmInvoice(invoiceString);
const data = invoice.invoiceData();
// data: { recipientId, assetSchema, assetId, assignment, network, expirationTimestamp, transportEndpoints }
const str = invoice.invoiceString(); // original string
```

### Querying wallet state

```javascript
const btcBalance = wallet.getBtcBalance();
const assetBalance = wallet.getAssetBalance(assetId);
const metadata = wallet.getAssetMetadata(assetId);
// metadata: { assetSchema, name, ticker, precision, initialSupply, maxSupply, knownCirculatingSupply, timestamp, ... }
const assets = wallet.listAssets([]);              // all schemas
const transfers = wallet.listTransfers(null);       // all assets
const unspents = wallet.listUnspents(false);
const transactions = wallet.listTransactions();
const address = wallet.getAddress();
```

### Fee estimation

```javascript
const feeRate = await wallet.getFeeEstimation(online, 6); // target 6 blocks
```

### Draining the wallet

```javascript
const psbt = await wallet.drainToBegin(online, destinationAddress, true, 1n);
const signed = wallet.signPsbt(psbt);
const txid = await wallet.drainToEnd(online, signed);
```

### Inflating an IFA asset

```javascript
const psbt = await wallet.inflateBegin(online, assetId, [200], 1n, 1);
const signed = wallet.signPsbt(psbt);
const result = await wallet.inflateEnd(online, signed);
```

### Local encrypted backup

Wallet state is encrypted with Scrypt + XChaCha20Poly1305 and returned as bytes:

```javascript
// Backup
const bytes = wallet.backup('my-password');
// bytes is a Uint8Array — save it as a file download

// Check if backup is needed
const needed = wallet.backupInfo(); // true if wallet changed since last backup

// Restore (wallet must be created first with the same mnemonic)
wallet.restoreBackup(backupBytes, 'my-password');
```

### VSS cloud backup

[VSS](https://github.com/lightningdevkit/vss-server) (Versioned Storage Service) provides cloud backup with optimistic locking:

```javascript
// Configure (signing key is a 32-byte hex-encoded secp256k1 secret key)
wallet.configureVssBackup('http://vss-server:8082/vss', 'my-store-id', signingKeyHex);

// Upload
const version = await wallet.vssBackup();

// Check status
const info = await wallet.vssBackupInfo();
// info: { backupExists, serverVersion, backupRequired }

// Restore
await wallet.vssRestoreBackup();

// Disable
wallet.disableVssBackup();
```

### Transfer management

```javascript
const refreshResult = await wallet.refresh(online, null, [], false);
const failed = await wallet.failTransfers(online, null, false, false);
const deleted = wallet.deleteTransfers(null, false);
```

### Utilities

```javascript
// Validate an RGB proxy server
await checkProxyUrl('http://localhost:3000/json-rpc');

// Validate an RGB consignment offchain (before TX is broadcast)
// consignmentBytes is a Uint8Array of the strict-encoded consignment
const result = validateConsignmentOffchain(consignmentBytes, txid, 'Regtest');
// result: { valid: true, warnings: [...] }
// result: { valid: false, error: "invalid", details: "..." }

// PSBT signing and finalization (standalone)
const signed = wallet.signPsbt(unsignedPsbtBase64);
const finalized = wallet.finalizePsbt(signedPsbtBase64);
```

## API reference

### Standalone functions

| Function | Description |
|----------|-------------|
| `generateKeys(network)` | Generate a new BIP39 mnemonic and derive xpubs |
| `restoreKeys(network, mnemonic)` | Restore xpubs from a mnemonic |
| `checkProxyUrl(url)` | Validate an RGB proxy server URL |
| `validateConsignmentOffchain(bytes, txid, network)` | Validate an RGB consignment using bundled witness data |

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
| `getWalletData()` | no | Return WalletData as JS object |
| `getAddress()` | no | Get a new Bitcoin address |
| `getBtcBalance()` | no | Get BTC balance |
| `getAssetBalance(assetId)` | no | Get balance for an RGB asset |
| `getAssetMetadata(assetId)` | no | Get metadata for an RGB asset (name, ticker, precision, supply, etc.) |
| `listAssets(schemas)` | no | List known RGB assets |
| `listTransfers(assetId?)` | no | List RGB transfers |
| `listUnspents(settledOnly)` | no | List unspent outputs with RGB allocations |
| `listTransactions()` | no | List Bitcoin transactions |
| `goOnline(skipCheck, indexerUrl)` | yes | Connect to an Esplora indexer |
| `sync(online)` | yes | Sync wallet with indexer |
| `createUtxosBegin(...)` | yes | Prepare PSBT to create colored UTXOs |
| `createUtxosEnd(online, psbt, skipSync)` | yes | Broadcast UTXO creation PSBT |
| `issueAssetNia(ticker, name, precision, amounts)` | no | Issue a Non-Inflatable Asset |
| `issueAssetIfa(...)` | no | Issue an Inflatable Fungible Asset |
| `blindReceive(...)` | no | Create a blind receive invoice |
| `witnessReceive(...)` | no | Create a witness receive invoice |
| `sendBegin(...)` | yes | Prepare RGB send PSBT |
| `sendEnd(online, psbt, skipSync)` | yes | Broadcast RGB send PSBT |
| `sendBtcBegin(...)` | yes | Prepare BTC send PSBT |
| `sendBtcEnd(online, psbt, skipSync)` | yes | Broadcast BTC send PSBT |
| `getFeeEstimation(online, blocks)` | yes | Estimate fee rate for target blocks |
| `drainToBegin(online, addr, destroyAssets, feeRate)` | yes | Prepare drain PSBT |
| `drainToEnd(online, psbt)` | yes | Broadcast drain PSBT |
| `inflateBegin(...)` | yes | Prepare IFA inflation PSBT |
| `inflateEnd(online, psbt)` | yes | Broadcast IFA inflation PSBT |
| `listUnspentsVanilla(online, minConf, skipSync)` | yes | List non-colored UTXOs |
| `refresh(online, assetId?, filter, skipSync)` | yes | Refresh pending transfers |
| `failTransfers(online, batchIdx?, noAssetOnly, skipSync)` | yes | Fail pending transfers |
| `deleteTransfers(batchIdx?, noAssetOnly)` | no | Delete failed transfers |
| `signPsbt(psbt)` | no | Sign a PSBT |
| `finalizePsbt(psbt)` | no | Finalize a signed PSBT |
| `backup(password)` | no | Create encrypted backup (returns bytes) |
| `restoreBackup(bytes, password)` | no | Restore from encrypted backup |
| `backupInfo()` | no | Check if backup is needed |
| `configureVssBackup(url, storeId, keyHex)` | no | Configure VSS cloud backup |
| `disableVssBackup()` | no | Disable VSS cloud backup |
| `vssBackup()` | yes | Upload backup to VSS server |
| `vssRestoreBackup()` | yes | Restore from VSS server |
| `vssBackupInfo()` | yes | Query VSS backup status |

## Cargo features

| Feature | Default | Description |
|---------|---------|-------------|
| `esplora` | yes | Online operations via Esplora HTTP indexer |
| `camel_case` | yes | camelCase JSON serialization for all types |


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
