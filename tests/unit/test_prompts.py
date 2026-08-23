"""The prompt data files: eager validation, template fields, wire descriptions.

The prompt text is data now, so it needs the guarantees data gets. Three of
them are asserted here.

*It is complete at startup.* A key the code consumes and the file does not
carry -- or the reverse -- raises while the module is imported, so a typo
cannot reach an agent as a blank tool description.

*Its placeholders are the ones the call sites supply.* Each template is
formatted here with exactly the arguments its caller passes, and the field
sets are compared, so renaming ``{roots}`` in the JSON fails here rather than
raising ``KeyError`` inside a refusal path in production. The call sites
themselves are covered by the tests that exercise them (``test_envelope``,
``test_cache``, ``test_mcp_server``), which format the same templates for
real.

*It reaches the wire.* Every description is read back off a live FastMCP
listing, not off the JSON they came from.
"""

import asyncio
import json
from string import Formatter
from typing import Any

import pytest
from fastmcp import Client

from agentless_mcp.adapters.mcp.server import (
    SURFACE_BOTH,
    ServerServices,
    ToolHandlers,
    build_server,
)
from agentless_mcp.application.graph_service import GraphService
from agentless_mcp.application.map_service import MapService
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.prompts import (
    ENVELOPE,
    MESSAGES,
    PARAMETER_DESCRIPTIONS,
    PARAMETER_NAMES,
    TOOL_DESCRIPTIONS,
    TOOL_NAMES,
    EnvelopeText,
    MessageText,
    PromptDataError,
    loader,
)

# What every template is formatted with at its call site. The keys are the
# manifest for the placeholder assertions below; a template that grows a field
# has to grow an entry here, which is the point.
ENVELOPE_ARGUMENTS = {
    "receipt_header": {},
    "receipt_line": {"root": "/srv/app", "head": "1a2b3c4d", "dirty": "3", "cache": "none"},
    "receipt_note": {"note": "git status timed out"},
    "receipt_config": {"path": "/srv/app/.agentless-mcp.json"},
    "receipt_config_warning": {"warning": "map_budget is not an integer"},
    "banner": {},
    "notice": {},
    "service_truncation": {"shown": 12, "total": 40, "unit": "symbols"},
    "ceiling_truncation": {"max_tokens": 16_000, "dropped": 7, "total": 900},
    "json_ceiling_untrimmable": {"max_tokens": 16_000},
    "json_ceiling_trimmed": {"max_tokens": 16_000},
}

MESSAGE_ARGUMENTS = {
    "server_no_roots": {},
    "server_root_required": {"roots": "/srv/app, /srv/other"},
    "unknown_operation": {
        "tool": "analyze_structure",
        "operation": "graph",
        "operations": "cycles, diagram, path",
    },
    "op_rejects_parameters": {
        "tool": "orient",
        "operation": "map",
        "stray": "source, target",
        "accepted": "focus, budget, limit, granularity",
        "required": "none",
    },
    "op_requires_parameters": {
        "tool": "orient",
        "operation": "path",
        "missing": "target",
        "accepted": "source, target, include_unique, include_ambiguous",
        "required": "source, target",
    },
    "path_needs_endpoints": {},
    "map_limit_out_of_range": {"limit": "500", "minimum": "1", "maximum": "200"},
    "repo_refused_no_roots": {},
    "repo_refused_not_allowed": {"roots": "/srv/app, /srv/other"},
    "roots_file_hint": {"file": "/srv/agentless-roots.txt"},
    "roots_file_unreadable": {"file": "/srv/agentless-roots.txt", "error": "No such file"},
    "map_unresolved_seeds": {"seeds": "rotate_age, shift_age"},
    "scan_skipped_files": {
        "count": 2,
        "listed": "huge.py (skipped: 1040000 bytes exceeds the per-file cap of 1000000 bytes)",
    },
    "expand_body_truncated": {"shown": 12, "total": 340},
    "expand_batch_shortened": {"shortened": 3, "total": 10, "budget": 12_000},
    "expand_no_room": {"requested": 400, "seats": 40},
    "overview_stable_ids": {"pattern": "py:src/app/svc.py::<QualifiedName>"},
    "slice_range_beyond_file": {"start": 9000, "end": 9050, "path": "src/app/svc.py", "total": 242},
    "cache_stale_remediation": {},
    "cache_stale_refreshing": {},
    "cache_absent_refreshing": {},
    "cache_build_hint": {"repo_root": "/srv/app"},
    "cache_discarded_no_index": {},
    "cache_discarded_old_schema": {"found": 1, "expected": 2},
    "cache_discarded_other_repo": {"repo_root": "/srv/other"},
}

