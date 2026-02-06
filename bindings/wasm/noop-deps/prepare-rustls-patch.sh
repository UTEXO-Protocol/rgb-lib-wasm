#!/usr/bin/env bash
# Copies rustls from Cargo registry and adds use rustls_pki_types::{IntoOwned,ToOwned} so it
# compiles with our patched rustls-pki-types. Run from repo root.

set -e
CARGO_REGISTRY="${CARGO_HOME:-$HOME/.cargo}/registry/src"
# Find rustls 0.23.x (match version in Cargo.lock)
SRC=$(find "$CARGO_REGISTRY" -maxdepth 3 -type d -name "rustls-0.23*" 2>/dev/null | head -1)
if [ -z "$SRC" ]; then
  echo "Fetching rustls first..."
  cargo fetch -p rustls 2>/dev/null || true
  SRC=$(find "$CARGO_REGISTRY" -maxdepth 3 -type d -name "rustls-0.23*" 2>/dev/null | head -1)
fi
if [ -z "$SRC" ]; then
  echo "Could not find rustls in $CARGO_REGISTRY. Run: cargo build -p rgb-lib 2>&1 | head -5, then re-run this script."
  exit 1
fi
DEST="$(dirname "$0")/rustls"
rm -rf "$DEST"
cp -r "$SRC" "$DEST"

# Add use of IntoOwned/ToOwned (compiler suggests rustls_pki_types; rustls may alias as pki_types)
HANDSHAKE="$DEST/src/msgs/handshake.rs"
if [ -f "$HANDSHAKE" ]; then
  if ! grep -q 'IntoOwned\|ToOwned' "$HANDSHAKE"; then
    # Insert after first "use " line (handshake uses pki_types or rustls_pki_types)
    if grep -q '^use pki_types::' "$HANDSHAKE"; then
      awk '!inserted && /^use pki_types::/ { print; print "use pki_types::{IntoOwned, ToOwned};"; inserted=1; next } 1' "$HANDSHAKE" > "$HANDSHAKE.tmp" && mv "$HANDSHAKE.tmp" "$HANDSHAKE"
    else
      sed -i.bak '1,/^use /{/^use /a\
use rustls_pki_types::{IntoOwned, ToOwned};
}' "$HANDSHAKE" 2>/dev/null || awk '!inserted && /^use / { print; print "use rustls_pki_types::{IntoOwned, ToOwned};"; inserted=1; next } 1' "$HANDSHAKE" > "$HANDSHAKE.tmp" && mv "$HANDSHAKE.tmp" "$HANDSHAKE"
    fi
    rm -f "$HANDSHAKE.bak"
  fi
  # Fix E0282: dns_name.as_ref().as_ref() -> .as_bytes() for &str to &[u8]
  if grep -q 'dns_name\.as_ref()\.as_ref()' "$HANDSHAKE"; then
    sed -i.bak 's/dns_name\.as_ref()\.as_ref()/dns_name.as_ref().as_bytes()/g' "$HANDSHAKE" 2>/dev/null || true
    rm -f "$HANDSHAKE.bak"
  fi
fi

ANCHORS="$DEST/src/webpki/anchors.rs"
if [ -f "$ANCHORS" ]; then
  if ! grep -q 'ToOwned' "$ANCHORS"; then
    if grep -q '^use pki_types::' "$ANCHORS"; then
      awk '!inserted && /^use / { print; print "use pki_types::ToOwned;"; inserted=1; next } 1' "$ANCHORS" > "$ANCHORS.tmp" && mv "$ANCHORS.tmp" "$ANCHORS"
    else
      awk '!inserted && /^use / { print; print "use rustls_pki_types::ToOwned;"; inserted=1; next } 1' "$ANCHORS" > "$ANCHORS.tmp" && mv "$ANCHORS.tmp" "$ANCHORS"
    fi
  fi
fi

echo "Patched rustls at $DEST. Run: cargo check -p rgb-lib"
