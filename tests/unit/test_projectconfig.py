"""The `.agentless-mcp.json` project config, treated as repository content.

Two halves. The schema tests say what a well-formed file does; the hostile
half says what a file written to abuse this does, and the answer is always the
same shape -- the value is dropped, a warning is produced, and the tool still
answers. A config file that could stop a read command, or reach a path, or
nominate a command an MCP client then runs, would be a hole rather than a
feature.
"""

import json

import pytest

from agentless_mcp.adapters.cli.formatting import EXIT_OK, EXIT_USAGE
from agentless_mcp.adapters.cli.main import CliServices, run
from agentless_mcp.application.graph_service import GraphService
from agentless_mcp.application.lint_service import LintService
from agentless_mcp.application.map_service import (
    DEFAULT_MAX_FILES,
    GRANULARITY_FUNCTION,
    MapRequest,
    MapService,
)
from agentless_mcp.application.patch_service import PatchService
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.application.validate_service import ValidateService
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.core import projectconfig

MODULE = '''\
"""Module."""


def quote(sku):
    """Return the quote for a sku."""
    return 1
'''


@pytest.fixture
def services(extractor, counter):
    """The same wiring bootstrap builds, without the console-script layer."""
    return CliServices(
        maps=MapService(extractor, counter),
        views=ViewService(extractor),
        symbols=SymbolService(extractor, counter),
        graphs=GraphService(extractor),
        patches=PatchService(extractor),
        validates=ValidateService(PatchService(extractor)),
        lints=LintService(extractor),
        counter=counter,
        extractor=extractor,
    )


@pytest.fixture
def repo(tmp_path):
    """A one-file repository, with a factory for writing its config."""

    def build(config=None, *, raw=None):
        (tmp_path / "core.py").write_text(MODULE, encoding="utf-8")
        if raw is not None:
            (tmp_path / projectconfig.CONFIG_FILENAME).write_text(raw, encoding="utf-8")
        elif config is not None:
            (tmp_path / projectconfig.CONFIG_FILENAME).write_text(
                json.dumps(config), encoding="utf-8"
            )
        return tmp_path

    return build


class TestAbsentAndMalformed:
    def test_no_file_is_an_empty_configuration_with_no_warnings(self, repo):
        config = projectconfig.load(repo())
        assert config.present is False
        assert config.warnings == ()

    def test_a_file_that_is_not_json_is_reported_not_raised(self, repo):
        config = projectconfig.load(repo(raw="{not json"))
        assert config.present is False
        assert "not valid JSON" in config.warnings[0]

    def test_a_json_array_is_refused(self, repo):
        config = projectconfig.load(repo(raw="[1, 2, 3]"))
        assert "must hold a JSON object" in config.warnings[0]

    def test_an_oversized_file_is_not_read(self, repo):
        padding = "x" * (projectconfig.MAX_CONFIG_BYTES + 1)
        config = projectconfig.load(repo(raw=json.dumps({"note": padding})))
        assert config.present is False
        assert "over the" in config.warnings[0]

    def test_a_malformed_file_never_stops_a_read_command(self, repo, services, capsys):
        root = repo(raw="{{{")
        assert run(["map", "--repo", str(root)], services) == EXIT_OK
        assert "config warning" in capsys.readouterr().out

    def test_a_symlinked_config_is_not_read(self, tmp_path):
        root = tmp_path / "repo"
        root.mkdir()
        outside = tmp_path / "elsewhere.json"
        outside.write_text(json.dumps({"borrowed_key_name": 1}), encoding="utf-8")
        (root / projectconfig.CONFIG_FILENAME).symlink_to(outside)

        config = projectconfig.load(root)

        assert config.present is False
        assert any("outside the repository" in warning for warning in config.warnings)
        assert not any("borrowed_key_name" in warning for warning in config.warnings)


class TestSchema:
    def test_known_keys_are_parsed_into_typed_values(self, repo):
        config = projectconfig.load(
            repo(
                {
                    "map_budget": 3000,
                    "max_files": 4,
                    "granularity": "file",
                    "docstrings": True,
                    "stoplist": ["ctx", "helper"],
                    "test_cmd": "pytest -q",
                }
            )
        )
        assert config.map_budget == 3000
        assert config.max_files == 4
        assert config.granularity == "file"
        assert config.docstrings is True
        assert config.stoplist == frozenset({"ctx", "helper"})
        assert config.test_cmd == "pytest -q"
        assert config.warnings == ()

    def test_an_unknown_key_is_a_warning_not_an_error(self, repo):
        config = projectconfig.load(repo({"map_budget": 3000, "nonsense": 1}))
        assert config.map_budget == 3000
        assert any("unknown key 'nonsense'" in warning for warning in config.warnings)

    def test_a_file_full_of_unknown_keys_produces_bounded_warnings(self, repo):
        extra = projectconfig.MAX_UNKNOWN_KEY_WARNINGS + 50
        config = projectconfig.load(repo({f"k{index}": 1 for index in range(extra)}))

        assert len(config.warnings) == projectconfig.MAX_UNKNOWN_KEY_WARNINGS + 1
        assert (
            config.warnings[-1]
            == f"50 further unknown keys in {projectconfig.CONFIG_FILENAME}: warnings suppressed"
        )

    @pytest.mark.parametrize(
        ("key", "value"),
        [
            ("map_budget", projectconfig.MAX_BUDGET + 1),
            ("map_budget", 0),
            ("max_files", projectconfig.MAX_MAX_FILES + 1),
            ("max_files", 0),
        ],
    )
    def test_an_out_of_bounds_number_is_dropped_not_clamped(self, repo, key, value):
        config = projectconfig.load(repo({key: value}))
        assert getattr(config, key) is None
        assert any(key in warning for warning in config.warnings)

    @pytest.mark.parametrize(
        ("key", "value"),
        [
            ("map_budget", "3000"),
            ("map_budget", True),
            ("max_files", 4.5),
            ("granularity", "symbol"),
            ("granularity", 1),
            ("docstrings", "yes"),
            ("stoplist", "ctx"),
            ("test_cmd", 42),
            ("test_cmd", ""),
        ],
    )
    def test_a_wrongly_typed_value_is_dropped_with_a_warning(self, repo, key, value):
        config = projectconfig.load(repo({key: value}))
        assert getattr(config, key) in (None, frozenset())
        assert config.warnings

    def test_a_multi_line_test_command_is_refused(self, repo):
        config = projectconfig.load(repo({"test_cmd": "pytest -q\nrm -rf /"}))
        assert config.test_cmd is None
        assert any("single line" in warning for warning in config.warnings)

    def test_an_over_long_test_command_is_refused(self, repo):
        config = projectconfig.load(
            repo({"test_cmd": "x" * (projectconfig.MAX_TEST_CMD_CHARS + 1)})
        )
        assert config.test_cmd is None


