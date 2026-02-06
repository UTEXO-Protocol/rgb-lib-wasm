#!/usr/bin/env bash
# One-time script: fetch sqlx-core 0.8.6 and point async-io to our patched async-io-1.13.
# Result: bindings/wasm/noop-deps/sqlx-core/
set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT_DIR="$SCRIPT_DIR/sqlx-core"
CRATE_URL="https://static.crates.io/crates/sqlx-core/sqlx-core-0.8.6.crate"
TARBALL="$SCRIPT_DIR/sqlx-core-0.8.6.crate"

if [[ -d "$OUT_DIR" && -f "$OUT_DIR/Cargo.toml" ]] && grep -q 'path = "../async-io-1.13"' "$OUT_DIR/Cargo.toml"; then
  echo "sqlx-core already patched at $OUT_DIR"
  exit 0
fi

echo "Fetching sqlx-core 0.8.6..."
if ! curl -sSLf -o "$TARBALL" "$CRATE_URL"; then
  echo "Download failed."
  exit 1
fi

echo "Unpacking..."
tar -xzf "$TARBALL" -C "$SCRIPT_DIR"
rm -f "$TARBALL"
EXTRACTED="$SCRIPT_DIR/sqlx-core-0.8.6"
[[ ! -d "$EXTRACTED" ]] && EXTRACTED="$SCRIPT_DIR/$(ls "$SCRIPT_DIR" | grep -E '^sqlx-core-' | head -1)"
if [[ -d "$OUT_DIR" && "$(ls -A "$OUT_DIR" 2>/dev/null)" ]]; then
  rm -rf "$OUT_DIR"
fi
mv "$EXTRACTED" "$OUT_DIR"

echo "Patching sqlx-core to use local async-io-1.13..."
# Replace async-io dependency with path (crates.io tarball uses [dependencies.async-io] + version = "...")
if grep -q '\[dependencies.async-io\]' "$OUT_DIR/Cargo.toml"; then
  perl -i -0pe 's/\[dependencies\.async-io\]\nversion = "[^"]*"\noptional = true/[dependencies.async-io]\npath = "..\/async-io-1.13"\noptional = true/' "$OUT_DIR/Cargo.toml"
else
  perl -i -pe 's/async-io = \{ version = "[^"]*", optional = true \}/async-io = { path = "..\/async-io-1.13", optional = true }/' "$OUT_DIR/Cargo.toml"
  perl -i -pe 's/async-io = "1\.[^"]*"/async-io = { path = "..\/async-io-1.13" }/' "$OUT_DIR/Cargo.toml" 2>/dev/null || true
fi

echo "Applying Instant polyfill for wasm32 (sqlx-core uses std::time::Instant in pool/logger)..."
PATCH_DIR="$SCRIPT_DIR/sqlx-core-patch"
cp "$PATCH_DIR/src/time.rs" "$OUT_DIR/src/"
# Add mod time and instant dependency
grep -q 'mod time' "$OUT_DIR/src/lib.rs" || perl -i -pe 's/^(\#\[macro_use\])/mod time;\n\n$1/' "$OUT_DIR/src/lib.rs"
grep -q 'target.*wasm32.*instant' "$OUT_DIR/Cargo.toml" || cat >> "$OUT_DIR/Cargo.toml" << 'EOF'

[target.'cfg(target_arch = "wasm32")'.dependencies]
instant = { version = "0.1", features = ["wasm-bindgen"] }
EOF
# Replace std::time::Instant with crate::time::Instant
perl -i -pe 's/use std::time::Instant;/use crate::time::Instant;/' "$OUT_DIR/src/logger.rs"
perl -i -pe 's/use std::time::\{Duration, Instant\};/use std::time::Duration;\nuse crate::time::Instant;/' "$OUT_DIR/src/pool/connection.rs" "$OUT_DIR/src/pool/mod.rs" "$OUT_DIR/src/pool/options.rs" "$OUT_DIR/src/pool/inner.rs"

echo "Done. Patched sqlx-core at $OUT_DIR"
echo "Add to root Cargo.toml [patch.crates-io]:"
echo '  sqlx-core = { path = "bindings/wasm/noop-deps/sqlx-core", version = "0.8.6" }'
