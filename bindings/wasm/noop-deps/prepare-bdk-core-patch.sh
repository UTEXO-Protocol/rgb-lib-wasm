#!/usr/bin/env bash
# One-time script: fetch bdk_core 0.6.0 and patch SyncRequest::builder / FullScanRequest::builder
# to avoid std::time on wasm32 (panics with "time not implemented on this platform").
# Result: bindings/wasm/noop-deps/bdk_core/ for [patch.crates-io].
set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT_DIR="$SCRIPT_DIR/bdk_core"
VERSION="0.6.0"
CRATE_URL="https://static.crates.io/crates/bdk_core/bdk_core-${VERSION}.crate"
TARBALL="$SCRIPT_DIR/bdk_core-${VERSION}.crate"

if [[ -d "$OUT_DIR" && -f "$OUT_DIR/src/spk_client.rs" ]] && grep -q 'target_arch = "wasm32"' "$OUT_DIR/src/spk_client.rs" 2>/dev/null; then
  echo "bdk_core already patched at $OUT_DIR"
  exit 0
fi

echo "Fetching bdk_core ${VERSION}..."
curl -sSLf -o "$TARBALL" "$CRATE_URL" || { echo "Download failed."; exit 1; }

echo "Unpacking..."
tar -xzf "$TARBALL" -C "$SCRIPT_DIR"
rm -f "$TARBALL"
EXTRACTED="$SCRIPT_DIR/bdk_core-${VERSION}"
if [[ ! -d "$EXTRACTED" ]]; then
  EXTRACTED=$(ls -d "$SCRIPT_DIR"/bdk_core-* 2>/dev/null | head -1)
fi
[[ -d "$OUT_DIR" ]] && rm -rf "$OUT_DIR"
mv "$EXTRACTED" "$OUT_DIR"

# Remove dev-dependencies so the crate builds without bdk repo
sed -i.bak '/^\[dev-dependencies\]/,/^\[/{
  /^\[dev-dependencies\]/d
  /^\[/!d
}' "$OUT_DIR/Cargo.toml" 2>/dev/null || true
rm -f "$OUT_DIR/Cargo.toml.bak"

SPK="$OUT_DIR/src/spk_client.rs"

# Patch: on wasm32 use builder_at(0) instead of std::time (SyncRequest and FullScanRequest).
# Pattern in bdk_core: start_time = std::time::UNIX_EPOCH .elapsed() or .duration_since
if ! grep -q 'target_arch = "wasm32"' "$SPK"; then
  echo "Patching spk_client.rs for wasm32..."
  # Replace "let start_time = std::time::UNIX_EPOCH" block with cfg
  # First SyncRequest::builder
  sed -i.bak 's/let start_time = std::time::UNIX_EPOCH$/#[cfg(not(target_arch = "wasm32"))]\n        let start_time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)/' "$SPK"
  # .elapsed() -> .unwrap() for the duration_since line (only first occurrence per block is tricky)
  # Simpler: replace the whole paragraph. We need to match the exact bdk source.
  # bdk might use: UNIX_EPOCH .elapsed() - so it's (something).elapsed(). Try replacing.
  sed -i.bak 's/\.elapsed()/.unwrap()/g' "$SPK"
  # Add wasm32 branch: after "let start_time = ... as_secs();" we need "#[cfg(target_arch = "wasm32")] let start_time = 0u64;"
  # Actually the clean approach: replace the 4-line block
  #   let start_time = ...
  #   .elapsed() or .duration_since(...)
  #   .expect(...)
  #   .as_secs();
  # with two cfg blocks.
  python3 << PYEOF
path = "$SPK"
with open(path, "r") as f:
    content = f.read()

# SyncRequest::builder - builder() -> SyncRequestBuilder<()>
old_sync = """    pub fn builder() -> SyncRequestBuilder<()> {
        let start_time = std::time::UNIX_EPOCH
            .elapsed()
            .expect("failed to get current timestamp")
            .as_secs();
        Self::builder_at(start_time)
    }"""

new_sync = """    pub fn builder() -> SyncRequestBuilder<()> {
        #[cfg(not(target_arch = "wasm32"))]
        let start_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("failed to get current timestamp")
            .as_secs();
        #[cfg(target_arch = "wasm32")]
        let start_time = 0u64;
        Self::builder_at(start_time)
    }"""

# FullScanRequest::builder
old_full = """    pub fn builder() -> FullScanRequestBuilder<K> {
        let start_time = std::time::UNIX_EPOCH
            .elapsed()
            .expect("failed to get current timestamp")
            .as_secs();
        Self::builder_at(start_time)
    }"""

new_full = """    pub fn builder() -> FullScanRequestBuilder<K> {
        #[cfg(not(target_arch = "wasm32"))]
        let start_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("failed to get current timestamp")
            .as_secs();
        #[cfg(target_arch = "wasm32")]
        let start_time = 0u64;
        Self::builder_at(start_time)
    }"""

# Revert the sed .elapsed() -> .unwrap() if we're doing full replace
content = content.replace(".unwrap()", ".elapsed()", 2)  # in case sed already ran
if "SyncRequestBuilder<()>" in content and "target_arch" not in content:
    content = content.replace(old_sync, new_sync, 1)
if "FullScanRequestBuilder<K>" in content and content.count("target_arch") < 2:
    content = content.replace(old_full, new_full, 1)
with open(path, "w") as f:
    f.write(content)
PYEOF
fi

# If python replace didn't find (whitespace), try alternate
if ! grep -q 'target_arch = "wasm32"' "$SPK" 2>/dev/null; then
  # Try with single-space/compact
  sed -i.bak 's/let start_time = std::time::UNIX_EPOCH$/#[cfg(not(target_arch = "wasm32"))]\n        let _start = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("ts").as_secs();\n        #[cfg(target_arch = "wasm32")]\n        let _start = 0u64;\n        let start_time = _start;/' "$SPK"
fi

rm -f "$SPK.bak" 2>/dev/null
echo "Done. bdk_core at $OUT_DIR"
echo "Ensure Cargo.toml has: bdk_core = { path = \"bindings/wasm/noop-deps/bdk_core\", version = \"0.6\" }"
