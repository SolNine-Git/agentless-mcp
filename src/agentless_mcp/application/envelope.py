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

Two receipts, and only one of them is a contract. The text block
:func:`receipt_lines` returns is for a person reading a terminal: it is
positional, lines appear and disappear with the repository, and a field added
to it moves every line below. The structural receipt :func:`receipt_fields`
returns is the one to parse -- its fields are named, and a new one is added
beside the others rather than in front of them. An agent that reads the text
form by line index is reading a human-facing rendering.

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
from agentless_mcp.util.errors import OperationFailed
from agentless_mcp.util.textsafe import one_line
from agentless_mcp.util.tokens import Chars4Counter, TokenCounter

DEFAULT_MAX_TOKENS = 16_000

# What the two receipt builders bound their warnings with when the caller has
# no counter to lend them. The receipt is rendered on paths that never see the
# ceiling -- a CLI stderr note, a report assembled by another service -- and
# those paths still may not let a repository's config file grow without limit.
# The default estimator rather than a character rule, so one selection answers
# for every path.
_ESTIMATOR = Chars4Counter()

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


def receipt_lines(
    ctx: RepoContext,
    *,
    counter: TokenCounter | None = None,
    max_tokens: int = DEFAULT_MAX_TOKENS,
) -> list[str]:
    """Return the whole receipt block: the tool's own lines, then the warnings.

    A repository carrying a ``.agentless-mcp.json`` says so, and the warnings
    the file produced are printed. Defaults taken from repository content have
    to be visible: an answer shaped by a file the caller never read is the
    thing this line exists to prevent.

    Human-facing and positional: read :func:`receipt_fields` to parse a
    receipt. The two halves are separable because :func:`wrap` renders them in
    different regions -- the tool's lines above the untrusted-content banner,
    the repository's warnings below it -- while a caller with no banner to
    place them either side of wants the block whole.
    """
    warnings = _bounded_warnings(ctx.config.warnings, counter, max_tokens)
    return [*_tool_lines(ctx), *_warning_lines(warnings)]


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
            cache=one_line(ctx.cache_receipt),
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


def _bounded_warnings(
    warnings: Sequence[str], counter: TokenCounter | None, max_tokens: int
) -> list[str]:
    """Return the warnings that fit both bounds, plus a count of the rest.

    One selection for both receipts, because the same call renders both. They
    bounded the same list differently before -- by count alone in the receipt
    builders, by count and token budget in the wrapped body -- so a CLI call
    that printed a stderr receipt beside a wrapped body could report a
    different ``shown`` count in each.

    The count bound alone is also not a bound. ``MAX_CONFIG_WARNINGS`` counts
    entries and an entry is repository-sized: a single 65 kB unknown key in a
    ``.agentless-mcp.json`` passed it eight times over, spent the whole
    ceiling on the envelope, and left the answer empty -- the failure the
    module docstring says cannot happen. The size bound is what makes that
    sentence true on both paths.
    """
    total = len(warnings)
    if not total:
        return []

    counted = counter if counter is not None else _ESTIMATOR
    candidates = list(warnings[:MAX_CONFIG_WARNINGS])
    block = "".join(f"{line}\n" for line in _warning_lines(candidates))
    _, dropped = _fit(block, counted, max_tokens // _BLOCK_TOKEN_SHARE)
    shown = len(candidates) - dropped
    if shown == total:
        return candidates
    return [
        *candidates[:shown],
        CONFIG_WARNINGS_SUPPRESSED.format(shown=shown, total=total),
    ]


def receipt_fields(
    ctx: RepoContext,
    *,
    counter: TokenCounter | None = None,
    max_tokens: int = DEFAULT_MAX_TOKENS,
) -> dict[str, Any]:
    """Return the same receipt as structured fields, for JSON responses.

    This is the receipt to parse: every field is named, and a field added
    later arrives beside the others instead of moving them.

    ``notice`` rides in the receipt rather than beside it because the marker
    is a property of the response, not of one wrapper function.
    :func:`wrap_json` used to be the only thing that added it, so a response
    another service assembled from these fields carried the repository
    framing -- which repository, which commit, which config file -- and no
    untrusted-content marker at all, with nothing at the call site to say
    which of the two shapes it had built.

    ``config`` appears only when there was one to report -- a file, or a
    reason one could not be used. The overwhelmingly common case is a
    repository with no config file, and a permanent ``"config": null`` would
    spend every response's tokens saying so. Its warnings are bounded by the
    same selection the text receipt uses, by count and by size: this document
    is emitted whole when it cannot be trimmed, so an unbounded list is an
    unbounded answer.
    """
    fields: dict[str, Any] = {
        "repo": str(ctx.root),
        "head": ctx.head_sha,
        "tree": ctx.tree_oid,
        "dirty": ctx.dirty_count,
        "cache": ctx.cache_receipt,
        "note": ctx.note,
        "notice": ENVELOPE.notice,
    }
    if ctx.config.present or ctx.config.warnings:
        config = ctx.config.as_dict()
        config["warnings"] = _bounded_warnings(ctx.config.warnings, counter, max_tokens)
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
    warnings = _config_warnings(ctx, counter, max_tokens)
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


def _config_warnings(ctx: RepoContext, counter: TokenCounter, max_tokens: int) -> str:
    """Render the repository's own config warnings, bounded by count and size.

    Below the banner, because the warning text is quoted from a file the
    analysed repository wrote. A warning left out is counted in the line that
    replaces it, so the block is never quietly shorter than the truth -- and
    the selection is :func:`_bounded_warnings`, the one both receipts use, so
    the count this block reports is the count they report.
    """
    lines = _warning_lines(_bounded_warnings(ctx.config.warnings, counter, max_tokens))
    return "".join(f"{line}\n" for line in lines)


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
        raise OperationFailed(message)

    # No key can be overwritten once the collisions above are refused, so the
    # envelope's own fields stay first, where every reader of this format --
    # goldens included -- already expects to find them.
    receipt = receipt_fields(ctx, counter=counter, max_tokens=max_tokens)
    document: dict[str, Any] = {
        "receipt": receipt,
        # The same string the receipt carries, read from it rather than
        # restated: one owner, so the two cannot drift.
        "notice": receipt["notice"],
        **payload,
    }
    rendered = _dump(document)
    if counter.count(rendered) <= max_tokens:
        return rendered

    items = document.get(items_key) if items_key else None
    # `items_key is None` is what narrows it to `str` below; a caller passing
    # no key already reaches this branch through the `items` above.
    if items_key is None or not isinstance(items, list):
        document["truncated"] = {
            "reason": ENVELOPE.json_ceiling_untrimmable.format(max_tokens=max_tokens),
            "token_ceiling": max_tokens,
            "tokens": counter.count(rendered),
        }
        return _dump(document)

    kept = _fit_items(document, items, items_key, counter, max_tokens)
    document[items_key] = items[:kept]
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
