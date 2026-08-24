"""The adapter-neutral capabilities contract and its compact text view."""

from dataclasses import replace

from agentless_mcp.application import (
    capability_service,
    envelope,
    graph_service,
    map_service,
    symbol_service,
)
from agentless_mcp.application.capability_service import (
    build_capability_report,
    render_capability_report,
)
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.core import (
    cache,
    communities,
    grammars,
    htmlgraph,
    locs,
    mermaid,
    projectconfig,
    resolve,
    treewalk,
)
from agentless_mcp.core.grammars import LanguageCapability
from agentless_mcp.util import fslimits


def test_report_contains_every_documented_capability_surface(tmp_path, extractor):
    (tmp_path / "sample.py").write_text("value = 1\n", encoding="utf-8")
    report = build_capability_report(resolve_repo(tmp_path, None), extractor)
    document = report.as_dict()

    assert document["languages"]
    assert document["extensions"][".py"] == "python"
    assert document["effective_config"]["max_files"] == 10
    assert document["caps"]["max_output_tokens"] == 16000
    assert document["cache"]["generation_matches"] is False


class TestTheCacheStatusIsMeasured:
    """The report describes this repository's database, not a stand-in.

    A call carrying no opened source used to be answered from
    ``cache.OnDemandSource(extractor)``, whose status is synthesised: "path
    None", generation None, and the advice to run ``index`` -- produced
    without looking for a database at all. Both assertions below fail against
    that stand-in.
    """

    def test_an_absent_cache_names_the_database_it_looked_for(self, tmp_path, extractor):
        (tmp_path / "sample.py").write_text("value = 1\n", encoding="utf-8")
        ctx = resolve_repo(tmp_path, None)

        document = build_capability_report(ctx, extractor).as_dict()

        assert document["cache"]["path"] == str(cache.cache_path(ctx.root))
        assert document["cache"]["generation_matches"] is False
        assert document["cache"]["files"] == 0
        assert "agentless-mcp index" in document["cache_hint"]

    def test_an_opened_source_reports_its_own_generation(self, tmp_path, extractor):
        (tmp_path / "sample.py").write_text("value = 1\n", encoding="utf-8")
        built = cache.build_index(tmp_path, extractor)
        source = cache.open_source(tmp_path, extractor, tree_oid=None)
        try:
            ctx = replace(resolve_repo(tmp_path, None), symbols=source)
            document = build_capability_report(ctx, extractor).as_dict()
        finally:
            source.close()

        assert document["cache"]["generation"] == built.generation
        assert document["cache"]["generation_matches"] is True
        assert document["cache"]["files"] == 1
        assert document["cache_hint"] == ""


class TestTheTwoRenderingsCarryTheSameFacts:
    """`--json` and the text form are one report, so the hint is in both.

    The CLI builds both up front so they cannot diverge by accident, and the
    remediation for an absent cache -- the one actionable line in the whole
    report -- was in the text form only.
    """

    def test_the_json_form_names_the_command_that_builds_the_cache(self, tmp_path, extractor):
        (tmp_path / "sample.py").write_text("value = 1\n", encoding="utf-8")
        report = build_capability_report(resolve_repo(tmp_path, None), extractor)

        assert report.as_dict()["cache_hint"] == report.cache_hint
        assert report.cache_hint in render_capability_report(report)


class TestNoAllowlistIsNotAnEmptyAllowlist:
    """CLI mode has no allowlist; a server may have one that is empty.

    ``repo_context`` keeps those apart in the type on purpose -- "that is a
    different question ... so it is a different value rather than an empty
    list" -- and the tool whose job is to report the configuration collapsed
    them into "roots: none configured".
    """

    def report(self, tmp_path, extractor, **roots):
        (tmp_path / "sample.py").write_text("value = 1\n", encoding="utf-8")
        return build_capability_report(resolve_repo(tmp_path, None), extractor, **roots)

    def test_a_call_with_no_allowlist_reads_as_unrestricted(self, tmp_path, extractor):
        report = self.report(tmp_path, extractor)

        assert report.configured_roots is None
        assert report.as_dict()["roots"]["configured"] is None
        assert "roots: unrestricted (CLI mode)" in render_capability_report(report)

    def test_a_server_configured_with_no_roots_says_so(self, tmp_path, extractor):
        report = self.report(tmp_path, extractor, configured_roots=())

        assert report.configured_roots == ()
        assert report.as_dict()["roots"]["configured"] == []
        assert "roots: none configured" in render_capability_report(report)

    def test_a_configured_root_is_named(self, tmp_path, extractor):
        report = self.report(tmp_path, extractor, configured_roots=(tmp_path,))

        assert report.as_dict()["roots"]["configured"] == [str(tmp_path)]
        assert f"roots: {tmp_path}" in render_capability_report(report)


