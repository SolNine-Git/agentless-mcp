"""The response envelope: receipt, untrusted-content banner, output ceiling.

Three things wrap every answer this package produces.

The **receipt** says which repository, at which commit, with how many dirty
files, from which cache generation. It exists so an agent working across a
workspace of repositories can tell a wrong-repository answer and a generation mismatch
from a right one, instead of discovering either through a failed patch. The
two receipt lines are a fixed format, pinned by tests:

    # agentless-mcp receipt
    # repo: /srv/app   head: 1a2b3c4d   dirty: 3 files   cache: none

``cache:`` reads ``none`` when the answer was parsed on demand -- the default
path, and a true statement about the answer rather than a placeholder. With a
tag cache open it names the generation the index was built at and whether that
is still the repository's own generation:

    cache: g:1a2b3c4d fresh
    cache: g:1a2b3c4d generation mismatch (repo g:5e6f7a8b); changed files parse live ...

A mismatched generation is served, not refused: every row it hands back is
checked against the sha256 of the file the view is about, so changed files
cost re-extraction rather than correctness. The receipt says so because an
agent deciding whether to re-index needs the performance signal.

The **banner** marks everything below it as repository data. Rendered source
is untrusted input: a docstring in an analysed repository that says "ignore
your instructions" is a string, and the banner is what keeps it one. Nothing
the analysed repository authored is rendered above it -- the warnings its own
``.agentless-mcp.json`` produced ride below the banner with the truncation
notes, because the region above it is the tool speaking.

The **ceiling** is a hard 16k-token cap on rendered text. Truncation is always
marked, with the counts, so a bounded view is never mistaken for a complete
one -- the failure this package exists to prevent. Every block the ceiling
covers is bounded *before* the body's budget is computed, so no repository
can spend the whole ceiling on the envelope and leave the answer empty.
"""

import json
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Any

from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.prompts import ENVELOPE
from agentless_mcp.util.errors import AtlasError
from agentless_mcp.util.textsafe import one_line
from agentless_mcp.util.tokens import TokenCounter

# What the receipt says when a call carries no symbol source at all: nothing
# was cached, so the answer was parsed on demand.
CACHE_NONE = "none"

DEFAULT_MAX_TOKENS = 16_000

# Room for the truncation marker itself, so adding it cannot push the reply
# back over the ceiling it announces.
_MARKER_TOKEN_ALLOWANCE = 64

# How many of a repository's config warnings the envelope will render. The
# parser caps them too; this is the half that holds whatever reaches here,
# because the receipt is on the critical path of every answer and a receipt
# that grows with repository content is a repository that can empty the body.
MAX_CONFIG_WARNINGS = 8

# Said in place of the warnings that were left out. The counts are the point:
# showing some of the warnings without saying so is the silent truncation
# this module exists to prevent.
CONFIG_WARNINGS_SUPPRESSED = "{shown} of {total} shown; the rest are suppressed"

# No block but the answer may take more than this share of the ceiling. The
# receipt above the banner and the config warnings below it are each clamped
# to it, so a header can never outgrow the answer it introduces.
_BLOCK_TOKEN_SHARE = 8

# The keys the envelope authors. A payload carrying one of them is a service
# bug, refused rather than absorbed: silently shadowing the receipt would
# disable the one field an agent uses to tell a wrong-repository answer from
# a right one.
_RESERVED_KEYS = ("receipt", "notice", "truncated")


@dataclass(frozen=True)
class Truncation:
    """How much of a render was left out, for the marker and the JSON field."""

    shown: int
    total: int
    unit: str


def cache_field(ctx: RepoContext) -> str:
    """Return the ``cache:`` field for one call's receipt."""
    return CACHE_NONE if ctx.symbols is None else ctx.symbols.receipt


def receipt_lines(ctx: RepoContext) -> list[str]:
    """Return the whole receipt block: the tool's own lines, then the warnings.

    A repository carrying a ``.agentless-mcp.json`` says so, and the warnings
    the file produced are printed. Defaults taken from repository content have
    to be visible: an answer shaped by a file the caller never read is the
    thing this line exists to prevent.

    The two halves are separable because :func:`wrap` renders them in
    different regions -- the tool's lines above the untrusted-content banner,
    the repository's warnings below it -- while a caller with no banner to
    place them either side of wants the block whole.
    """
    return [*_tool_lines(ctx), *_warning_lines(_capped_warnings(ctx.config.warnings))]


