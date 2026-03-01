# RGB Lib WebAssembly Bindings

WebAssembly bindings for RGB Lib, allowing it to be used in browsers and Node.js environments.

This package follows the same pattern as [bdk-wasm](https://github.com/bitcoindevkit/bdk-wasm).

## About

The `rgb-lib-wasm` library provides access to RGB Lib functionality in JavaScript and Node.js environments (and eventually any device supporting WebAssembly). It compiles RGB Lib for the `wasm32-unknown-unknown` target and uses `wasm-bindgen` to create TypeScript bindings.

This library offers RGB wallet functionality:

* RGB asset management (NIA, CFA, IFA, UDA)
* UTXO management
* Wallet state management
* Asset transfers
* Dynamic addresses
* And much more

## ✅ In-Memory Support

**In-memory mode support added to rgb-lib!**

WASM bindings now use in-memory mode:
- ✅ SQLite in-memory (`sqlite::memory:`)
- ✅ RGB runtime in-memory (`Stock::in_memory()`)
- ✅ No file system required

**Usage**: Set `data_dir = ":memory:"` in `WalletData`.

See:
- [ARCHITECTURE.md](ARCHITECTURE.md) — communication details
- [SQLITE_IN_MEMORY.md](SQLITE_IN_MEMORY.md) — how SQLite in-memory works
- [COMMUNICATION.md](COMMUNICATION.md) — communication diagrams

## WASM Considerations

⚠️ **Warning**: There are several limitations to using RGB Lib in WASM. Basically any functionality that requires the OS standard library is not directly available in WASM. However, there are viable workarounds documented below.

### No access to the file system

With no direct access to the file system, persistence cannot be handled by RGB Lib directly. Instead, an in-memory wallet must be used in the WASM environment, and the data must be exported using wallet methods. The persisted data can be passed to wallet methods to recover the wallet state.

### Network access is limited to http(s)

This essentially means the library only supports Esplora as blockchain client. Both RPC and Electrum clients require sockets and will not work for RGB Lib in a WASM environment out of the box.

## Building

### Requirements

* Install Rust
* Install [wasm-pack](https://rustwasm.github.io/wasm-pack/)
* **Install WASI SDK** (required for C code compilation - see below)

### Setup WASI SDK

**⚠️ Important**: The `secp256k1-sys` dependency requires C code compilation. You need to install and configure WASI SDK:

1. **Extract WASI SDK** (if you have the archive):
```bash
cd bindings/wasm
tar -xzf wasi-sdk-29.0-arm64-macos.tar.gz
```

2. **Setup environment variables**:
```bash
source bindings/wasm/setup-env.sh
```

Or manually:
```bash
export CC_wasm32_unknown_unknown="$(pwd)/bindings/wasm/wasi-sdk-29.0-arm64-macos/bin/clang"
export AR_wasm32_unknown_unknown="$(pwd)/bindings/wasm/wasi-sdk-29.0-arm64-macos/bin/llvm-ar"
export CFLAGS_wasm32_unknown_unknown="--target=wasm32-wasi"
```

See [INSTALL_WASI.md](INSTALL_WASI.md) for detailed instructions.

### Build with `wasm-pack`

**⚠️ Current Status**: Building is partially working. WASI SDK is configured and `secp256k1-sys` compiles successfully, but there are issues with `async-std` dependencies (`errno`, `polling`) that don't support `wasm32-unknown-unknown`. See [WASM_TARGET_ISSUES.md](WASM_TARGET_ISSUES.md) for details and possible solutions.

**Important**: You need the `wasm32-unknown-unknown` toolchain to be installed:

```bash
rustup target add wasm32-unknown-unknown
```

**Before building**, make sure WASI SDK environment variables are set:

```bash
source setup-env.sh
```

To build for browser/web:

```bash
wasm-pack build --target bundler --features "esplora"
```

To build for Node.js:

```bash
wasm-pack build --target nodejs --features "esplora"
```

### Troubleshooting

If you encounter compilation errors:

- **`secp256k1-sys` errors**: See [INSTALL_WASI.md](INSTALL_WASI.md) for WASI SDK setup
- **`errno`, `polling`, `async-std` errors**: See [WASM_TARGET_ISSUES.md](WASM_TARGET_ISSUES.md) - these dependencies don't support `wasm32-unknown-unknown`
- **Other build issues**: See [BUILD.md](BUILD.md) - build instructions and common problems

### Test in Headless Browsers with `wasm-pack test`

```bash
wasm-pack test --headless --firefox
```

Works with `--firefox`, `--chrome` or `--safari`.

## Usage

### Browser/Web

```bash
yarn add rgb-lib-wasm-web
```

### Node.js

```bash
yarn add rgb-lib-wasm-node
```

## Development

See the main [README.md](../../README.md) for development guidelines.

## License

Licensed under either of

* Apache License, Version 2.0, (LICENSE-APACHE or <https://www.apache.org/licenses/LICENSE-2.0>)
* MIT license (LICENSE-MIT or <https://opensource.org/licenses/MIT>)

at your option.
