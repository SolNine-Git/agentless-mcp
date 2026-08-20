"""The adapter-neutral capabilities contract and its compact text view."""

from dataclasses import replace

from agentless_mcp.application.capability_service import (
    build_capability_report,
    render_capability_report,
)
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.core.grammars import LanguageCapability


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
