"""Tests for grammar loading, warmed-state gating and degradation.

Nothing here reaches the network. The tests that need a real grammar read the
one the session-scoped ``warm_grammars`` fixture warmed, which is where the
suite's single download can happen; those assertions therefore rest on what
that fixture arranged, not on this file. Everywhere else the warmed-state
probe is monkeypatched so the "not warmed" and "no download" paths do not
depend on what this machine happens to have cached, and the cold-cache warm
test poisons the pack's manifest URL so any attempted fetch fails instantly.
"""

import logging
import os
import subprocess
import sys
import threading
import time

import pytest
import tree_sitter_language_pack as pack

from agentless_mcp.core import grammars
from agentless_mcp.util.errors import AtlasError, LanguageUnavailable


class TestGetLanguage:
    def test_warmed_language_loads(self):
        language = grammars.get_language("python")
        assert language.abi_version >= 13

    def test_each_caller_gets_its_own_parser(self):
        # Not memoized, and this test is the reason stated where a reader
        # will look for it. A Parser carries mutable state across parse
        # calls and four call sites reach it while the background index
        # thread runs, so a shared instance is one object driven by two
        # threads. Sharing was safe only because py-tree-sitter 0.26 holds
        # the GIL across parse, which the version pin does not promise.
        assert grammars.get_parser("python") is not grammars.get_parser("python")

    def test_the_grammar_behind_them_is_still_shared(self):
        # The expensive half is the Language, and it is immutable. Dropping
        # the parser memo must not turn every parse into a grammar load.
        assert grammars.get_parser("python").language == grammars.get_parser("python").language

    def test_unknown_language_is_named_as_unknown(self):
        with pytest.raises(LanguageUnavailable) as caught:
            grammars.get_language("not-a-real-language")
        assert "unknown language 'not-a-real-language'" in str(caught.value)

    def test_unwarmed_language_reports_the_remediation(self, monkeypatch):
        monkeypatch.setattr(pack, "downloaded_languages", lambda: ["python"])
        with pytest.raises(LanguageUnavailable) as caught:
            grammars.get_language("go")
        assert str(caught.value) == "language 'go' not warmed: run agentless-mcp warmup"


class TestWarmup:
    def test_no_download_flag_refuses_to_fetch(self, monkeypatch):
        monkeypatch.setattr(pack, "downloaded_languages", list)
        with pytest.raises(AtlasError) as caught:
            grammars.warmup(["python"], no_download=True)
        message = str(caught.value)
        assert "refusing to download grammar 'python'" in message
        assert grammars.ENV_NO_DOWNLOAD in message

    def test_no_download_environment_variable_refuses_to_fetch(self, monkeypatch):
        monkeypatch.setattr(pack, "downloaded_languages", list)
        monkeypatch.setenv(grammars.ENV_NO_DOWNLOAD, "1")
        with pytest.raises(AtlasError, match="refusing to download grammar"):
            grammars.warmup(["python"])

    def test_already_warmed_languages_do_not_need_a_download(self, monkeypatch):
        monkeypatch.setenv(grammars.ENV_NO_DOWNLOAD, "1")
        report = grammars.warmup(["python"])
        assert report.ok
        assert [cap.name for cap in report.languages] == ["python"]

    def test_report_carries_versions_and_cache_dir(self):
        report = grammars.warmup(["python"])
        assert report.pack_version == grammars.pack_version()
        assert report.cache_dir == grammars.cache_dir()
        assert report.languages[0].probe_ok is True

    def test_fetch_failure_degrades_one_language(self, monkeypatch):
        monkeypatch.setattr(pack, "downloaded_languages", lambda: ["python"])

        def explode(languages):
            message = f"no bundle for {languages}"
            raise pack.DownloadError(message)

        monkeypatch.setattr(pack, "prefetch", explode)

        report = grammars.warmup(["python", "go"])
        assert report.ok is False
        degraded = {cap.name: cap for cap in report.degraded}
        assert set(degraded) == {"go"}
        assert degraded["go"].detail.startswith("fetch failed: ")