class TestABackgroundWarmIsReported:
    """A cold language being warmed right now is not a cold language.

    Both adapters start a background warm at process start, so a
    `capabilities` call in the first minute of a server's life told the agent
    to run `warmup` -- duplicating the warm already running -- and showed
    tier-1 languages as unavailable with nothing saying it was temporary. The
    cache half of this same report already solves this, for the same reason.
    """

    # Which grammars are warm on the machine running the suite is not this
    # test's subject, so the language list is stubbed rather than read.
    LANGUAGES = (
        LanguageCapability("python", 14, "1.0", warmed=True, probe_ok=True),
        LanguageCapability(
            "sql",
            None,
            "1.0",
            warmed=False,
            probe_ok=False,
            detail="not warmed: run agentless-mcp warmup sql",
        ),
    )

    def build(self, tmp_path, extractor, monkeypatch, *, warming):
        (tmp_path / "sample.py").write_text("value = 1\n", encoding="utf-8")
        monkeypatch.setattr(grammars, "loaded_capabilities", lambda: list(self.LANGUAGES))
        monkeypatch.setattr(grammars, "auto_warm_in_progress", lambda: warming)
        return build_capability_report(resolve_repo(tmp_path, None), extractor)

    def test_a_cold_language_says_the_warm_is_in_flight(self, tmp_path, extractor, monkeypatch):
        report = self.build(tmp_path, extractor, monkeypatch, warming=True)
        text = render_capability_report(report)

        assert report.languages[1].detail == capability_service.WARM_IN_PROGRESS
        assert "warming now: sql:2/-" in text
        assert "run agentless-mcp warmup" not in text
        # The warm changes nothing about a language that is already warm.
        assert report.languages[0] == self.LANGUAGES[0]

    def test_with_no_warm_running_the_advice_is_to_run_warmup(
        self, tmp_path, extractor, monkeypatch
    ):
        report = self.build(tmp_path, extractor, monkeypatch, warming=False)
        text = render_capability_report(report)

        assert report.languages == self.LANGUAGES
        assert "unavailable: sql:2/-" in text
        assert "warming now:" not in text


def test_renderer_groups_normal_language_states_and_preserves_exceptions(tmp_path, extractor):
    (tmp_path / "sample.py").write_text("value = 1\n", encoding="utf-8")
    report = build_capability_report(resolve_repo(tmp_path, None), extractor)
    languages = (
        LanguageCapability("python", 14, "1.0", warmed=True, probe_ok=True),
        LanguageCapability(
            "json",
            None,
            "1.0",
            warmed=False,
            probe_ok=False,
            detail="not warmed: unavailable",
        ),
        LanguageCapability(
            "sql",
            14,
            "1.0",
            warmed=True,
            probe_ok=False,
            detail="probe failed",
        ),
    )

    text = render_capability_report(replace(report, languages=languages))

    assert "warmed+probe: python:1/14" in text
    assert "unavailable: json:2/-" in text
    assert "warmed=true probe=false: sql:2/14 -- probe failed" in text


