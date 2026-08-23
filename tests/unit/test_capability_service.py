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
from agentless_mcp.core import communities, htmlgraph, locs, mermaid, resolve, treewalk
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
                "DEFAULT_MEMBER_LIMIT",
            ),
        ),
        (symbol_service, ("DEFAULT_FIND_LIMIT", "DEFAULT_REFS_LIMIT", "DEFAULT_EXPAND_LIMIT")),
        (map_service, ("DEFAULT_MAX_FILES",)),
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
