#!/usr/bin/env bash
# Write packaging/notices/, which the wheel and the sdist ship: the project license and notice, the
# grammar licenses from the vendored archive, and the crate notices from cargo-about.
# It needs GNU tar with zstd, cargo-about, and every crate of Cargo.lock in the cargo cache.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
vendor="$root/crates/syntax/vendor"
out="$root/packaging/notices"
archive="$(sed -n 's/^archive = "\(.*\)"$/\1/p' "$vendor/MANIFEST.toml")"
test -n "$archive"

rm -rf "$out"
mkdir -p "$out/grammars"
cp "$root/LICENSE" "$root/NOTICE" "$out/"
cp "$vendor/MANIFEST.toml" "$out/grammars/"
tar --zstd -xf "$vendor/$archive" -C "$out/grammars" licenses shim/utf8proc/LICENSE.md
cargo about generate --frozen --fail --threshold 0.9 \
    --config "$root/packaging/about.toml" \
    --manifest-path "$root/crates/agentless/Cargo.toml" \
    --output-file "$out/THIRD-PARTY-CRATES.md" \
    "$root/packaging/about.hbs"