class TestTheEffectiveConfigCoversEveryKey:
    """`_effective_config` says it resolves the repository defaults, so check it.

    The report is what a caller reads to learn what this repository asked for.
    A key the parser accepts and this omits is a setting a repository can turn
    on with nothing anywhere saying it is on.
    """

    def test_every_parsed_key_is_resolved_in_the_report(self):
        # `stoplist` and `test_cmd` are reported under their own names; every
        # other key in the schema has to appear under its own name too.
        reported = {name for name, _ in capability_service._effective_config(projectconfig.EMPTY)}

        missing = [key for key in projectconfig.KNOWN_KEYS if key not in reported]
        assert not missing, f"config keys the report does not resolve: {missing}"

    def test_relation_weights_defaults_to_off(self):
        """The flag ships dark, and the report is where that is auditable.

        It reads `ASTSymbol.bases`, which only the extractor's Python class
        handler fills in, so it is not language-neutral and must not become
        the default by drift.
        """
        resolved = dict(capability_service._effective_config(projectconfig.EMPTY))

        assert resolved["relation_weights"] is False

    def test_a_repository_that_asks_for_it_is_reported_as_asking(self):
        config = projectconfig.parse({"relation_weights": True})

        resolved = dict(capability_service._effective_config(config))

        assert resolved["relation_weights"] is True


class TestTheInventoryIsComplete:
    """`_caps` says it lists every bound the services apply, so check it.

    It used to say "every public bound in force" and list eight of the
    twenty-three a caller can set or hit, which made it a sample presented as
    an inventory. Hand-maintained is fine; hand-maintained and unchecked is
    what let it drift, so this walks the modules it draws from and fails when
    one grows a bound the inventory has not learned.
    """

    # Every module `_caps` reports from, and the names in each that are
    # bounds rather than tuning constants or defaults of another kind. A new
    # entry in one of these modules shows up here as a missing value.
    SOURCES = (
        (envelope, ("DEFAULT_MAX_TOKENS", "MAX_CONFIG_WARNINGS")),
        (
            graph_service,
            (
                "DEFAULT_EXPLAIN_LIMIT",
                "DEFAULT_CYCLE_LIMIT",
                "DEFAULT_COMMUNITY_LIMIT",
                "DEFAULT_HEALTH_LIMIT",
                "DEFAULT_MEMBER_LIMIT",
            ),
        ),
        (symbol_service, ("DEFAULT_FIND_LIMIT", "DEFAULT_REFS_LIMIT", "DEFAULT_EXPAND_LIMIT")),
        (map_service, ("DEFAULT_MAX_FILES", "DEFAULT_MAX_TEST_FILES", "TEST_COMPANION_DEPTH")),
        (mermaid, ("DEFAULT_DIAGRAM_NODES", "DEFAULT_DIAGRAM_EDGES")),
        (
            htmlgraph,
            (
                "DEFAULT_HTML_NODES",
                "DEFAULT_HTML_EDGES",
                "MAX_HTML_NODES",
                "MAX_HTML_EDGES",
            ),
        ),
        (communities, ("DEFAULT_RESOLUTION",)),
        (locs, ("DEFAULT_CONTEXT_LINES",)),
        (resolve, ("DEFAULT_MAX_VISITED",)),
        (treewalk, ("DEFAULT_RENDER_DEPTH", "DEFAULT_MAX_ENTRIES")),
        (fslimits, ("DEFAULT_MAX_DEPTH", "DEFAULT_MAX_WALK_FILES", "DEFAULT_MAX_FILE_BYTES")),
    )

    def test_every_declared_bound_is_reported(self):
        reported = dict(capability_service._caps())
        missing = [
            f"{module.__name__}.{name} = {getattr(module, name)}"
            for module, names in self.SOURCES
            for name in names
            if getattr(module, name) not in reported.values()
        ]
        assert not missing, f"bounds the capability report does not name: {missing}"

    def test_no_name_in_the_inventory_means_two_values(self):
        # The finding behind the rename: DEFAULT_MAX_NODES was 40 in
        # core/mermaid and 200 in core/htmlgraph, and both spellings reached
        # `--help`. Pinned across the modules rather than inside one, because
        # a collision between two modules is what happened.
        declared: dict[str, set[float]] = {}
        for module, names in self.SOURCES:
            for name in names:
                declared.setdefault(name, set()).add(getattr(module, name))

        collisions = {name: values for name, values in declared.items() if len(values) > 1}
        assert not collisions, f"one name, two values: {collisions}"

    def test_the_report_carries_every_entry(self, tmp_path, extractor):
        (tmp_path / "sample.py").write_text("value = 1\n", encoding="utf-8")
        report = build_capability_report(resolve_repo(tmp_path, None), extractor)

        assert dict(report.as_dict()["caps"]) == dict(capability_service._caps())
