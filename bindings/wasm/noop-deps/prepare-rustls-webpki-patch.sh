#!/usr/bin/env bash
# Copies rustls-webpki 0.103.8 from Cargo registry and fixes ip_address.rs (Ipv4Addr/Ipv6Addr as_ref -> octets).
# Run from repo root. Then: cargo check -p rgb-lib

set -e
CARGO_REGISTRY="${CARGO_HOME:-$HOME/.cargo}/registry/src"
SRC=$(find "$CARGO_REGISTRY" -maxdepth 3 -type d -name "rustls-webpki-0.103.8" 2>/dev/null | head -1)
if [ -z "$SRC" ]; then
  echo "Fetching rustls-webpki first..."
  cargo fetch -p rustls-webpki 2>/dev/null || true
  SRC=$(find "$CARGO_REGISTRY" -maxdepth 3 -type d -name "rustls-webpki-0.103.8" 2>/dev/null | head -1)
fi
if [ -z "$SRC" ]; then
  echo "Could not find rustls-webpki-0.103.8 in $CARGO_REGISTRY. Run: cargo build -p rgb-lib 2>&1 | head -5 (to pull deps), then re-run this script."
  exit 1
fi
DEST="$(dirname "$0")/rustls-webpki"
rm -rf "$DEST"
cp -r "$SRC" "$DEST"
FILE="$DEST/src/subject_name/ip_address.rs"
if [ ! -f "$FILE" ]; then
  echo "Expected $FILE not found."
  exit 1
fi
# Replace ip.as_ref() with let binding + octets() for Rust < 1.77 (avoid temporary dropped borrow)
perl -i -0pe 's/IpAddr::V4\(ip\) => untrusted::Input::from\(ip\.as_ref\(\)\),/IpAddr::V4(ip) => {\n            let octets = ip.octets();\n            untrusted::Input::from(\&octets)\n        }/g; s/IpAddr::V6\(ip\) => untrusted::Input::from\(ip\.as_ref\(\)\),/IpAddr::V6(ip) => {\n            let octets = ip.octets();\n            untrusted::Input::from(\&octets)\n        }/g' "$FILE" 2>/dev/null || {
  sed -i.bak 's/ip\.as_ref()/\&ip.octets()/g' "$FILE"
  echo "Note: if build fails with 'temporary value dropped', edit $FILE: wrap each arm in { let octets = ip.octets(); untrusted::Input::from(\&octets) }"
  rm -f "$FILE.bak"
}
echo "Patched rustls-webpki at $DEST. Run: cargo check -p rgb-lib"
grep -n 'as_ref\|octets' "$FILE" || true
