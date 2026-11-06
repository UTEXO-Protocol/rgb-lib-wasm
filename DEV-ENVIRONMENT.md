# Regtest Infrastructure

Development and CI infrastructure for rgb-lib-wasm integration tests.

## Architecture

Integration tests run in a headless Firefox browser (via wasm-pack/wasm-bindgen-test) against a local regtest Bitcoin network. The test environment consists of 10 Docker services:

The WASM tests (running in headless Firefox) communicate with all services over HTTP:

- **esplora** (:8094) — Block explorer and indexer. Bundles its own internal bitcoind for block indexing.
- **bitcoind** (:18443) — Primary Bitcoin Core node. Connected to esplora's bitcoind via p2p (:18444) so both see the same chain.
- **regtest-helper** (:8080) — Lightweight HTTP server that wraps bitcoind RPC for mining, funding, and status checks.
- **RGB proxy** (:3000): rgb-proxy-server 0.4.0 (proxy protocol 1.0), the relay for RGB transfers between wallets. It answers the browser with CORS headers itself.
- **RGB proxy B** (:3010): a second 0.4.0 relay, used by the batch and receive tests.
- **RGB proxy auth** (:3020): a 0.4.0 relay that only accepts calls signed with the key of `PROXY_AUTH_PUBKEYS`, used by the proxy authentication test.
- **anvil** (:8545): local EVM node for BFA validation.
- **vss-cors-proxy** (:8082) — Nginx reverse proxy that adds CORS headers, forwarding to the VSS server.
- **vss-server** (:8080 internal) — Versioned Storage Service for cloud backups. Not browser-accessible directly (no CORS).
- **vss-postgres** (:5432 internal) — PostgreSQL database backing the VSS server.

### Why a CORS proxy?

The VSS server does not send CORS headers. Browser-based WASM tests are blocked by the same-origin policy when making direct requests. An nginx reverse proxy sits in front of the VSS server and adds `Access-Control-Allow-Origin: *` headers.

### Why two bitcoind nodes?

Esplora bundles its own bitcoind for block indexing. Our primary bitcoind handles wallet funding and block mining (via regtest-helper). The two nodes are connected as peers so both see the same chain.

## Service endpoints

