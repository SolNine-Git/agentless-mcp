#!/usr/bin/env bash
# Rewrite crates/graph/fixtures with the Python reference on this platform, then show what changed.
# The committed fixtures come from Linux. $1 is a scratch directory for the reference's venv and caches.
set -euo pipefail

reference="$1"
root="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$reference/home" "$reference/cache"
cd "$root"
UV_PROJECT_ENVIRONMENT="$reference/venv" uv sync --frozen --no-dev
HOME="$reference/home" XDG_CACHE_HOME="$reference/cache" PYTHONDONTWRITEBYTECODE=1 \
    "$reference/venv/bin/python" scripts/graph_fixtures.py
git diff --stat -- crates/graph/fixtures