def _tool_lines(ctx: RepoContext) -> list[str]:
    """Return the receipt lines the tool itself authored: no repository text.

    "No repository text" is the claim; :func:`one_line` is what makes it true.
    Three of the values interpolated here reach us from outside -- the root can
    be a client-advertised directory, the note and the config path come from the
    analysed repository -- and the receipt sits ABOVE the banner that tells an
    agent where trusted framing stops. A newline in any of them forges a second
    ``# NOTE:`` line, which is worse than forging a data row below the banner
    because it can carry free-form directive prose.

    Held here rather than upstream on purpose. ``gitinfo`` and ``projectconfig``
    happen to keep their values single-line today (``splitlines()[0]`` and
    ``{key!r}``), but neither documents that as an envelope precondition, so
    neither can be relied on to keep doing it.
    """
    head = ctx.head_sha or "nogit"
    dirty = "unknown" if ctx.dirty_count is None else str(ctx.dirty_count)
    lines = [
        ENVELOPE.receipt_header,
        ENVELOPE.receipt_line.format(
            root=one_line(str(ctx.root)),
            head=one_line(head),
            dirty=dirty,
            cache=one_line(cache_field(ctx)),
        ),
    ]
    if ctx.note:
        lines.append(ENVELOPE.receipt_note.format(note=one_line(ctx.note)))
    if ctx.config.present:
        lines.append(ENVELOPE.receipt_config.format(path=one_line(str(ctx.config.path))))
    return lines


def _warning_lines(warnings: Sequence[str]) -> list[str]:
    """Return one receipt line per config warning.

    A warning quotes a key from the analysed repository's config, so it is
    repository text on a receipt line and gets the same treatment.
    """
    return [
        ENVELOPE.receipt_config_warning.format(warning=one_line(warning)) for warning in warnings
    ]


def _capped_warnings(warnings: Sequence[str]) -> list[str]:
    """Return the first few warnings, plus a count of whatever was left out."""
    if len(warnings) <= MAX_CONFIG_WARNINGS:
        return list(warnings)
    return [
        *warnings[:MAX_CONFIG_WARNINGS],
        CONFIG_WARNINGS_SUPPRESSED.format(shown=MAX_CONFIG_WARNINGS, total=len(warnings)),
    ]


def receipt_fields(ctx: RepoContext) -> dict[str, Any]:
    """Return the same receipt as structured fields, for JSON responses.

    ``config`` appears only when there was one to report -- a file, or a
    reason one could not be used. The overwhelmingly common case is a
    repository with no config file, and a permanent ``"config": null`` would
    spend every response's tokens saying so. Its warnings are capped here for
    the same reason they are capped in the text receipt: this document is
    emitted whole when it cannot be trimmed, so an uncapped list would be an
    uncapped answer.
    """
    fields: dict[str, Any] = {
        "repo": str(ctx.root),
        "head": ctx.head_sha,
        "tree": ctx.tree_oid,
        "dirty": ctx.dirty_count,
        "cache": cache_field(ctx),
        "note": ctx.note,
    }
    if ctx.config.present or ctx.config.warnings:
        config = ctx.config.as_dict()
        config["warnings"] = _capped_warnings(ctx.config.warnings)
        fields["config"] = config
    return fields