class TestHostileStoplist:
    def test_a_stoplist_longer_than_the_cap_is_truncated_with_a_warning(self, repo):
        entries = [f"name{index}" for index in range(projectconfig.MAX_STOPLIST_ENTRIES + 50)]
        config = projectconfig.load(repo({"stoplist": entries}))
        assert len(config.stoplist) == projectconfig.MAX_STOPLIST_ENTRIES
        assert any("over the" in warning for warning in config.warnings)

    @pytest.mark.parametrize(
        "entry",
        [
            "../../etc/passwd",
            "/etc/passwd",
            "C:\\Windows\\system32",
            "name with spaces",
            "$(rm -rf /)",
            "`whoami`",
            "a" * (projectconfig.MAX_STOPLIST_ENTRY_CHARS + 1),
            "",
            42,
            None,
            ["nested"],
        ],
    )
    def test_an_entry_that_is_not_a_bare_name_is_dropped(self, repo, entry):
        config = projectconfig.load(repo({"stoplist": ["kept", entry]}))
        assert config.stoplist == frozenset({"kept"})
        assert any("bare names" in warning for warning in config.warnings)


class TestPrecedence:
    def test_the_config_supplies_the_map_default(self, repo, services, capsys):
        root = repo({"max_files": 1})
        assert run(["map", "--repo", str(root), "--json"], services) == EXIT_OK
        payload = json.loads(capsys.readouterr().out)
        assert len(payload["files"]) == 1

    def test_an_explicit_argument_beats_the_config(self, repo, extractor, counter):
        ctx = resolve_repo(repo({"granularity": "file"}), None)
        maps = MapService(extractor, counter)

        configured = maps.build(ctx, MapRequest())
        overridden = maps.build(ctx, MapRequest(granularity=GRANULARITY_FUNCTION))

        assert all(entry.entries == () for entry in configured.files)
        assert any(entry.entries for entry in overridden.files)

    def test_the_built_in_default_applies_when_neither_speaks(self, repo, extractor, counter):
        ctx = resolve_repo(repo(), None)
        result = MapService(extractor, counter).build(ctx, MapRequest())
        assert len(result.files) <= DEFAULT_MAX_FILES

    def test_the_config_supplies_the_docstring_default(self, repo, services, capsys):
        root = repo({"docstrings": True})
        assert run(["skeleton", "core.py", "--repo", str(root)], services) == EXIT_OK
        assert "Return the quote for a sku" in capsys.readouterr().out

    def test_the_receipt_names_the_config_file(self, repo, services, capsys):
        root = repo({"max_files": 2})
        run(["map", "--repo", str(root)], services)
        assert projectconfig.CONFIG_FILENAME in capsys.readouterr().out


class TestTestCommand:
    def test_validate_refuses_when_neither_the_flag_nor_the_config_names_one(
        self, repo, services, tmp_path, capsys
    ):
        candidates = tmp_path / "candidates"
        candidates.mkdir()
        (candidates / "one.txt").write_text("noop", encoding="utf-8")

        code = run(["validate", "--repo", str(repo()), "--candidates", str(candidates)], services)
        assert code == EXIT_USAGE
        assert projectconfig.CONFIG_FILENAME in capsys.readouterr().err

    def test_the_configured_command_is_echoed_before_it_runs(
        self, make_git_repo, services, tmp_path, capsys, python_cmd
    ):
        script = tmp_path / "green.py"
        script.write_text("print('ok')\n", encoding="utf-8")
        command = python_cmd(str(script))
        root = make_git_repo(
            {
                "app.py": "def add(a, b):\n    return a + b\n",
                projectconfig.CONFIG_FILENAME: json.dumps({"test_cmd": command}),
            }
        )

        candidates = tmp_path / "candidates"
        candidates.mkdir()
        (candidates / "one.txt").write_text("nothing to apply", encoding="utf-8")

        run(["validate", "--repo", str(root), "--candidates", str(candidates)], services)
        errors = capsys.readouterr().err
        assert command in errors
        assert "from the repository under analysis" in errors
