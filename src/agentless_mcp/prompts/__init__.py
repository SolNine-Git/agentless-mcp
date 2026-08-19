"""Every string this package shows an agent, held as data.

Tool descriptions, the response envelope's wording and the refusal texts are
prompts: they are read by a model, they are revised for how well they steer
one, and revising them should not mean editing Python. They live in the JSON
files beside this module and are loaded once, here, at import time.

Three files, grouped by consumer:

``tool_descriptions.json``
    The MCP tool descriptions, keyed by tool name. These are the wire
    descriptions -- ``adapters.mcp.server`` passes each one to
    ``@mcp.tool(description=...)``, so the docstrings on those functions are
    code documentation and nothing more.

``envelope.json``
    The receipt lines, the untrusted-content banner and the truncation
    markers that ``application.envelope`` wraps every answer in.

``messages.json``
    The refusal and guidance texts: the allowlist refusals raised by the
    server and by ``application.repo_context``, and the cache-staleness
    remediation ``core.cache`` renders into the receipt.

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
    banner: str
    notice: str
    service_truncation: str
    ceiling_truncation: str
    json_ceiling_untrimmable: str
    json_ceiling_trimmed: str


@dataclass(frozen=True)
class MessageText:
    """The refusal and guidance texts an agent is answered with."""

    server_no_roots: str
    server_root_required: str
    repo_refused_no_roots: str
    repo_refused_not_allowed: str
    cache_stale_remediation: str
    cache_discarded_no_index: str
    cache_discarded_old_schema: str
    cache_discarded_other_repo: str


# The tools this server registers. Also the manifest for
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
    "symbol_path",
    "import_cycles",
    "resolve_locations",
    "capabilities",
)

ENVELOPE: EnvelopeText = load_record(
    "envelope.json", EnvelopeText, tuple(field.name for field in fields(EnvelopeText))
)
MESSAGES: MessageText = load_record(
    "messages.json", MessageText, tuple(field.name for field in fields(MessageText))
)
TOOL_DESCRIPTIONS: Mapping[str, str] = load_mapping("tool_descriptions.json", TOOL_NAMES)
