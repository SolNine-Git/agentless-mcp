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
import re
from string import Formatter
from typing import Any

import pytest
from fastmcp import Client

from agentless_mcp.adapters.mcp.cliargs import SURFACE_BOTH
from agentless_mcp.adapters.mcp.server import (
    ServerServices,
    ToolHandlers,
    build_server,
)
from agentless_mcp.application.graph_service import GraphService
from agentless_mcp.application.history_service import HistoryService
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
    "receipt_summary": {"summary": "12 files, 3 skipped"},
    "notice": {},
    "service_truncation": {"shown": 12, "total": 40, "unit": "symbols"},
    "ceiling_truncation": {"max_tokens": 16_000, "dropped": 7, "total": 900},
    "json_ceiling_untrimmable": {"max_tokens": 16_000},
    "json_ceiling_trimmed": {"max_tokens": 16_000},
    "json_ceiling_oversized_item": {"max_tokens": 16_000},
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
    "path_needs_endpoints": {"tool": "orient"},
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
    "find_no_matches": {"query": "rotate_age"},
    "find_no_matches_kind": {"kind": "class", "query": "quote", "kinds": "function, method"},
    "find_unparsed": {"query": "unfinished", "listed": "core.py:4"},
    "expand_body_truncated": {"shown": 12, "total": 340},
    "expand_batch_shortened": {"shortened": 3, "total": 10, "budget": 12_000},
    "expand_no_room": {"requested": 400, "seats": 40},
    "grouped_ids": {"count": 12},
    "map_file_unreached": {"count": 9},
    "refs_target_unresolved": {"target": "py:src/app/svc.py::Invoice.total", "name": "total"},
    "explain_tie": {"count": 2, "target": "shared"},
    "stable_ids_pattern": {"pattern": "py:src/app/svc.py::<QualifiedName>"},
    "overview_stable_ids": {"pattern": "py:src/app/svc.py::<QualifiedName>"},
    "slice_range_beyond_file": {"start": 9000, "end": 9050, "path": "src/app/svc.py", "total": 242},
    "slice_range_not_a_range": {"start": 60, "end": 30, "path": "src/app/svc.py"},
    "cache_stale_remediation": {"repo_root": "/srv/app"},
    "cache_stale_refreshing": {},
    "cache_absent_refreshing": {},
    "cache_build_hint": {"repo_root": "/srv/app"},
    "cache_discarded_no_index": {},
    "cache_discarded_old_schema": {"found": 1, "expected": 2},
    "cache_discarded_other_repo": {"repo_root": "/srv/other"},
    "history_target_unresolved": {"target": "quote", "reason": "core.py no longer defines quote"},
    "history_no_git": {"note": "/srv/app is not inside a git repository"},
    "history_path_not_in_head": {"path": "src/app/svc.py"},
    "history_span_beyond_head": {"path": "src/app/svc.py", "start": 120, "end": 158},
    "history_git_failed": {"note": "git log timed out after 30.0s"},
    "history_git_output_malformed": {"detail": "commit 2 does not open with a full sha"},
    "history_no_commits": {"path": "src/app/svc.py", "start": 120, "end": 158},
    "history_more_commits": {"shown": 10},
    "history_seats_capped": {"shown": 120, "path": "src/app/svc.py", "start": 120, "end": 158},
    "history_body_truncated": {"shown": 12, "total": 40, "sha": "a1b2c3d4"},
    "history_output_capped": {"bytes": 2_000_000, "count": 7},
    "history_output_capped_no_commits": {
        "bytes": 2_000_000,
        "path": "src/app/svc.py",
        "start": 120,
        "end": 158,
    },
    "history_dirty_file": {"path": "src/app/svc.py", "start": 120, "end": 158},
    "history_symbol_not_in_head": {"name": "quote", "path": "src/app/svc.py"},
    "history_signature_changed": {"name": "quote"},
    "history_shallow": {},
    "history_shallow_unknown": {"note": "git rev-parse timed out after 30.0s"},
    "history_dirty_unknown": {
        "path": "src/app/svc.py",
        "note": "git diff timed out after 5.0s",
        "start": 120,
        "end": 158,
    },
    "complexity_language_unsupported": {
        "path": "config.yaml",
        "language": "yaml",
        "languages": "bash, c, python",
    },
    "complexity_does_not_parse": {"path": "src/app/svc.py", "line": 12},
    "complexity_tokenizer_rejects": {
        "path": "src/app/svc.py",
        "detail": "IndentationError: unindent does not match any outer indentation level",
    },
    "complexity_grew": {
        "qualname": "register_search_tools",
        "old_cc": 16,
        "new_cc": 17,
        "old_sloc": 194,
        "new_sloc": 201,
        "threshold": 10,
    },
    "complexity_crossed": {
        "qualname": "Store.find",
        "old_cc": 10,
        "new_cc": 12,
        "old_sloc": 30,
        "new_sloc": 36,
        "threshold": 10,
    },
    "complexity_new_unit": {
        "qualname": "parse_rows",
        "new_cc": 14,
        "new_sloc": 40,
        "threshold": 10,
    },
    "complexity_evidence": {
        "old_cc": 16,
        "new_cc": 17,
        "old_sloc": 194,
        "new_sloc": 201,
        "threshold": 10,
    },
    "complexity_new_unit_evidence": {"new_cc": 14, "new_sloc": 40, "threshold": 10},
    "complexity_old_not_measured": {
        "path": "src/app/svc.py",
        "reason": "src/app/svc.py does not parse: the first syntax error is at line 12.",
    },
    "complexity_new_not_measured": {
        "path": "src/app/svc.py",
        "reason": "src/app/svc.py does not parse: the first syntax error is at line 12.",
    },
    "pair_old_unread": {
        "path": "src/app/big.py",
        "reason": "skipped: 1200000 bytes exceeds the per-file cap of 1000000 bytes",
    },
    "pair_new_unread": {"path": "src/app/svc.py", "reason": "unreadable: No such file"},
    "pair_edits_unapplied": {"failed": 1, "path": "src/app/svc.py"},
    "lint_unknown_check": {"name": "complexity", "checks": "arity, complexity_delta"},
    "lint_no_checks": {"checks": "arity, complexity_delta"},
    "lint_path_needs_after": {},
    "lint_path_takes_pair_checks": {"pair_checks": "complexity_delta", "checks": "arity"},
    "lint_since_alone": {},
    "lint_since_takes_pair_checks": {"pair_checks": "complexity_delta", "checks": "arity"},
    "lint_revision_unresolved": {
        "revision": "HEAD~5",
        "note": "git rev-parse exited 128: fatal: Needed a single revision",
    },
    "lint_revision_not_resolved": {
        "revision": "HEAD~5",
        "note": "git is not installed, so repository state is unknown",
    },
    "lint_revision_unlisted": {"command": "diff", "note": "git diff timed out after 30.0s"},
    "lint_revision_listing_capped": {"command": "diff", "bytes": 8_000_000},
    "lint_revision_output_malformed": {"detail": "a diff record ends before its path"},
    "lint_revision_unchanged": {"revision": "HEAD"},
    "lint_revision_unchanged_evidence": {},
    "lint_revision_submodule": {"path": "vendor/lib"},
    "lint_revision_submodule_evidence": {},
    "lint_revision_binary": {"path": "src/app/blob.py"},
    "lint_revision_binary_evidence": {"bytes": 8_000},
    "lint_revision_status": {"path": "src/app/svc.py", "status": "U"},
    "lint_revision_status_evidence": {"status": "U"},
    "pair_worktree_unread": {"path": "src/app/svc.py", "reason": "unreadable: No such file"},
    "pair_blob_over_cap": {"cap": 1_000_000},
    "config_erosion_gate_not_object": {
        "config": ".agentless-mcp.json",
        "modes": "off, advisory, block",
    },
    "config_erosion_gate_mode": {
        "check": "complexity_delta",
        "config": ".agentless-mcp.json",
        "modes": "off, advisory, block",
    },
    "config_erosion_gate_unknown": {
        "config": ".agentless-mcp.json",
        "count": 2,
        "checks": "complexity_delta, contract_drift, clone_delta",
    },
    "contract_no_boundary": {"config": ".agentless-mcp.json"},
    "contract_no_boundary_evidence": {},
    "contract_no_rules": {"language": "bash", "path": "src/adapters/run.sh"},
    "contract_old_not_measured": {
        "path": "src/adapters/api.py",
        "reason": "src/adapters/api.py does not parse",
    },
    "contract_new_not_measured": {
        "path": "src/adapters/api.py",
        "reason": "src/adapters/api.py does not parse",
    },
    "contract_new_hit": {
        "unit": "parse_items",
        "capture": 'data.get("items", [])',
        "rule": "get-default-empty",
        "why": "A missing key reads as an empty value.",
    },
    "contract_evidence": {
        "rule": "get-default-empty",
        "unit": "parse_items",
        "capture": 'data.get("items", [])',
    },
    "clone_language_unsupported": {
        "path": "notes.md",
        "language": "markdown",
        "languages": "bash, python",
    },
    "clone_does_not_parse": {"path": "src/app/svc.py", "line": 12},
    "clone_tokenizer_rejects": {"path": "src/app/svc.py", "detail": "TokenError: EOF"},
    "clone_old_not_indexed": {"path": "src/app/svc.py", "reason": "src/app/svc.py does not parse"},
    "clone_new_not_indexed": {"path": "src/app/svc.py", "reason": "src/app/svc.py does not parse"},
    "clone_index_gaps": {"count": 2, "total": 40, "paths": "a.py, b.py"},
    "clone_index_gaps_evidence": {"path": "a.py", "reason": "skipped: over the cap"},
    "clone_index_unlisted": {"reason": "walk refused: more than 20000 files"},
    "clone_index_stopped": {},
    "clone_new_copy": {"start": 10, "end": 14, "count": 2},
    "clone_new_copy_evidence": {"sites": "src/app/a.py:4"},
    "clone_and_more": {"listed": "a.py:4, b.py:9, c.py:2", "count": 3},
    "config_boundary_globs_not_list": {"config": ".agentless-mcp.json", "limit": 64, "chars": 256},
    "config_boundary_globs_too_many": {"config": ".agentless-mcp.json", "count": 65, "limit": 64},
    "config_boundary_globs_dropped": {"config": ".agentless-mcp.json", "count": 2, "chars": 256},
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
            record.notice = "x"

    def test_the_envelope_carries_the_documented_wording(self):
        assert ENVELOPE.receipt_header == "// agentless-mcp receipt (repository data below)"
        assert ENVELOPE.notice in ENVELOPE.receipt_header


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

    @pytest.mark.parametrize(
        ("record", "table"),
        [(ENVELOPE, ENVELOPE_ARGUMENTS), (MESSAGES, MESSAGE_ARGUMENTS)],
        ids=["envelope", "messages"],
    )
    def test_no_catalog_line_renders_as_a_markdown_heading(self, record, table):
        """Tool framing must not open a line a Markdown client reads as a heading.

        0.7.0 moved the receipt marker from ``#`` to ``//`` because CommonMark
        reads up to three spaces of indent, one to six ``#`` and a space -- or
        the end of the line -- as an ATX heading, so every receipt rendered
        heading-sized in a client that treats tool output as Markdown. That is
        a property of every catalog string, not of the receipt alone: any
        framing line that opens with ``#`` is the same defect. The map's
        rationale rows keep ``#`` legitimately -- they quote source comments
        and carry enough indent that Markdown reads them as code -- and they
        are rendered from repository text, not from these catalogs.
        """
        atx_heading = re.compile(r"^ {0,3}#{1,6}(?:[ \t]|$)")
        for name, arguments in sorted(table.items()):
            rendered = getattr(record, name).format(**arguments)
            for line in rendered.splitlines():
                assert not atx_heading.match(line), name


