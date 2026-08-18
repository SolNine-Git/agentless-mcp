"""Tests for grammar loading, warmed-state gating and degradation.

Nothing here reaches the network: the warmed-state probe is monkeypatched so
the "not warmed" and "no download" paths are exercised without depending on
what this machine happens to have cached.
"""

import pytest
import tree_sitter_language_pack as pack

from agentless_mcp.core import grammars
from agentless_mcp.util.errors import AtlasError, LanguageUnavailable


class TestGetLanguage:
    def test_warmed_language_loads(self):
        language = grammars.get_language("python")
        assert language.abi_version >= 13

    def test_parser_is_memoized(self):
        assert grammars.get_parser("python") is grammars.get_parser("python")

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

    def test_default_capability_list_is_tier_one(self, monkeypatch):
        monkeypatch.setattr(pack, "downloaded_languages", list)
        names = [cap.name for cap in grammars.loaded_capabilities()]
        assert names == list(grammars.TIER1_LANGUAGES)
