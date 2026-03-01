# Patches for building rgb-lib with no-op dependencies (WASM)

## Why patches: what is missing in wasm32

WASM build targets the **browser** (or Node). There are no OS APIs that standard Rust relies on:

| Missing in wasm32 | What breaks without a patch | Workaround |
|-------------------|-----------------------------|------------|
| **std::time** (SystemTime, Instant) | Panic "time not implemented on this platform" | Replace with `instant::Instant` (JS `Date.now()`) or fixed value (e.g. `builder_at(0)`) in **async-io**, **async-std**, **reqwest**, **bdk_core** patches |
| **File system** | Expects home dir path, DB on disk | **home** — no-op (no home dir); DB — in-memory (`:memory:`) |
| **Sockets / blocking network** | Minreq/blocking HTTP → "operation not supported" | Network only via **fetch** (gloo_net, reqwest wasm); blocking Esplora not used for broadcast/sync in browser — async + fetch only |
| **errno, polling, rustix, socket2** | Unix/Windows code does not compile or panics | **no-op** versions of these crates (stubs for wasm32) |
| **New rustls-webpki API** | Older Rust or type mismatch (Ipv4Addr/Ipv6Addr) | **rustls-webpki** — patch for current compiler |
| **rustls / pki_types** | Type mismatch (IntoOwned, ToOwned, as_bytes()) | **rustls**, **rustls-pki-types** — compatibility fixes and UnixTime for WASM |

Summary: **WASM does not "provide"** time, sockets, or file system. Patches either supply an implementation via JS (instant), or disable/stub code (no-op), or fix API incompatibilities.

---

Patches are **code in the repo**, not scripts. `[patch.crates-io]` points to local directories with already-patched crates.

## async-io (Instant on wasm32)

On wasm32 `std::time::Instant::now()` panics ("time not implemented on this platform"). async-io (via sea-orm/sqlx) uses Instant. The patch replaces it with `instant::Instant` when building for wasm32.

**Two async-io versions in the tree:** async-io 2.6.0 (async-std) and async-io 1.13.0 (sqlx-core). Both patches are needed. The patched crates are in `async-io/` and `async-io-1.13/`; sqlx-core uses the latter. Patches are already enabled in the root `Cargo.toml`.

## rustls

Patched rustls lives in `bindings/wasm/noop-deps/rustls/` (adds `use pki_types::{IntoOwned, ToOwned}` and `dns_name.as_ref().as_ref()` → `.as_bytes()`). The patch is enabled in `Cargo.toml`. No scripts required.

## rustls-webpki

Patched rustls-webpki in `rustls-webpki/` (fixes `ip_address.rs` for Rust &lt; 1.77). The directory is already in the repo.

---

## Summary per patch (Cargo.toml [patch.crates-io])

| Crate | Purpose |
|-------|---------|
| **errno** | No-op for wasm32 (no OS errno). |
| **polling** | No-op for wasm32 (no epoll/kqueue). |
| **rustix** | No-op/stub for wasm32 (no POSIX). |
| **rustls-pki-types** | UnixTime::now() and WASM compatibility. |
| **home** | No-op: no home dir in browser. |
| **socket2** | No-op: no sockets in WASM. |
| **rustls-webpki** | Ipv4Addr/Ipv6Addr fix for Rust < 1.77. |
| **rustls** | IntoOwned/ToOwned and dns_name.as_bytes() for build. |
| **async-io** (2.6 and 1.13) | Instant → `instant::Instant` on wasm32. |
| **sqlx-core** | Uses our async-io 1.13 instead of std time. |
| **async-std** | Instant on wasm32 via instant. |
| **reqwest** | Instant polyfill for wasm32. |
| **bdk_core** | SyncRequest/FullScanRequest without std::time (builder_at(0) on wasm32). |
