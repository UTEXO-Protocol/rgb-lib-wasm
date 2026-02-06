#!/usr/bin/env bash
# One-time script: fetch async-io 2.6.0 and apply WASM Instant polyfill.
# Result: bindings/wasm/noop-deps/async-io/ (full patched crate for [patch.crates-io]).
set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
OUT_DIR="$SCRIPT_DIR/async-io"
PATCH_DIR="$SCRIPT_DIR/async-io-patch"
CRATE_URL="https://static.crates.io/crates/async-io/async-io-2.6.0.crate"
TARBALL="$SCRIPT_DIR/async-io-2.6.0.crate"

if [[ -d "$OUT_DIR" && -f "$OUT_DIR/src/time.rs" ]]; then
  echo "async-io already patched at $OUT_DIR"
  exit 0
fi

echo "Fetching async-io 2.6.0..."
if ! curl -sSLf -o "$TARBALL" "$CRATE_URL"; then
  echo "Download failed. Try manually: git clone --depth 1 --branch v2.6.0 https://github.com/smol-rs/async-io.git $OUT_DIR"
  exit 1
fi

echo "Unpacking..."
# .crate is gzip'd tar; extracts to async-io-2.6.0/ in SCRIPT_DIR
tar -xzf "$TARBALL" -C "$SCRIPT_DIR"
rm -f "$TARBALL"
EXTRACTED="$SCRIPT_DIR/async-io-2.6.0"
if [[ ! -d "$EXTRACTED" ]]; then
  # some crates.io tarballs have a single top-level dir
  EXTRACTED="$SCRIPT_DIR/$(ls "$SCRIPT_DIR" | grep -E '^async-io-' | head -1)"
fi
if [[ -d "$OUT_DIR" && "$(ls -A "$OUT_DIR" 2>/dev/null)" ]]; then
  rm -rf "$OUT_DIR"
fi
mv "$EXTRACTED" "$OUT_DIR"

echo "Applying Instant polyfill for wasm32..."
cp "$PATCH_DIR/src/time.rs" "$OUT_DIR/src/"

# Use perl for portable in-place edit and newline in replacement
perl -i -pe 's/use std::time::\{Duration, Instant\};/use std::time::Duration;\nuse crate::time::Instant;/' "$OUT_DIR/src/lib.rs"
perl -i -pe 's/^(mod driver;)/mod time;\n$1/' "$OUT_DIR/src/lib.rs"

perl -i -pe 's/use std::time::\{Duration, Instant\};/use std::time::Duration;\nuse crate::time::Instant;/' "$OUT_DIR/src/reactor.rs"
perl -i -pe 's/use std::time::\{Duration, Instant\};/use std::time::Duration;\nuse crate::time::Instant;/' "$OUT_DIR/src/driver.rs"

# Cargo.toml: add instant for wasm32
if ! grep -q 'target.*wasm32.*instant' "$OUT_DIR/Cargo.toml"; then
  # Remove self-patch section if present
  perl -i -0pe 's/\[patch\.crates-io\].*?^async-io = .*$//ms' "$OUT_DIR/Cargo.toml" 2>/dev/null || true
  cat >> "$OUT_DIR/Cargo.toml" << 'EOF'

[target.'cfg(target_arch = "wasm32")'.dependencies]
instant = { version = "0.1", features = ["wasm-bindgen"] }
EOF
fi

echo "Done. Patched async-io at $OUT_DIR"
echo "Add to root Cargo.toml [patch.crates-io]:"
echo '  async-io = { path = "bindings/wasm/noop-deps/async-io", version = "2.6.0" }'