ENVELOPE_TEXT = json.dumps({key: f"<{key}>" for key in ENVELOPE_ARGUMENTS})


def fields_of(template: str) -> set[str]:
    """The ``str.format`` field names a template names."""
    return {name for _, name, _, _ in Formatter().parse(template) if name}


class TestLoadedData:
    def test_every_tool_name_has_a_description(self):
        assert set(TOOL_DESCRIPTIONS) == set(TOOL_NAMES)
        for name in TOOL_NAMES:
            assert TOOL_DESCRIPTIONS[name].strip(), name

    def test_every_parameter_name_has_a_description(self):
        assert set(PARAMETER_DESCRIPTIONS) == set(PARAMETER_NAMES)
        for name in PARAMETER_NAMES:
            assert PARAMETER_DESCRIPTIONS[name].strip(), name

    def test_the_description_mapping_cannot_be_edited(self):
        descriptions: Any = TOOL_DESCRIPTIONS
        with pytest.raises(TypeError):
            descriptions["repo_map"] = "something else"

    def test_the_records_are_frozen(self):
        record: Any = ENVELOPE
        with pytest.raises(AttributeError):
            record.banner = "x"

    def test_the_envelope_carries_the_documented_wording(self):
        assert ENVELOPE.receipt_header == "# agentless-mcp receipt"
        assert ENVELOPE.banner.startswith("# NOTE:")
        assert ENVELOPE.notice in ENVELOPE.banner


class TestTemplates:
    @pytest.mark.parametrize(("name", "arguments"), sorted(ENVELOPE_ARGUMENTS.items()))
    def test_every_envelope_template_formats_with_its_arguments(self, name, arguments):
        template = getattr(ENVELOPE, name)
        assert fields_of(template) == set(arguments)
        rendered = template.format(**arguments)
        assert "{" not in rendered

    @pytest.mark.parametrize(("name", "arguments"), sorted(MESSAGE_ARGUMENTS.items()))
    def test_every_message_template_formats_with_its_arguments(self, name, arguments):
        template = getattr(MESSAGES, name)
        assert fields_of(template) == set(arguments)
        rendered = template.format(**arguments)
        assert "{" not in rendered

    def test_the_argument_tables_cover_every_field(self):
        assert set(ENVELOPE_ARGUMENTS) == set(vars(ENVELOPE))
        assert set(MESSAGE_ARGUMENTS) == set(vars(MESSAGES))

    def test_no_tool_description_carries_a_format_field(self):
        for name, description in TOOL_DESCRIPTIONS.items():
            assert fields_of(description) == set(), name

    @pytest.mark.parametrize("name", ["server_no_roots", "repo_refused_no_roots"])
    def test_a_no_roots_refusal_names_both_ways_to_configure_a_root(self, name):
        """Issue #4: naming only --root sends the operator to a 30-flag command."""
        template = getattr(MESSAGES, name)
        assert "--root DIR" in template
        assert "--roots-from FILE" in template

    @pytest.mark.parametrize("name", ["server_no_roots", "repo_refused_no_roots"])
    def test_a_no_roots_refusal_does_not_demand_a_restart(self, name):
        """``roots_file_hint`` is appended to these when a roots file exists.

        It says the file is re-read with no restart needed, so a "restart the
        server" imperative in the static half would contradict the hint in the
        same refusal.
        """
        assert "restart" not in getattr(MESSAGES, name).lower()

    def test_the_re_read_fact_has_exactly_one_home(self):
        """Stated in roots_file_hint alone, so the two never drift apart."""
        carriers = [name for name in vars(MESSAGES) if "re-read" in getattr(MESSAGES, name)]
        assert carriers == ["roots_file_hint"]


