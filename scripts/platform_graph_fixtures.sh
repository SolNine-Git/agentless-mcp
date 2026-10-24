#!/usr/bin/env bash
# Rewrite crates/graph/fixtures with the Python reference on this platform, then show what changed.
# The committed fixtures come from Linux. $1 is a scratch directory for the reference's venv, caches
# and grammar libraries. The generator does not download, so warmup fills the libraries first.
set -euo pipefail

reference="$1"
root="$(cd "$(dirname "$0")/.." && pwd)"
grammars="$reference/grammars/libs"
mkdir -p "$reference/home" "$reference/cache" "$grammars"
cd "$root"
UV_PROJECT_ENVIRONMENT="$reference/venv" uv sync --frozen --no-dev
scratch=(env HOME="$reference/home" XDG_CACHE_HOME="$reference/cache"
    TREE_SITTER_LANGUAGE_PACK_CACHE_DIR="$grammars" PYTHONDONTWRITEBYTECODE=1)
"${scratch[@]}" "$reference/venv/bin/agentless-mcp" warmup
"${scratch[@]}" "$reference/venv/bin/python" scripts/graph_fixtures.py
git diff --stat -- crates/graph/fixtures
