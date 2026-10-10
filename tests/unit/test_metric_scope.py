"""The metric scope: one rule for which files carry metrics, and every index entry point uses it."""

import ast
from pathlib import Path

import pytest

import agentless_mcp
from agentless_mcp.application.symbol_service import in_metric_scope

SRC = Path(agentless_mcp.__file__).parent
ENTRY_POINTS = frozenset({"build_index", "start_auto_index"})
# The index's own pass-through: the background refresh forwards the rule it was given.
FORWARDING_MODULE = "core/cache.py"


class TestTheRule:
    @pytest.mark.parametrize(
        "path",
        ["app.py", "src/pkg/app.py", "web/view.tsx", "cmd/tool.go", "lib/a.rs", "run.sh"],
    )
    def test_a_tier1_source_file_is_measured(self, path):
        assert in_metric_scope(path)

    @pytest.mark.parametrize(
        "path",
        [
            "tests/test_app.py",
            "pkg/tests/helpers.py",
            "conftest.py",
            "pkg/store_test.go",
            "web/view.test.ts",
            "src/fixtures/sample.py",
        ],
    )
    def test_a_test_or_fixture_path_is_not(self, path):
        assert not in_metric_scope(path)

    @pytest.mark.parametrize("path", ["README.md", "config.json", "schema.sql", "App.kt", "a"])
    def test_a_file_without_tier1_metrics_is_not(self, path):
        assert not in_metric_scope(path)


def index_calls():
    for path in sorted(SRC.rglob("*.py")):
        tree = ast.parse(path.read_text(encoding="utf-8"))
        for node in ast.walk(tree):
            if not isinstance(node, ast.Call):
                continue
            func = node.func
            name = func.attr if isinstance(func, ast.Attribute) else getattr(func, "id", None)
            if name in ENTRY_POINTS:
                yield path.relative_to(SRC).as_posix(), name, node


def test_every_index_entry_point_passes_the_metric_scope():
    found: dict[str, set[str]] = {}
    for path, name, call in index_calls():
        measure = {keyword.arg: keyword.value for keyword in call.keywords}.get("measure")
        expected = "measure" if path == FORWARDING_MODULE else "in_metric_scope"
        assert isinstance(measure, ast.Name), f"{path}:{call.lineno}: {name} passes no rule"
        assert measure.id == expected, f"{path}:{call.lineno}: {name} passes {measure.id}"
        found.setdefault(path, set()).add(name)

    # A new entry point fails here until it is checked to pass the rule.
    assert found == {
        "adapters/cli/main.py": {"build_index"},
        "adapters/mcp/server.py": {"start_auto_index"},
        "core/cache.py": {"build_index"},
    }