class TestEagerValidation:
    """Every defect below is raised while the package is imported."""

    def test_a_missing_key_is_refused_by_name(self):
        document = json.loads(ENVELOPE_TEXT)
        del document["notice"]
        with pytest.raises(PromptDataError, match=r"missing \['notice'\]"):
            loader.build_record(
                "envelope.json", json.dumps(document), EnvelopeText, ENVELOPE_ARGUMENTS
            )

    def test_a_key_no_code_consumes_is_refused_by_name(self):
        document = json.loads(ENVELOPE_TEXT)
        document["notiice"] = "typo"
        with pytest.raises(PromptDataError, match=r"unknown \['notiice'\]"):
            loader.build_record(
                "envelope.json", json.dumps(document), EnvelopeText, ENVELOPE_ARGUMENTS
            )

    def test_a_blank_value_is_refused(self):
        document = json.loads(ENVELOPE_TEXT)
        document["notice"] = "   "
        with pytest.raises(PromptDataError, match="must be a non-empty string"):
            loader.build_record(
                "envelope.json", json.dumps(document), EnvelopeText, ENVELOPE_ARGUMENTS
            )

    def test_a_non_string_value_is_refused(self):
        document = json.loads(ENVELOPE_TEXT)
        document["notice"] = 3
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
        assert "change surface spans files" in description
        # The map used to claim fan-in and blast radius as well, which is the
        # question find_referencing_symbols publishes. Two tools answering one
        # question with no tiebreak is the one defect these strings exist to
        # prevent, so the clause belongs to the reference tool alone.
        assert "fan-in" not in description
        assert "blast radius" not in description
        assert "Fan-in for blast radius" in TOOL_DESCRIPTIONS["find_referencing_symbols"]

    def test_the_v2_surface_advertises_rationale_nodes(self):
        # v2 is the default surface and routes to the same handlers, so a
        # feature documented only on the retired v1 tools is unreachable
        # documentation for every agent that reads the published schema.
        assert "rationale nodes" in TOOL_DESCRIPTIONS["orient"]
        assert "rationale comments" in TOOL_DESCRIPTIONS["symbols"]

    def test_both_surfaces_say_what_the_health_complexity_sections_do_not_answer(self):
        for name in ("orient", "analyze_structure"):
            assert "hotspots" in TOOL_DESCRIPTIONS[name], name
            assert "not judge design" in TOOL_DESCRIPTIONS[name], name
        for name in ("orient_limit", "structure_operation"):
            assert "hotspots" in PARAMETER_DESCRIPTIONS[name], name

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
            histories=HistoryService(extractor, counter),
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
