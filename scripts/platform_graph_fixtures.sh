#!/usr/bin/env bash
# Rewrite crates/graph/fixtures with the Python reference on this platform, then show what changed.
# The committed fixtures come from Linux. The reference is the Python build at tag v0.11.0rc1, the
# last tag that holds it. $1 is a scratch directory for the tag's clone, the reference's venv,
# caches and grammar libraries. The generator does not download, so warmup fills the libraries
# first.
set -euo pipefail

tag=v0.11.0rc1
reference="$1"
root="$(cd "$(dirname "$0")/.." && pwd)"
tree="$reference/tree"
grammars="$reference/grammars/libs"
mkdir -p "$reference/home" "$reference/cache" "$grammars"
# From origin: a shallow checkout holds no tags.
git -c advice.detachedHead=false clone --quiet --depth 1 --branch "$tag" \
    "$(git -C "$root" remote get-url origin)" "$tree"
# The tag's generator reproduces only the fixtures that the tag holds.
if ! diff -r -q "$tree/crates/graph/fixtures" "$root/crates/graph/fixtures" >&2; then
    echo "crates/graph/fixtures differ from $tag, so its reference cannot rewrite them" >&2
    exit 1
fi
cd "$tree"
UV_PROJECT_ENVIRONMENT="$reference/venv" uv sync --frozen --no-dev
scratch=(env HOME="$reference/home" XDG_CACHE_HOME="$reference/cache"
    TREE_SITTER_LANGUAGE_PACK_CACHE_DIR="$grammars" PYTHONDONTWRITEBYTECODE=1)
"${scratch[@]}" "$reference/venv/bin/agentless-mcp" warmup
"${scratch[@]}" "$reference/venv/bin/python" scripts/graph_fixtures.py
cp -R "$tree/crates/graph/fixtures/." "$root/crates/graph/fixtures/"
git -C "$root" diff --stat -- crates/graph/fixtures