def wrap(
    ctx: RepoContext,
    body: str,
    *,
    counter: TokenCounter,
    max_tokens: int = DEFAULT_MAX_TOKENS,
    truncation: Truncation | None = None,
) -> str:
    """Wrap ``body`` in the receipt and banner, enforcing the output ceiling.

    ``truncation`` is what the *service* already left out (symbols past a
    budget, matches past a limit); the ceiling enforced here is the separate,
    last-resort bound on the rendered text. Both are reported, because a
    reader needs to know that what they are looking at is partial regardless
    of which bound made it so.

    The header and the repository's config warnings are each clamped to a
    share of ``max_tokens`` before the body's budget is computed, and the
    body's budget never goes below zero. Between them the reply cannot exceed
    the ceiling it announces whatever the repository put in its config file.
    """
    header = _header(ctx, counter, max_tokens // _BLOCK_TOKEN_SHARE)
    warnings = _config_warnings(ctx, counter, max_tokens // _BLOCK_TOKEN_SHARE)
    notes: list[str] = []
    if truncation is not None and truncation.shown < truncation.total:
        notes.append(
            ENVELOPE.service_truncation.format(
                shown=truncation.shown, total=truncation.total, unit=truncation.unit
            )
        )

    budget = max_tokens - counter.count(header) - counter.count(warnings)
    kept, dropped = _fit(body, counter, max(budget - _MARKER_TOKEN_ALLOWANCE, 0))
    if dropped:
        notes.append(
            ENVELOPE.ceiling_truncation.format(
                max_tokens=max_tokens, dropped=dropped, total=len(body.splitlines())
            )
        )

    pieces = [header, kept if kept.endswith("\n") or not kept else kept + "\n", warnings]
    if notes:
        pieces.append("\n".join(notes) + "\n")
    return "".join(pieces)


def _header(ctx: RepoContext, counter: TokenCounter, budget: int) -> str:
    """Render the tool-authored receipt and the banner, clamped to ``budget``.

    The banner is rendered whatever the clamp dropped above it: a bounded
    answer that lost its untrusted-content marker would be the worse failure
    of the two.
    """
    block = "".join(f"{line}\n" for line in _tool_lines(ctx))
    kept, _ = _fit(block, counter, budget)
    return f"{kept}{ENVELOPE.banner}\n"


def _config_warnings(ctx: RepoContext, counter: TokenCounter, budget: int) -> str:
    """Render the repository's own config warnings, bounded by count and size.

    Below the banner, because the warning text is quoted from a file the
    analysed repository wrote. A warning left out is counted in the line that
    replaces it, so the block is never quietly shorter than the truth.
    """
    total = len(ctx.config.warnings)
    if not total:
        return ""

    lines = _warning_lines(ctx.config.warnings[:MAX_CONFIG_WARNINGS])
    kept, dropped = _fit("".join(f"{line}\n" for line in lines), counter, budget)
    shown = len(lines) - dropped
    if shown == total:
        return kept

    suppressed = CONFIG_WARNINGS_SUPPRESSED.format(shown=shown, total=total)
    return f"{kept}{ENVELOPE.receipt_config_warning.format(warning=suppressed)}\n"


def wrap_json(
    ctx: RepoContext,
    payload: Mapping[str, Any],
    *,
    counter: TokenCounter,
    max_tokens: int = DEFAULT_MAX_TOKENS,
    items_key: str | None = None,
) -> str:
    """Render a JSON response carrying the same receipt fields structurally.

    A JSON document cannot be cut off mid-way and stay parseable, so the
    ceiling is enforced by dropping whole items from the list named by
    ``items_key`` and recording what was dropped in a ``truncated`` field.
    Without an ``items_key`` an oversized payload is emitted whole with that
    field set: an honest oversized answer beats a silently mangled one.

    The envelope authors ``receipt``, ``notice`` and ``truncated`` and nothing
    else may: a payload naming one of them is refused rather than merged, so
    no service can shadow the fields an agent reads to tell a
    wrong-repository or a partial answer from a complete one.
    """
    collisions = [key for key in _RESERVED_KEYS if key in payload]
    if collisions:
        message = (
            f"payload keys collide with the envelope's own: {', '.join(collisions)}. "
            "Rename the field: the envelope owns these keys."
        )
        raise AtlasError(message)

    # No key can be overwritten once the collisions above are refused, so the
    # envelope's own fields stay first, where every reader of this format --
    # goldens included -- already expects to find them.
    document: dict[str, Any] = {
        "receipt": receipt_fields(ctx),
        "notice": ENVELOPE.notice,
        **payload,
    }
    rendered = _dump(document)
    if counter.count(rendered) <= max_tokens:
        return rendered

    items = document.get(items_key) if items_key else None
    if not isinstance(items, list):
        document["truncated"] = {
            "reason": ENVELOPE.json_ceiling_untrimmable.format(max_tokens=max_tokens),
            "token_ceiling": max_tokens,
            "tokens": counter.count(rendered),
        }
        return _dump(document)

    kept = _fit_items(document, items, items_key or "", counter, max_tokens)
    document[items_key or ""] = items[:kept]
    document["truncated"] = _json_truncation(max_tokens, kept, len(items))
    return _dump(document)


def _dump(document: Mapping[str, Any]) -> str:
    """Render one JSON document, stably ordered and newline-terminated."""
    return json.dumps(document, indent=2) + "\n"


def _json_truncation(max_tokens: int, shown: int, total: int) -> dict[str, Any]:
    """Return the metadata that must fit beside every trimmed JSON list."""
    return {
        "reason": ENVELOPE.json_ceiling_trimmed.format(max_tokens=max_tokens),
        "token_ceiling": max_tokens,
        "shown": shown,
        "total": total,
    }


def _fit(body: str, counter: TokenCounter, budget: int) -> tuple[str, int]:
    """Return the longest whole-line prefix of ``body`` within ``budget``."""
    if budget <= 0:
        return "", len(body.splitlines())
    if counter.count(body) <= budget:
        return body, 0

    lines = body.splitlines(keepends=True)
    low, high = 0, len(lines)
    while low < high:
        middle = (low + high + 1) // 2
        if counter.count("".join(lines[:middle])) <= budget:
            low = middle
        else:
            high = middle - 1
    return "".join(lines[:low]), len(lines) - low


def _fit_items(
    document: dict[str, Any],
    items: list[Any],
    items_key: str,
    counter: TokenCounter,
    max_tokens: int,
) -> int:
    """Return the largest item count whose rendered document fits the ceiling."""
    low, high = 0, len(items)
    while low < high:
        middle = (low + high + 1) // 2
        probe = dict(document)
        probe[items_key] = items[:middle]
        probe["truncated"] = _json_truncation(max_tokens, middle, len(items))
        if counter.count(_dump(probe)) <= max_tokens:
            low = middle
        else:
            high = middle - 1
    return low
