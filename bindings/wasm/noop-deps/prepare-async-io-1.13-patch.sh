#!/usr/bin/env bash
# One-time script: fetch async-io 1.13.0 and apply WASM Instant polyfill.
# sqlx-core depends on async-io 1.13.0; we patch it so WASM doesn't panic on Instant.
# Result: bindings/wasm/noop-deps/async-io-1.13/
set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT_DIR="$SCRIPT_DIR/async-io-1.13"
PATCH_DIR="$SCRIPT_DIR/async-io-patch"
CRATE_URL="https://static.crates.io/crates/async-io/async-io-1.13.0.crate"
TARBALL="$SCRIPT_DIR/async-io-1.13.0.crate"

if [[ -d "$OUT_DIR" && -f "$OUT_DIR/src/time.rs" ]]; then
  echo "async-io 1.13 already patched at $OUT_DIR"
  exit 0
fi

echo "Fetching async-io 1.13.0..."
if ! curl -sSLf -o "$TARBALL" "$CRATE_URL"; then
  echo "Download failed. Try: git clone --depth 1 --branch v1.13.0 https://github.com/smol-rs/async-io.git $OUT_DIR"
  exit 1
fi

echo "Unpacking..."
tar -xzf "$TARBALL" -C "$SCRIPT_DIR"
rm -f "$TARBALL"
EXTRACTED="$SCRIPT_DIR/async-io-1.13.0"
[[ ! -d "$EXTRACTED" ]] && EXTRACTED="$SCRIPT_DIR/$(ls "$SCRIPT_DIR" | grep -E '^async-io-' | head -1)"
if [[ -d "$OUT_DIR" && "$(ls -A "$OUT_DIR" 2>/dev/null)" ]]; then
  rm -rf "$OUT_DIR"
fi
mv "$EXTRACTED" "$OUT_DIR"

echo "Applying Instant polyfill for wasm32..."
cp "$PATCH_DIR/src/time.rs" "$OUT_DIR/src/"

# Only replace the FIRST occurrence (top-level use), not those in doc comments.
perl -i -pe 's/use std::time::\{Duration, Instant\};/use std::time::Duration;\nuse crate::time::Instant;/ if !$done++ && $. < 100' "$OUT_DIR/src/lib.rs"
perl -i -pe 's/^(mod driver;)/mod time;\n$1/' "$OUT_DIR/src/lib.rs"

# async-io 1.13 has reactor.rs and driver.rs
if [[ -f "$OUT_DIR/src/reactor.rs" ]]; then
  perl -i -pe 's/use std::time::\{Duration, Instant\};/use std::time::Duration;\nuse crate::time::Instant;/' "$OUT_DIR/src/reactor.rs"
fi
if [[ -f "$OUT_DIR/src/driver.rs" ]]; then
  perl -i -pe 's/use std::time::\{Duration, Instant\};/use std::time::Duration;\nuse crate::time::Instant;/' "$OUT_DIR/src/driver.rs"
fi

if ! grep -q 'target.*wasm32.*instant' "$OUT_DIR/Cargo.toml"; then
  perl -i -0pe 's/\[patch\.crates-io\].*?^async-io = .*$//ms' "$OUT_DIR/Cargo.toml" 2>/dev/null || true
  cat >> "$OUT_DIR/Cargo.toml" << 'EOF'

[target.'cfg(target_arch = "wasm32")'.dependencies]
instant = { version = "0.1", features = ["wasm-bindgen"] }
EOF
fi

echo "Done. Patched async-io 1.13 at $OUT_DIR"
echo "Next: run prepare-sqlx-core-patch.sh so sqlx-core uses this crate."