class TestCapabilities:
    def test_warmed_language_is_reported_warmed(self):
        capabilities = {cap.name: cap for cap in grammars.loaded_capabilities(["python"])}
        python = capabilities["python"]
        assert python.warmed is True
        assert python.probe_ok is True
        assert python.abi_version is not None
        assert python.pack_version == grammars.pack_version()

    def test_unwarmed_language_is_reported_with_remediation(self, monkeypatch):
        monkeypatch.setattr(pack, "downloaded_languages", list)
        (capability,) = grammars.loaded_capabilities(["go"])
        assert capability.warmed is False
        assert capability.probe_ok is False
        assert capability.detail == "not warmed: run agentless-mcp warmup go"

    def test_default_capability_list_covers_both_tiers(self, monkeypatch):
        monkeypatch.setattr(pack, "downloaded_languages", list)
        names = [cap.name for cap in grammars.loaded_capabilities()]
        assert names == list(grammars.ALL_LANGUAGES)

    def test_every_language_is_labelled_with_its_tier(self, monkeypatch):
        monkeypatch.setattr(pack, "downloaded_languages", list)
        tiers = {cap.name: cap.tier for cap in grammars.loaded_capabilities()}
        assert tiers["python"] == 1
        assert tiers["kotlin"] == 2
        assert {tiers[name] for name in grammars.TIER1_LANGUAGES} == {1}
        assert {tiers[name] for name in grammars.TIER2_LANGUAGES} == {2}


@pytest.fixture
def auto_warm_isolated(monkeypatch):
    """Fresh per-process warm state with both opt-outs cleared."""
    monkeypatch.delenv(grammars.ENV_NO_AUTO_WARM, raising=False)
    monkeypatch.delenv(grammars.ENV_NO_DOWNLOAD, raising=False)
    monkeypatch.setattr(grammars, "_AUTO_WARM", grammars._AutoWarmState())


class TestAutoWarm:
    """Issue #19: the background warm at process start."""

    def test_the_caller_is_served_before_the_warm_completes(self, monkeypatch, auto_warm_isolated):
        release = threading.Event()
        calls: list[tuple[str, ...]] = []

        def slow_warmup(languages, *, no_download=False):
            calls.append(tuple(languages))
            release.wait(timeout=5)
            return grammars.WarmupReport(cache_dir="c", pack_version="p", languages=())

        monkeypatch.setattr(grammars, "warmup", slow_warmup)
        monkeypatch.setattr(grammars, "warmed_languages", frozenset)

        thread = grammars.start_auto_warm(["json"])
        assert thread is not None
        # The start returned while the warm is still blocked: the process
        # serves first and the warm lands later.
        assert thread.is_alive()
        # A second start is a no-op on the same thread: exactly one warm.
        assert grammars.start_auto_warm(["json"]) is thread
        release.set()
        thread.join(timeout=5)
        assert not thread.is_alive()
        assert calls == [("json",)]

    def test_no_download_has_absolute_priority(self, monkeypatch, auto_warm_isolated):
        monkeypatch.setenv(grammars.ENV_NO_DOWNLOAD, "1")
        monkeypatch.setattr(grammars, "warmup", self._must_not_warm)
        assert grammars.start_auto_warm(["json"]) is None
        assert grammars.auto_warm_in_progress() is False

    def test_the_environment_opt_out_keeps_the_warm_off(self, monkeypatch, auto_warm_isolated):
        monkeypatch.setenv(grammars.ENV_NO_AUTO_WARM, "1")
        monkeypatch.setattr(grammars, "warmup", self._must_not_warm)
        assert grammars.start_auto_warm(["json"]) is None

    def test_nothing_cold_means_no_thread(self, monkeypatch, auto_warm_isolated):
        monkeypatch.setattr(grammars, "warmed_languages", lambda: frozenset(["json"]))
        monkeypatch.setattr(grammars, "warmup", self._must_not_warm)
        assert grammars.start_auto_warm(["json"]) is None

    def test_a_failed_warm_logs_one_line_and_changes_nothing(
        self, monkeypatch, auto_warm_isolated, caplog
    ):
        def exploding(languages, *, no_download=False):
            message = "boom"
            raise RuntimeError(message)

        monkeypatch.setattr(grammars, "warmup", exploding)
        monkeypatch.setattr(grammars, "warmed_languages", frozenset)

        with caplog.at_level(logging.WARNING, logger="agentless_mcp.core.grammars"):
            thread = grammars.start_auto_warm(["json"])
            assert thread is not None
            thread.join(timeout=5)
        assert "background grammar warm failed: boom" in caplog.text

        # Behavior is unchanged afterwards: the labeled skip carries exactly
        # today's remediation once the warm is no longer in progress.
        monkeypatch.setattr(pack, "downloaded_languages", list)
        with pytest.raises(LanguageUnavailable) as caught:
            grammars.get_language("go")
        assert str(caught.value) == "language 'go' not warmed: run agentless-mcp warmup"

    def test_the_skip_reason_names_the_warm_while_it_runs(self, monkeypatch, auto_warm_isolated):
        release = threading.Event()

        def slow_warmup(languages, *, no_download=False):
            release.wait(timeout=5)
            return grammars.WarmupReport(cache_dir="c", pack_version="p", languages=())

        monkeypatch.setattr(grammars, "warmup", slow_warmup)
        monkeypatch.setattr(grammars, "warmed_languages", frozenset)
        monkeypatch.setattr(pack, "downloaded_languages", list)

        thread = grammars.start_auto_warm(["json"])
        assert thread is not None
        try:
            with pytest.raises(LanguageUnavailable) as caught:
                grammars.get_language("go")
            assert "a background warm is in progress" in str(caught.value)
        finally:
            release.set()
            thread.join(timeout=5)

    @staticmethod
    def _must_not_warm(languages, *, no_download=False):
        pytest.fail("the background warm must not run here")