class TestEagerValidation:
    """Every defect below is raised while the package is imported."""

    def test_a_missing_key_is_refused_by_name(self):
        document = json.loads(ENVELOPE_TEXT)
        del document["banner"]
        with pytest.raises(PromptDataError, match=r"missing \['banner'\]"):
            loader.build_record(
                "envelope.json", json.dumps(document), EnvelopeText, ENVELOPE_ARGUMENTS
            )

    def test_a_key_no_code_consumes_is_refused_by_name(self):
        document = json.loads(ENVELOPE_TEXT)
        document["bannner"] = "typo"
        with pytest.raises(PromptDataError, match=r"unknown \['bannner'\]"):
            loader.build_record(
                "envelope.json", json.dumps(document), EnvelopeText, ENVELOPE_ARGUMENTS
            )

    def test_a_blank_value_is_refused(self):
        document = json.loads(ENVELOPE_TEXT)
        document["banner"] = "   "
        with pytest.raises(PromptDataError, match="must be a non-empty string"):
            loader.build_record(
                "envelope.json", json.dumps(document), EnvelopeText, ENVELOPE_ARGUMENTS
            )

    def test_a_non_string_value_is_refused(self):
        document = json.loads(ENVELOPE_TEXT)
        document["banner"] = 3
        with pytest.raises(PromptDataError, match="must be a non-empty string"):
            loader.build_record(
                "envelope.json", json.dumps(document), EnvelopeText, ENVELOPE_ARGUMENTS
            )

    def test_malformed_json_names_the_file_and_the_parse_error(self):
        with pytest.raises(PromptDataError, match=r"messages\.json.*is not valid JSON"):
            loader.parse_strings("messages.json", "{not json", MESSAGE_ARGUMENTS)

    def test_a_json_document_that_is_not_an_object_is_refused(self):
        with pytest.raises(PromptDataError, match="must hold a JSON object, not list"):
            loader.parse_strings("messages.json", "[]", MESSAGE_ARGUMENTS)

    def test_a_missing_file_names_the_package_it_should_ship_in(self):
        with pytest.raises(PromptDataError, match=r"agentless_mcp\.prompts"):
            loader.resource_text("no-such-prompt-file.json")

    def test_the_real_files_load(self):
        assert loader.load_record("messages.json", MessageText, MESSAGE_ARGUMENTS) == MessageText(
            **{name: getattr(MESSAGES, name) for name in MESSAGE_ARGUMENTS}
        )


class TestWireDescriptions:
    def test_map_description_carries_the_selection_rule(self):
        description = TOOL_DESCRIPTIONS["repo_map"]

        assert "use grep when the literal string or file is already known" in description
        assert "target location is unknown" in description
        assert "fan-in or blast radius" in description
        assert "change surface spans files" in description

    def test_every_registered_tool_publishes_its_json_description(
        self, extractor, counter, tmp_path
    ):
        # Built with surface=both: the union of the two surfaces is exactly
        # the TOOL_NAMES manifest, so a description without a tool or a tool
        # without a description fails here whichever surface it belongs to.
        root = tmp_path / "alpha"
        root.mkdir()
        (root / "core.py").write_text("def quote(sku):\n    return 1\n", encoding="utf-8")
        services = ServerServices(
            maps=MapService(extractor, counter),
            views=ViewService(extractor),
            symbols=SymbolService(extractor, counter),
            graphs=GraphService(extractor),
            counter=counter,
            extractor=extractor,
        )
        server = build_server(ToolHandlers([root], services), surface=SURFACE_BOTH)

        async def go():
            async with Client(server) as client:
                return await client.list_tools()

        tools = asyncio.run(go())
        assert {tool.name for tool in tools} == set(TOOL_NAMES)
        for tool in tools:
            assert tool.description == TOOL_DESCRIPTIONS[tool.name], tool.name


class TestAPackagedFileThatIsNotUtf8:
    """The sixth defect class, in the module that exists to name the other five.

    `loader`'s docstring enumerates five ways packaged data can be broken and
    says all five raise `PromptDataError`. A prompt file that is present but
    not valid UTF-8 is a sixth in the same class, and it escaped the wrapper:
    `read_text(encoding="utf-8")` raises `UnicodeDecodeError`, which is a
    `ValueError`, and the handler named only `OSError`.
    """

    @pytest.mark.parametrize(
        "error",
        [
            OSError("no such file"),
            UnicodeDecodeError("utf-8", b"\xff", 0, 1, "invalid start byte"),
        ],
        ids=["absent", "not-utf8"],
    )
    def test_it_is_named_rather_than_raised_raw(self, monkeypatch, error):
        traversable = loader.resources.files(loader.PACKAGE)

        def refuse(*_args, **_kwargs):
            raise error

        monkeypatch.setattr(type(traversable.joinpath("x")), "read_text", refuse, raising=False)

        with pytest.raises(PromptDataError, match="cannot be read from"):
            loader.resource_text("messages.json")