| Service | Port | URL | Purpose |
|---------|------|-----|---------|
| bitcoind | 18443 | `http://127.0.0.1:18443` | Bitcoin Core RPC |
| esplora | 8094 | `http://127.0.0.1:8094/regtest/api` | Block explorer / indexer API |
| esplora (electrum) | 50004 | — | Not used (WASM can't do TCP) |
| RGB proxy | 3000 | `http://127.0.0.1:3000/json-rpc` | RGB consignment relay (0.4.0) |
| RGB proxy B | 3010 | `http://127.0.0.1:3010/json-rpc` | Second relay for the batch and receive tests (0.4.0) |
| RGB proxy auth | 3020 | `http://127.0.0.1:3020/json-rpc` | Relay with proxy authentication (0.4.0) |
| anvil | 8545 | `http://127.0.0.1:8545` | Local EVM node for BFA validation |
| regtest-helper | 8080 | `http://127.0.0.1:8080` | Mining / funding helper |
| VSS server | 8081 | `http://127.0.0.1:8081` | VSS direct (not browser-accessible) |
| VSS CORS proxy | 8082 | `http://127.0.0.1:8082/vss` | VSS via nginx CORS proxy |
| PostgreSQL | 5432 | — | VSS database (internal only) |

## Local development

### Starting services

```bash
cd tests
bash regtest.sh start
```

This will:
1. Start all 10 Docker containers
2. Wait for each service to become healthy
3. Connect the two bitcoind nodes as peers
4. Mine 111 initial blocks (required for coinbase maturity)
5. Wait for esplora to sync to height 111
6. Wait for the VSS server (via CORS proxy) to respond

### Funding a wallet

```bash
# Fund an address with 1 BTC
bash tests/regtest.sh fund <address> 1.0

# Mine a block to confirm the funding tx
bash tests/regtest.sh mine 1
```

### Other commands

```bash
bash tests/regtest.sh mine 10      # Mine 10 blocks
bash tests/regtest.sh status       # Check service health
bash tests/regtest.sh stop         # Stop all services
```

### Running tests locally

```bash
# Unit tests (no Docker needed)
cargo test --lib

# WASM key and proof of work tests (no Docker needed)
wasm-pack test --headless --firefox --release -- --test keys_wasm
WASM_BINDGEN_TEST_TIMEOUT=300 wasm-pack test --headless --firefox --release -- --test pow_wasm

# proof of work time at the highest difficulty (slow, not in CI)
WASM_BINDGEN_TEST_TIMEOUT=3600 wasm-pack test --headless --firefox --release -- --test pow_wasm -- --nocapture --include-ignored pow_max_difficulty_time

# Integration tests (Docker required)
cd tests && bash regtest.sh start && cd ..
wasm-pack test --headless --firefox --release -- --test wallet_wasm
wasm-pack test --headless --firefox --release bindings/wasm --test bindings_wasm
wasm-pack test --headless --firefox --release -- --test integration_wasm
WASM_BINDGEN_TEST_TIMEOUT=1800 wasm-pack test --headless --firefox --release --lib -- test_bfa::
WASM_BINDGEN_TEST_TIMEOUT=1800 wasm-pack test --headless --firefox --release --lib -- test_burn::
WASM_BINDGEN_TEST_TIMEOUT=3600 wasm-pack test --headless --firefox --release --lib -- test_mint::
cd tests && bash regtest.sh stop
```

The integration test timeout is controlled by `WASM_BINDGEN_TEST_TIMEOUT` (default: 60s, CI uses 300s).

## CI configuration

CI runs on GitHub Actions (`.github/workflows/ci.yml`). Four jobs:

### 1. `check` — WASM compilation check

```yaml
cargo check --target wasm32-unknown-unknown
```

Catches compilation errors without building the full test suite.

### 2. `native-tests` — Rust unit tests

```yaml
cargo test --lib
```

Runs 94 native unit tests (database, crypto, backup, VSS encryption).

### 3. `wasm-tests` — WASM unit tests

```yaml
wasm-pack test --headless --firefox --release -- --test keys_wasm
WASM_BINDGEN_TEST_TIMEOUT=300 wasm-pack test --headless --firefox --release -- --test pow_wasm
```

Key generation, key restoration and proof of work. No Docker needed.

### 4. `integration-tests` — Full end-to-end

This job:
1. Builds Docker services from `tests/docker-compose.ci.yml`
2. Waits for all services (with timeouts)
3. Connects bitcoind peers
4. Mines 111 initial blocks
5. Waits for esplora sync
6. Waits for VSS server readiness
7. Runs the wallet, binding, integration, receive, send and BFA tests

The integration test covers: fund wallet, sync, create UTXOs, issue NIA/IFA, two-wallet RGB send, BTC send, fee estimation, IFA inflation, drain, backup/restore, VSS backup/restore, blind/witness receive.

## Docker Compose files

### `tests/docker-compose.ci.yml`

Compose file of the whole stack, used both locally and by GitHub Actions. It holds all 10 services with their own build contexts, and `regtest.sh` runs it alone under the compose project name `rgb-lib-wasm`. The three proxies run `ghcr.io/rgb-tools/rgb-proxy-server:0.4.0` with `MAX_POW_DIFFICULTY: 0`, so the tests need no proof of work; `proxy-auth` also sets `PROXY_AUTH_PUBKEYS`.

### `tests/vss-server/Dockerfile`

Multi-stage build for the VSS server:

1. **Builder stage**: Clones [lightningdevkit/vss-server](https://github.com/lightningdevkit/vss-server) at a pinned commit, builds with `--features sigs` (signature authentication)
2. **Runtime stage**: Debian slim with just the binary and config

### `tests/vss-server/nginx-cors.conf`

Nginx config for the CORS proxy:

- Handles `OPTIONS` preflight requests (returns 204)
- Proxies `POST` requests to `vss-server:8080`
- Adds `Access-Control-Allow-Origin: *` to all responses
- Uses Docker DNS resolver (`127.0.0.11`) with runtime resolution (avoids startup failures when vss-server isn't ready yet)

## Troubleshooting

### esplora not syncing

The two bitcoind nodes (primary and esplora's internal) must be connected as peers. Check with:

```bash
docker compose exec bitcoind bitcoin-cli -regtest -rpcuser=user -rpcpassword=default_password getpeerinfo
```

If empty, manually add the peer:

```bash
docker compose exec bitcoind bitcoin-cli -regtest -rpcuser=user -rpcpassword=default_password addnode "esplora:18444" "add"
```

### VSS CORS proxy not responding

Check the nginx logs:

```bash
docker compose logs vss-cors-proxy
```

Common issue: nginx starts before `vss-server` is ready. The config uses runtime DNS resolution (`set $upstream`) to handle this — nginx will resolve the hostname on each request rather than at startup.

### Integration test timeout

Increase the timeout:

```bash
WASM_BINDGEN_TEST_TIMEOUT=600 wasm-pack test --headless --firefox --release -- --test integration_wasm
```

### Port conflicts

If ports are already in use, stop existing containers:

```bash
cd tests && bash regtest.sh stop
docker ps  # check for leftover containers
```