class TestColdCacheOffline:
    def test_a_cold_cache_without_network_degrades_instead_of_crashing(self, tmp_path):
        """The path the background warm takes on a truly cold, offline machine.

        The manifest URL points at a dead loopback port, so the fetch the
        cold cache forces fails instantly and deterministically with no real
        network involved. The warm must report the language degraded --
        today's labeled-skip behavior -- not raise. A subprocess because the
        pack's cache directory is process-global.
        """
        cold = tmp_path / "cold-cache"
        cold.mkdir()
        env = dict(os.environ)
        env[grammars.ENV_CACHE_DIR] = str(cold)
        env["TREE_SITTER_LANGUAGE_PACK_MANIFEST_URL"] = "http://127.0.0.1:9/parsers.json"
        env.pop(grammars.ENV_NO_DOWNLOAD, None)
        script = (
            "import sys\n"
            "from agentless_mcp.core import grammars\n"
            "report = grammars.warmup(['json'])\n"
            "(cap,) = report.languages\n"
            "print(cap.detail)\n"
            "sys.exit(0 if not report.ok and not cap.warmed else 1)\n"
        )
        result = subprocess.run(
            [sys.executable, "-c", script],
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
            check=False,
        )
        assert result.returncode == 0, result.stdout + result.stderr
        assert result.stdout.startswith("fetch failed: ")
        assert not (cold / "libtree_sitter_json.so").exists()


class TestWaitBudget:
    """The wait is budgeted from the wait, not from the warm's start.

    The callers that need this -- a CLI process at its exit, a server about to
    replace its own image -- can arrive long after the start deadline lapsed.
    A lapsed deadline read as "nothing to wait for" skipped the join entirely,
    which is the mid-extraction kill the function exists to prevent.
    """

    def test_a_lapsed_start_deadline_still_joins(self, monkeypatch):
        joined = []

        class Warm:
            def is_alive(self):
                return False

            def join(self, timeout=None):
                joined.append(timeout)

        state = grammars._AutoWarmState(thread=Warm(), deadline=time.monotonic() - 3600)
        monkeypatch.setattr(grammars, "_AUTO_WARM", state)

        grammars.wait_for_auto_warm()

        assert joined == [grammars.AUTO_WARM_JOIN_SECONDS]

    def test_no_warm_means_no_wait(self, monkeypatch):
        monkeypatch.setattr(grammars, "_AUTO_WARM", grammars._AutoWarmState())

        grammars.wait_for_auto_warm()

    def test_a_warm_that_outlasts_the_budget_is_reported_not_awaited(self, monkeypatch, caplog):
        class Stuck:
            def is_alive(self):
                return True

            def join(self, timeout=None):
                return None

        monkeypatch.setattr(grammars, "AUTO_WARM_JOIN_SECONDS", 0.01)
        monkeypatch.setattr(
            grammars, "_AUTO_WARM", grammars._AutoWarmState(thread=Stuck(), deadline=0.0)
        )

        with caplog.at_level(logging.WARNING, logger="agentless_mcp.core.grammars"):
            grammars.wait_for_auto_warm()

        assert "may be truncated" in caplog.text
