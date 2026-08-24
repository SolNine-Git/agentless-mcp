"""Where this package's cache directory lives, and why the answer is bounded.

A leaf module rather than a corner of ``core/cache.py``, because two callers
need this and only one of them is the tag database. ``core/sandbox.py`` puts
its worktrees under the same root, and importing ``core.cache`` for three
constants dragged tree-sitter, the language pack and ``sqlite3`` into the
write side: 181 modules and 38.8 ms in a fresh interpreter, for a module that
parses nothing.

Copying ``cache_root`` into the sandbox was the other option and is worse. The
relative-``XDG_CACHE_HOME`` refusal below is a rule about where this package
may write, not an implementation detail of the tag cache, and a rule with two
homes drifts apart silently.
"""

from __future__ import annotations

import logging
import os
from pathlib import Path

ENV_CACHE_HOME = "XDG_CACHE_HOME"
APPLICATION_DIR = "agentless-mcp"

# Directories under the user's cache home hold derived facts about private
# repositories, so they are owner-only.
DIRECTORY_MODE = 0o700

logger = logging.getLogger(__name__)


def cache_root() -> Path:
    """Return the directory holding every repository's cache, per XDG.

    A relative ``XDG_CACHE_HOME`` is ignored, which is what the XDG base
    directory specification requires -- "if an implementation encounters a
    relative path it must consider the value invalid" -- and which this
    package needs for a reason of its own. A relative value resolves against
    the current working directory, and the working directory during a
    ``validate`` run is the repository being analysed. Reproduced:
    ``cd victim && XDG_CACHE_HOME=relcache agentless-mcp validate --repo
    victim`` created ``victim/relcache/agentless-mcp/worktrees`` inside the
    repository under analysis. It also made the cache location depend on
    where each call happened to be standing, so two calls in one process
    could read two different databases.
    """
    configured = os.environ.get(ENV_CACHE_HOME, "").strip()
    if configured and not Path(configured).is_absolute():
        _warn_once_about_relative_cache_home(configured)
        configured = ""
    home = Path(configured) if configured else Path.home() / ".cache"
    return home / APPLICATION_DIR


_RELATIVE_CACHE_HOMES_SEEN: set[str] = set()


def _warn_once_about_relative_cache_home(value: str) -> None:
    """Say why the environment was ignored, once per distinct value.

    Once rather than per call: ``cache_root`` runs on every cached read, and a
    warning per read would bury the answer it is attached to.
    """
    if value in _RELATIVE_CACHE_HOMES_SEEN:
        return
    _RELATIVE_CACHE_HOMES_SEEN.add(value)
    logger.warning(
        "%s=%r is relative and was ignored; the XDG specification requires an absolute "
        "path, and a relative one would put the cache inside whichever directory the "
        "call was made from -- including the repository being analysed",
        ENV_CACHE_HOME,
        value,
    )
