"""Where this package is allowed to put its cache.

These tests followed ``cache_root`` out of ``core/cache.py``. The rule they
pin is about where the package may write, not about the tag database, and the
sandbox depends on it for its worktrees -- so it gets one home and the tests
sit beside it.
"""

import logging
from pathlib import Path

from agentless_mcp.util import cachedir


class TestRelativeCacheHomeIsIgnored:
    """A relative XDG_CACHE_HOME resolves against the working directory."""

    def test_a_relative_value_falls_back_to_the_default(self, monkeypatch, tmp_path, caplog):
        # Reproduced during the audit: `cd victim && XDG_CACHE_HOME=relcache
        # ... validate --repo victim` created victim/relcache/agentless-mcp/
        # worktrees inside the repository being analysed.
        monkeypatch.setattr(cachedir, "_RELATIVE_CACHE_HOMES_SEEN", set())
        monkeypatch.setenv(cachedir.ENV_CACHE_HOME, "relcache")
        monkeypatch.setattr(Path, "home", lambda: tmp_path)

        with caplog.at_level(logging.WARNING, logger=cachedir.logger.name):
            root = cachedir.cache_root()

        assert root == tmp_path / ".cache" / cachedir.APPLICATION_DIR
        assert "is relative and was ignored" in caplog.text

    def test_the_warning_is_not_repeated_for_the_same_value(self, monkeypatch, tmp_path, caplog):
        monkeypatch.setattr(cachedir, "_RELATIVE_CACHE_HOMES_SEEN", set())
        monkeypatch.setenv(cachedir.ENV_CACHE_HOME, "relcache")
        monkeypatch.setattr(Path, "home", lambda: tmp_path)

        with caplog.at_level(logging.WARNING, logger=cachedir.logger.name):
            for _ in range(5):
                cachedir.cache_root()

        assert caplog.text.count("is relative and was ignored") == 1

    def test_an_absolute_value_is_honoured(self, monkeypatch, tmp_path):
        monkeypatch.setenv(cachedir.ENV_CACHE_HOME, str(tmp_path / "elsewhere"))
        assert cachedir.cache_root() == tmp_path / "elsewhere" / cachedir.APPLICATION_DIR
