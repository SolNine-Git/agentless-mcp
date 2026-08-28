"""Every string this package shows an agent, held as data.

Tool descriptions, the response envelope's wording and the refusal texts are
prompts: they are read by a model, they are revised for how well they steer
one, and revising them should not mean editing Python. They live in the JSON
files beside this module and are loaded once, here, at import time.

Four files, grouped by consumer:

``tool_descriptions.json``
    The MCP tool descriptions, keyed by tool name. These are the wire
    descriptions -- ``adapters.mcp.server`` passes each one to
    ``@mcp.tool(description=...)``, so the docstrings on those functions are
    code documentation and nothing more.

``parameter_descriptions.json``
    Wire descriptions for parameters shared across tools, keyed by parameter
    name. ``adapters.mcp.server`` carries each one into the published schema,
    where it is the only documentation an arbitrary client is guaranteed to
    see.

``envelope.json``
    The receipt lines, the untrusted-content banner and the truncation
    markers that ``application.envelope`` wraps every answer in.

``messages.json``
    The refusals and the guidance notes: a root or operation refusal, a
    truncation or skipped-file note, a remediation for a cache generation
    that no longer matches. Grouped by what a text says rather than by who
    says it, because the consumers run from the adapter through the
    application services down to ``core.cache`` and an enumerated list of
    them goes stale.

Every value is validated against the keys the code consumes, so a rename in
the JSON fails at startup rather than serving a blank description. Templates
carry ``str.format`` fields and are formatted at the call site; the field sets
are pinned by ``tests/unit/test_prompts.py``.
"""

from collections.abc import Mapping
from dataclasses import dataclass, fields

from agentless_mcp.prompts.loader import PromptDataError, load_mapping, load_record

__all__ = [
    "ENVELOPE",
    "MESSAGES",
    "PARAMETER_DESCRIPTIONS",
    "PARAMETER_NAMES",
    "TOOL_DESCRIPTIONS",
    "TOOL_NAMES",
    "EnvelopeText",
    "MessageText",
    "PromptDataError",
]


@dataclass(frozen=True)
class EnvelopeText:
    """The wording of the envelope every answer is wrapped in."""

    receipt_header: str
    receipt_line: str
    receipt_note: str
    receipt_config: str
    receipt_config_warning: str
    receipt_summary: str
    banner: str
    notice: str
    service_truncation: str
    ceiling_truncation: str
    json_ceiling_untrimmable: str
    json_ceiling_trimmed: str
    json_ceiling_oversized_item: str


@dataclass(frozen=True)
class MessageText:
    """The refusal and guidance texts an agent is answered with."""

    server_no_roots: str
    server_root_required: str
    unknown_operation: str
    op_rejects_parameters: str
    op_requires_parameters: str
    path_needs_endpoints: str
    map_limit_out_of_range: str
    repo_refused_no_roots: str
    repo_refused_not_allowed: str
    roots_file_hint: str
    roots_file_unreadable: str
    map_unresolved_seeds: str
    scan_skipped_files: str
    find_no_matches: str
    find_no_matches_kind: str
    expand_body_truncated: str
    expand_batch_shortened: str
    expand_no_room: str
    grouped_ids: str
    # Not `_omitted_line`'s wording, deliberately. That line ends "more ...
    # not listed", which offers the rest to anyone who raises the budget; a
    # file the focus never reached has no budget that would produce it.
    map_file_unreached: str
    refs_target_unresolved: str
    # Two spellings, because two views need different amounts of it. A
    # grouped view whose rows show `[Class.method]` demonstrates the
    # nesting rule on every line, so it prints the pattern alone. The
    # overview body carries no ids at all, so it keeps the sentence that
    # says how a nested name is spelled.
    stable_ids_pattern: str
    overview_stable_ids: str
    slice_range_beyond_file: str
    slice_range_not_a_range: str
    cache_stale_remediation: str
    cache_stale_refreshing: str
    cache_absent_refreshing: str
    cache_build_hint: str
    cache_discarded_no_index: str
    cache_discarded_old_schema: str
    cache_discarded_other_repo: str


# Every tool this server can register, across both published surfaces: the
# v1 tools first, then the v2 consolidated ones (``find_referencing_symbols``
# and ``capabilities`` are shared by both surfaces). Also the manifest for
# ``tool_descriptions.json``: a tool without a description, or a description
# without a tool, is a startup failure rather than a silent gap.
TOOL_NAMES = (
    "repo_map",
    "list_dir",
    "get_symbols_overview",
    "expand_symbols",
    "read_slice",
    "find_symbol",
    "find_referencing_symbols",
    "explain_symbol",
    "analyze_structure",
    "resolve_locations",
    "capabilities",
    "orient",
    "symbols",
    "read",
)

ENVELOPE: EnvelopeText = load_record(
    "envelope.json", EnvelopeText, tuple(field.name for field in fields(EnvelopeText))
)
MESSAGES: MessageText = load_record(
    "messages.json", MessageText, tuple(field.name for field in fields(MessageText))
)
TOOL_DESCRIPTIONS: Mapping[str, str] = load_mapping("tool_descriptions.json", TOOL_NAMES)

# The parameters shared across tools. Keyed apart from the tool descriptions
# because the manifest there is the tool listing itself.
PARAMETER_NAMES = (
    "repo_root",
    "map_focus",
    "map_budget",
    "map_max_files",
    "map_granularity",
    "no_cache",
    "tree_path",
    "tree_depth",
    "tree_max_entries",
    "overview_paths",
    "docstrings",
    "stable_ids",
    "expand_limit",
    "file_path",
    "slice_lines",
    "context_lines",
    "whole_file",
    "find_name",
    "symbol_kind",
    "find_limit",
    "reference_target",
    "reference_limit",
    "shared_callers",
    "explain_target",
    "explain_limit",
    "structure_operation",
    "path_source",
    "path_target",
    "include_unique",
    "include_ambiguous",
    "structure_limit",
    "community_resolution",
    "diagram_focus",
    "diagram_max_edges",
    "diagram_max_nodes",
    "group_by_communities",
    "locations",
    "orient_operation",
    "symbols_operation",
    "read_operation",
    "orient_focus",
    "orient_limit",
    "symbols_limit",
    "read_path",
)

PARAMETER_DESCRIPTIONS: Mapping[str, str] = load_mapping(
    "parameter_descriptions.json", PARAMETER_NAMES
)
