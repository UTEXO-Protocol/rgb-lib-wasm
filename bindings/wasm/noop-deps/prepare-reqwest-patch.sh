#!/usr/bin/env bash
# One-time script: fetch reqwest 0.12.24 and apply WASM Instant polyfill.
# Result: bindings/wasm/noop-deps/reqwest/ (full patched crate for [patch.crates-io]).
# Reqwest wasm code path (src/wasm/) uses only std::time::Duration; Instant is used
# in blocking/ and async_impl/h3_client/ (not compiled for wasm32). This patch adds
# a time module and instant dependency for wasm32 so that any future or transitive
# use of Instant in reqwest wasm can use crate::time::Instant.
set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT_DIR="$SCRIPT_DIR/reqwest"
PATCH_DIR="$SCRIPT_DIR/reqwest-patch"
REQWEST_VERSION="0.12.24"
CRATE_URL="https://static.crates.io/crates/reqwest/reqwest-${REQWEST_VERSION}.crate"
TARBALL="$SCRIPT_DIR/reqwest-${REQWEST_VERSION}.crate"

if [[ -d "$OUT_DIR" && -f "$OUT_DIR/src/time.rs" ]]; then
  echo "reqwest already patched at $OUT_DIR"
  exit 0
fi

echo "Fetching reqwest ${REQWEST_VERSION}..."
if ! curl -sSLf -o "$TARBALL" "$CRATE_URL"; then
  echo "Download failed. Try manually: curl -L -o $TARBALL $CRATE_URL"
  exit 1
fi

echo "Unpacking..."
tar -xzf "$TARBALL" -C "$SCRIPT_DIR"
rm -f "$TARBALL"
EXTRACTED="$SCRIPT_DIR/reqwest-${REQWEST_VERSION}"
if [[ ! -d "$EXTRACTED" ]]; then
  EXTRACTED="$SCRIPT_DIR/$(ls "$SCRIPT_DIR" | grep -E '^reqwest-' | head -1)"
fi
if [[ -d "$OUT_DIR" && "$(ls -A "$OUT_DIR" 2>/dev/null)" ]]; then
  rm -rf "$OUT_DIR"
fi
mv "$EXTRACTED" "$OUT_DIR"

echo "Files in reqwest that use Instant or std::time (for reference):"
grep -rln 'Instant\|std::time' "$OUT_DIR/src" --include='*.rs' 2>/dev/null || true

echo "Applying Instant polyfill for wasm32..."
mkdir -p "$OUT_DIR/src"
cp "$PATCH_DIR/src/time.rs" "$OUT_DIR/src/"

# Add mod time in lib.rs inside if_wasm! { ... } (right after "mod wasm;")
if ! grep -q 'mod time;' "$OUT_DIR/src/lib.rs"; then
  perl -i -pe 's/^(\s*mod wasm;)$/    mod time;\n$1/' "$OUT_DIR/src/lib.rs"
fi

# Cargo.toml: add instant for wasm32 (after web-sys block)
if ! grep -q "target.*wasm32.*instant" "$OUT_DIR/Cargo.toml"; then
  # Insert after "RequestCache",] and newline, before dev-dependencies
  perl -i -0pe 's/("RequestCache",\n]\n)(\n\[target\.)/$1\n[target.\x27cfg(target_arch = "wasm32")\x27.dependencies.instant]\nversion = "0.1"\nfeatures = ["wasm-bindgen"]\n\n$2/' "$OUT_DIR/Cargo.toml"
fi

echo "Done. Patched reqwest at $OUT_DIR"
echo "Add to root Cargo.toml [patch.crates-io]:"
echo "  reqwest = { path = \"bindings/wasm/noop-deps/reqwest\", version = \"${REQWEST_VERSION}\" }"
