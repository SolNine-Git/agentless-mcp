"""Code-shaped renderers and the view models the services fill in.

Two research constraints decide everything in this module. Flattened,
code-shaped text beats structured dumps for localization, so a map is rendered
as numbered signature lines under a filename rather than as a table. And
denormalized "incident cards" carrying names, not opaque handles, measurably
beat id-only encodings. Stable ids already carry the repository-relative path,
so text locations append only ``@line`` or ``@start-end`` rather than spelling
that path twice.

The view models live here rather than in the services because this module owns
the output vocabulary: a service decides *what* is worth showing, this decides
what it looks like. Nothing here reads the filesystem or parses anything.

Every path is repository-relative with forward slashes, and every navigable
row carries a stable id plus an exact line or span.

**This module owns the line grammar, so it is where repository text is made
safe for a row.** Every field that carries a repository-derived value --
paths, signatures, stable ids, qualified names, module strings, rationale
text -- goes through :func:`agentless_mcp.util.textsafe.one_line` at the
point it is placed on a line. Not at the source: a newline is legal in a
POSIX filename, so refusing such a repository would be worse than rendering
it safely, and a value escaped upstream and again here would come out
double-escaped. Entry points reject, the sink escapes, nothing normalises in
between.

The rule is worth restating because it failed once. Reproduced against the
working tree: a repository containing a file named ``a\n    42|
forged_symbol  [py:trusted.py::admin]\nb.py`` rendered a byte-identical
structural row directly below the line that tells an agent where trusted
framing stops. ``tests/unit/test_render.py`` walks every view model's string
fields by reflection and asserts that none of them can add a line, so a field
added later is covered without anyone remembering to cover it.

Two other modules render line-oriented answers and carry the same rule:
:func:`agentless_mcp.core.treewalk.render_tree` and
:mod:`agentless_mcp.application.envelope`. ``core/mermaid`` has its own
stricter escape because its grammar is not this one.
"""

from abc import ABC, abstractmethod
from collections.abc import Sequence
from dataclasses import dataclass, field
from typing import Any, overload

from agentless_mcp.core.refs import SkippedFile
from agentless_mcp.core.slices import line_prefix
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util.textsafe import one_line


def _no_negative_zero(value: float) -> float:
    """Return ``value`` with a rounded-away negative zero flattened to zero.

    A modularity that is mathematically zero comes out of the summation as a
    tiny negative float whose sign is accumulation noise, and the exact noise
    differs between interpreters. Formatted, that is the difference between
    ``0.000`` and ``-0.000`` in a byte-exact golden -- a diff that says the
    partition changed when only the floating-point rounding did.
    """
    return value + 0.0 if value == 0 else value


MODULE_LEVEL = "(module level)"

# How wide the line-number column of a row-per-line view is. The prefix
# itself is `core.slices.line_prefix`, which every line-numbered view in the
# package renders through.
LINE_NUMBER_WIDTH = 5

# How many of a candidate's shared callers the text render lists before
# cutting to a count. The callers are evidence for the overlap number, and a
# handful proves it; the full list is what let one adjacency listing approach
# the response ceiling on its own. The JSON form still carries every caller.
SHARED_CALLERS_SHOWN = 5

# How many skipped files the warning line names before cutting to a count,
# for the same reason: the warning is evidence that the scan was partial, not
# an inventory. The JSON forms still carry every entry.
SKIPPED_FILES_SHOWN = 5

# The markdown fence a diagram travels in when it is going into a response
# body. Declared here rather than in `core.mermaid` because fencing is a
# property of the destination, not of the diagram.
FENCE = "```"
MERMAID_FENCE = FENCE + "mermaid"
WEAK_MODULARITY_THRESHOLD = 0.3


class _Bounded(ABC):
    """The arithmetic every bounded listing announces its cut with.

    A listing carries the pre-slice ``total`` and says how much of it it
    ``shown``; ``omitted`` is the difference. It lives here rather than being
    spelled out on each value object because it is the one number that keeps a
    bounded view from being read as a complete one, and six copies of it would
    be six chances to compute it against the wrong denominator.
    """

    total: int

    @property
    @abstractmethod
    def shown(self) -> int:
        """How much of ``total`` this listing actually carries."""

    @property
    def omitted(self) -> int:
        """How much of ``total`` the limit left out."""
        return max(0, self.total - self.shown)


@dataclass(frozen=True)
class RationaleNode:
    """One rationale comment and the symbol node it is linked to."""

    stable_id: str
    parent_id: str
    line: int
    kind: str
    text: str
    citations: tuple[str, ...] = ()

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this rationale node."""
        return {
            "stable_id": self.stable_id,
            "parent_id": self.parent_id,
            "line": self.line,
            "kind": self.kind,
            "text": self.text,
            "citations": list(self.citations),
        }


@dataclass(frozen=True)
class MapEntry:
    """One symbol line in a repository map."""

    line: int
    signature: str
    stable_id: str
    depth: int = 0
    rationales: tuple[RationaleNode, ...] = ()

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this entry."""
        record: dict[str, Any] = {
            "line": self.line,
            "signature": self.signature,
            "stable_id": self.stable_id,
            "depth": self.depth,
        }
        if self.rationales:
            record["rationales"] = [rationale.as_dict() for rationale in self.rationales]
        return record


@dataclass(frozen=True)
class MapFile:
    """One ranked file in a repository map, with the symbols that fit."""

    path: str
    rank: float
    entries: tuple[MapEntry, ...] = ()
    omitted: int = 0

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this file."""
        return {
            "path": self.path,
            "rank": round(self.rank, 6),
            "symbols": [entry.as_dict() for entry in self.entries],
            "omitted": self.omitted,
        }


@dataclass(frozen=True)
class SymbolCard:
    """A denormalized symbol record: everything a reader needs, in one place.

    ``body_shown`` and ``body_total`` are the line counts behind a body that
    was cut to fit a budget. They are line counts of the *symbol*, not of the
    rendered text, and they appear in JSON only when they disagree -- a card
    that says nothing about truncation is a card that was not truncated.
    """

    stable_id: str
    path: str
    start_line: int
    end_line: int
    kind: str
    language: str
    signature: str
    parent_class: str = ""
    body: str = ""
    body_shown: int = 0
    body_total: int = 0

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this card."""
        record: dict[str, Any] = {
            "stable_id": self.stable_id,
            "path": self.path,
            "start_line": self.start_line,
            "end_line": self.end_line,
            "kind": self.kind,
            "language": self.language,
            "signature": self.signature,
            "parent_class": self.parent_class,
        }
        if self.body:
            record["body"] = self.body
        if self.body_shown < self.body_total:
            record["body_truncated"] = {"lines_shown": self.body_shown, "lines": self.body_total}
        return record


@dataclass(frozen=True)
class RefSite:
    """One reference, attributed to the symbol whose body contains it."""

    line: int
    enclosing: str
    stable_id: str | None = None

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this reference site."""
        return {"line": self.line, "enclosing": self.enclosing, "stable_id": self.stable_id}


@dataclass(frozen=True)
class RefGroup:
    """The references to one symbol from one file.

    ``tier`` says what kind of evidence connects this file to the target: an
    import it declares, a definition in the file itself, the fact that the
    name is unique in the repository, or nothing but the spelling. Fan-in
    still lists every name match -- a missed caller is the expensive error --
    but the label is what lets a reader weigh the rows instead of trusting
    them equally.
    """

    path: str
    sites: tuple[RefSite, ...] = field(default_factory=tuple)
    tier: str = ""
    tier_label: str = ""

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this group."""
        return {
            "path": self.path,
            "count": len(self.sites),
            "tier": self.tier,
            "sites": [site.as_dict() for site in self.sites],
        }


@dataclass(frozen=True)
class RefListing(_Bounded, Sequence[RefGroup]):
    """The fan-in groups the limit kept, and the sites and files it left out.

    A ``Sequence`` for the reason :class:`SharedCallerListing` is one: the
    renderer receives nothing but this object and the target name, so a total
    that lived anywhere else would be a truncation the text render could not
    announce -- and a missed caller is the expensive error this view exists to
    prevent.

    The sites are cut *before* they are grouped, so a file whose every site
    fell past the limit is absent from the listing altogether. ``files``
    carries the count before that cut, because "and every reference in 21
    more files" is what a blast-radius reader has to be told.
    """

    rows: tuple[RefGroup, ...] = ()
    total: int = 0
    limit: int = 0
    files: int = 0

    @property
    def shown(self) -> int:
        """How many reference sites the kept groups carry between them."""
        return sum(len(group.sites) for group in self.rows)

    @property
    def files_omitted(self) -> int:
        """How many referencing files the limit cut out whole."""
        return max(0, self.files - len(self.rows))

    def __len__(self) -> int:
        """Return how many file groups the listing kept."""
        return len(self.rows)

    @overload
    def __getitem__(self, index: int) -> RefGroup: ...

    @overload
    def __getitem__(self, index: slice) -> tuple[RefGroup, ...]: ...

    def __getitem__(self, index: int | slice) -> RefGroup | tuple[RefGroup, ...]:
        """Return one kept group, or a slice of them."""
        return self.rows[index]

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this listing."""
        return {
            "total": self.total,
            "limit": self.limit,
            "omitted": self.omitted,
            "files": self.files,
            "files_omitted": self.files_omitted,
            "groups": [group.as_dict() for group in self.rows],
        }


@dataclass(frozen=True)
class CardListing(_Bounded, Sequence[SymbolCard]):
    """The symbol cards a lookup's limit kept, and how many it left out.

    Same shape and same reason as :class:`RefListing`. An expansion passes a
    bare tuple of cards instead, because nothing there is cut by a limit
    silently: every id it could not answer comes back named.
    """

    rows: tuple[SymbolCard, ...] = ()
    total: int = 0
    limit: int = 0

    @property
    def shown(self) -> int:
        """How many cards the listing kept."""
        return len(self.rows)

    def __len__(self) -> int:
        """Return how many cards the listing kept."""
        return len(self.rows)

    @overload
    def __getitem__(self, index: int) -> SymbolCard: ...

    @overload
    def __getitem__(self, index: slice) -> tuple[SymbolCard, ...]: ...

    def __getitem__(self, index: int | slice) -> SymbolCard | tuple[SymbolCard, ...]:
        """Return one kept card, or a slice of them."""
        return self.rows[index]

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this listing."""
        return {
            "total": self.total,
            "limit": self.limit,
            "omitted": self.omitted,
            "matches": [card.as_dict() for card in self.rows],
        }


@dataclass(frozen=True)
class CallerRef:
    """One caller shared between the refs target and a candidate symbol."""

    qualname: str
    path: str
    line: int

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this caller."""
        return {"qualname": self.qualname, "path": self.path, "line": self.line}


@dataclass(frozen=True)
class SharedCaller:
    """A symbol that shares callers with the target of a refs query.

    ``overlap`` counts the shared callers and ``shared_files`` the distinct
    files they live in; ``score`` is the second damped by how common the
    candidate's name is across the repository and by how promiscuous each
    shared caller is. The ranking reads off ``score``, so the row carries all
    three rather than a number a reader would have to take on trust.

    ``in_tests`` says the candidate is defined under a test tree. The row is
    still shown -- hiding it would misreport the repository -- but the ranking
    seats it below every production candidate, because the question the view
    answers is "is there a production utility for this already".
    """

    stable_id: str
    path: str
    line: int
    overlap: int
    shared_files: int
    score: float
    callers: tuple[CallerRef, ...]
    in_tests: bool = False

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this adjacency row."""
        return {
            "stable_id": self.stable_id,
            "path": self.path,
            "line": self.line,
            "overlap": self.overlap,
            "shared_files": self.shared_files,
            "score": round(self.score, 6),
            "defined_in_tests": self.in_tests,
            "shared_callers": [caller.as_dict() for caller in self.callers],
        }


@dataclass(frozen=True)
class SharedCallerListing(_Bounded, Sequence[SharedCaller]):
    """The adjacency rows the limit kept, and how many candidates it left out.

    A ``Sequence`` rather than a bare tuple because the renderer receives
    nothing but this object and the target name, and an omitted count that
    lived anywhere else would be a truncation the text render could not
    announce. Iteration and indexing behave exactly as the tuple did.
    """

    rows: tuple[SharedCaller, ...] = ()
    total: int = 0
    limit: int = 0

    @property
    def shown(self) -> int:
        """How many ranked candidates the listing kept."""
        return len(self.rows)

    def __len__(self) -> int:
        """Return how many rows the listing kept."""
        return len(self.rows)

    @overload
    def __getitem__(self, index: int) -> SharedCaller: ...

    @overload
    def __getitem__(self, index: slice) -> tuple[SharedCaller, ...]: ...

    def __getitem__(self, index: int | slice) -> SharedCaller | tuple[SharedCaller, ...]:
        """Return one kept row, or a slice of them."""
        return self.rows[index]

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this listing."""
        return {
            "total": self.total,
            "limit": self.limit,
            "omitted": self.omitted,
            "rows": [row.as_dict() for row in self.rows],
        }


@dataclass(frozen=True)
class EdgeRow:
    """One resolved edge, seen from the symbol the card is about."""

    node: str
    label: str
    path: str
    line: int
    relation: str

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this row."""
        return {
            "node": self.node,
            "label": self.label,
            "path": self.path,
            "line": self.line,
            "relation": self.relation,
        }


@dataclass(frozen=True)
class TierGroup(_Bounded):
    """The edges of one evidence tier, already capped at the section limit."""

    tier: str
    tier_label: str
    rows: tuple[EdgeRow, ...]
    total: int

    @property
    def shown(self) -> int:
        """How many rows of this tier the limit kept."""
        return len(self.rows)

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this group."""
        return {
            "tier": self.tier,
            "total": self.total,
            "omitted": self.omitted,
            "rows": [row.as_dict() for row in self.rows],
        }


@dataclass(frozen=True)
class ImportRow:
    """One module-level import edge, in whichever direction it was asked for."""

    path: str
    line: int
    module: str
    other: str

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this row."""
        return {"path": self.path, "line": self.line, "module": self.module, "other": self.other}


@dataclass(frozen=True)
class ImportListing(_Bounded, Sequence[ImportRow]):
    """One direction of a file's import edges, already capped at the limit.

    The rows used to travel as a bare tuple, so the count behind them was
    never even computed: twenty importers of a widely-imported module read as
    the complete set in the text *and* in the JSON, which had no field that
    could have carried it.
    """

    rows: tuple[ImportRow, ...] = ()
    total: int = 0
    limit: int = 0

    @property
    def shown(self) -> int:
        """How many import rows the limit kept."""
        return len(self.rows)

    def __len__(self) -> int:
        """Return how many rows the listing kept."""
        return len(self.rows)

    @overload
    def __getitem__(self, index: int) -> ImportRow: ...

    @overload
    def __getitem__(self, index: slice) -> tuple[ImportRow, ...]: ...

    def __getitem__(self, index: int | slice) -> ImportRow | tuple[ImportRow, ...]:
        """Return one kept row, or a slice of them."""
        return self.rows[index]

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this listing."""
        return {
            "total": self.total,
            "limit": self.limit,
            "omitted": self.omitted,
            "rows": [row.as_dict() for row in self.rows],
        }


@dataclass(frozen=True)
class PathHop:
    """One step of a resolved path, with the direction the edge really runs."""

    verb: str
    tier: str
    tier_label: str
    arrow: str
    node: str
    label: str
    path: str
    line: int

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this hop."""
        return {
            "relation": self.verb,
            "tier": self.tier,
            "direction": self.arrow,
            "node": self.node,
            "label": self.label,
            "path": self.path,
            "line": self.line,
        }


@dataclass(frozen=True)
class CycleRow:
    """One import cycle as the chain of files that closes it."""

    files: tuple[str, ...]

    @property
    def chain(self) -> str:
        """Render the cycle as ``a -> b -> a``."""
        return " -> ".join([*self.files, self.files[0]])

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this cycle."""
        return {"length": len(self.files), "files": list(self.files), "chain": self.chain}


@dataclass(frozen=True)
class Explanation:
    """One symbol's card: where it is defined, what it touches, what touches it."""

    target: str
    card: SymbolCard | None
    message: str
    alternatives: tuple[str, ...]
    rationales: tuple[RationaleNode, ...]
    fan_out: tuple[TierGroup, ...]
    fan_in: tuple[TierGroup, ...]
    imports_out: ImportListing
    imports_in: ImportListing

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this explanation."""
        record: dict[str, Any] = {
            "target": self.target,
            "symbol": self.card.as_dict() if self.card is not None else None,
            "message": self.message,
            "alternatives": list(self.alternatives),
            "fan_out": [group.as_dict() for group in self.fan_out],
            "fan_in": [group.as_dict() for group in self.fan_in],
            "imports": {
                "declared": self.imports_out.as_dict(),
                "importers": self.imports_in.as_dict(),
            },
        }
        if self.rationales:
            record["rationales"] = [rationale.as_dict() for rationale in self.rationales]
        return record


@dataclass(frozen=True)
class PathTrace:
    """The hops between two symbols, or the reason there are none."""

    source: str
    target: str
    source_label: str
    target_label: str
    hops: tuple[PathHop, ...]
    found: bool
    message: str
    visited: int
    exhausted: bool
    include_unique: bool
    include_ambiguous: bool
    endpoints_resolved: bool

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this trace."""
        return {
            "source": self.source,
            "target": self.target,
            "endpoints_resolved": self.endpoints_resolved,
            "found": self.found,
            "message": self.message,
            "hops": [hop.as_dict() for hop in self.hops],
            "visited": self.visited,
            "exhausted": self.exhausted,
            "include_unique": self.include_unique,
            "include_ambiguous": self.include_ambiguous,
        }


@dataclass(frozen=True)
class CycleReport(_Bounded):
    """Every import cycle found, already capped at the caller's limit."""

    cycles: tuple[CycleRow, ...]
    total: int
    limit: int

    @property
    def shown(self) -> int:
        """How many cycles the limit kept."""
        return len(self.cycles)

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this report."""
        return {
            "total": self.total,
            "limit": self.limit,
            "omitted": self.omitted,
            "cycles": [cycle.as_dict() for cycle in self.cycles],
        }


@dataclass(frozen=True)
class CommunityRow:
    """One community of files: its mechanical label and the members shown."""

    label: str
    size: int
    members: tuple[str, ...]
    omitted: int
    internal_weight: float
    total_weight: float

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this community."""
        return {
            "label": self.label,
            "size": self.size,
            "members": list(self.members),
            "omitted": self.omitted,
            "internal_weight": round(self.internal_weight, 6),
            "total_weight": round(self.total_weight, 6),
        }


@dataclass(frozen=True)
class CommunityReport(_Bounded):
    """A whole partition, already capped, with the score behind it."""

    communities: tuple[CommunityRow, ...]
    total: int
    limit: int
    modularity: float
    resolution: float
    files: int

    @property
    def shown(self) -> int:
        """How many communities the limit kept."""
        return len(self.communities)

    @property
    def weak_partition(self) -> bool:
        """Whether the modularity is too weak for an architectural claim."""
        return self.modularity < WEAK_MODULARITY_THRESHOLD

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this report."""
        return {
            "total": self.total,
            "limit": self.limit,
            "omitted": self.omitted,
            "modularity": _no_negative_zero(round(self.modularity, 6)),
            "weak_partition": self.weak_partition,
            "resolution": self.resolution,
            "files": self.files,
            "communities": [community.as_dict() for community in self.communities],
        }


@dataclass(frozen=True)
class DiagramView:
    """Rendered mermaid text, plus what the render left out.

    ``text`` is bare mermaid with no fence: the CLI writes it into a document
    the caller fences, and the MCP tool fences it into a response body.
    ``message`` is non-empty only when there is no diagram to show.
    """

    text: str
    nodes: int
    elided: int
    grouped: bool
    focus: str
    message: str

    @property
    def caveat(self) -> str:
        """The qualification a grouped, bounded diagram has to carry.

        A subgraph is titled after its whole community, and the rank bound
        drops members out of the picture without changing that title. Left
        unsaid, a reader counts the boxes inside a group and believes the
        count.
        """
        if not self.grouped or self.elided <= 0:
            return ""
        return (
            "note: subgraph titles name whole communities, including the "
            f"{self.elided} module(s) the rank bound left out of this diagram"
        )

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this diagram."""
        return {
            "mermaid": self.text,
            "nodes": self.nodes,
            "elided": self.elided,
            "grouped": self.grouped,
            "focus": self.focus,
            "message": self.message,
            "caveat": self.caveat,
        }


@dataclass(frozen=True)
class LintFinding:
    """One patch-lint finding, denormalized the way every other row is."""

    check: str
    severity: str
    message: str
    path: str
    line: int
    location: str
    evidence: str

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this finding."""
        return {
            "check": self.check,
            "severity": self.severity,
            "message": self.message,
            "path": self.path,
            "line": self.line,
            "location": self.location,
            "evidence": self.evidence,
        }


@dataclass(frozen=True)
class LintCandidate:
    """Every finding one candidate patch produced."""

    id: str
    findings: tuple[LintFinding, ...]

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this candidate's report."""
        return {"id": self.id, "findings": [finding.as_dict() for finding in self.findings]}


@dataclass(frozen=True)
class LintReportView:
    """The lint report over one or more candidate patches.

    Deliberately without a verdict field, matching
    :class:`agentless_mcp.core.patchlint.LintReport`: this view says what to
    look at, never whether to proceed.
    """

    candidates: tuple[LintCandidate, ...]

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this report."""
        return {"candidates": [candidate.as_dict() for candidate in self.candidates]}


def render_communities(report: CommunityReport) -> str:
    """Render a community partition, largest group first.

    Keyed on ``total`` rather than on the list the limit left, for the reason
    :func:`render_cycles` records: a bound of zero must describe the bound.
    """
    if not report.total:
        return "no communities: nothing in this repository parsed into files\n"

    groups = "community" if report.total == 1 else "communities"
    lines = [
        (
            f"{report.total} {groups} over {report.files} files "
            f"(modularity {_no_negative_zero(round(report.modularity, 3)):.3f} "
            f"at resolution {report.resolution:g})"
        )
    ]
    if report.weak_partition:
        lines.append("  note: weak partition; use these communities as a hint, not a boundary")
    for index, community in enumerate(report.communities, start=1):
        files = "file" if community.size == 1 else "files"
        lines.append(f"  {index:>3}. {one_line(community.label)}  ({community.size} {files})")
        lines.extend(f"       {one_line(member)}" for member in community.members)
        if community.omitted:
            lines.append(f"       ... {community.omitted} more files in this community")
    if report.omitted:
        lines.append(f"  ... {report.omitted} more communities not listed")
    return "\n".join(lines) + "\n"


def render_diagram(view: DiagramView) -> str:
    """Render a diagram for a response body: fenced, with its caveat below.

    The fence is added here rather than in
    :mod:`agentless_mcp.core.mermaid` because it is a property of where the
    text is going. The CLI writes ``view.text`` straight out so a caller can
    paste it into a document and choose their own fence.
    """
    if not view.text:
        return (view.message or "no diagram").rstrip("\n") + "\n"

    body = f"{MERMAID_FENCE}\n{view.text.rstrip(chr(10))}\n{FENCE}\n"
    return body if not view.caveat else f"{body}\n{view.caveat}\n"


def strip_fence(text: str) -> str:
    """Return the diagram inside a markdown fence, or ``text`` unchanged.

    The inverse of what :func:`render_diagram` adds, so that a diagram
    committed into a document -- fenced, as a document needs it -- can be
    compared byte for byte against a fresh render. Only a fence that opens the
    document and closes it is removed: anything else is a file with a diagram
    somewhere inside it, which is not what a drift check is comparing.
    """
    lines = text.split("\n")
    while lines and not lines[0].strip():
        lines.pop(0)
    if not lines or not lines[0].startswith(FENCE):
        return text

    closing = len(lines) - 1
    while closing > 0 and not lines[closing].strip():
        closing -= 1
    if closing == 0 or lines[closing].strip() != FENCE:
        return text
    return "\n".join(lines[1:closing]) + "\n"


def render_lint(report: LintReportView) -> str:
    """Render lint findings, grouped by candidate and ordered as they arrived."""
    blocks: list[str] = []
    for candidate in report.candidates:
        counts = _severity_counts(candidate.findings)
        lines = [f"{one_line(candidate.id)}: {counts}"]
        lines.extend(
            f"  [{one_line(finding.severity)}] {one_line(finding.check)}  "
            f"{one_line(finding.location)}\n"
            f"      {one_line(finding.message)}\n"
            f"      evidence: {one_line(finding.evidence)}"
            for finding in candidate.findings
        )
        blocks.append("\n".join(lines))
    return "\n\n".join(blocks) + "\n" if blocks else "no candidates to lint\n"


def _severity_counts(findings: Sequence[LintFinding]) -> str:
    """Summarise one candidate's findings by severity, in a fixed order."""
    tally: dict[str, int] = {}
    for finding in findings:
        tally[finding.severity] = tally.get(finding.severity, 0) + 1
    if not tally:
        return "no findings"
    return ", ".join(f"{tally[severity]} {severity}" for severity in sorted(tally))


def render_explanation(explanation: Explanation) -> str:
    """Render one symbol card with its tiered fan-out, fan-in and imports."""
    if explanation.card is None:
        return explanation.message.rstrip("\n") + "\n"

    lines = [_render_card(explanation.card)]
    lines.extend(f"  also defined at {one_line(entry)}" for entry in explanation.alternatives)
    if explanation.rationales:
        lines.append("")
        lines.append("rationale")
        lines.extend(
            f"  {one_line(node.kind).upper()}  {one_line(node.text)}    "
            f"[{one_line(node.stable_id)} -> {one_line(node.parent_id)}] @{node.line}"
            for node in explanation.rationales
        )
    lines.append("")
    lines.append(_render_tiers("references (fan-out)", explanation.fan_out))
    lines.append(_render_tiers("referenced by (fan-in)", explanation.fan_in))
    lines.append(_render_imports(explanation))
    return "\n".join(lines).rstrip("\n") + "\n"


def render_path(trace: PathTrace) -> str:
    """Render a path hop by hop, each with its relation, tier and file:line."""
    if not trace.found:
        return trace.message.rstrip("\n") + "\n"

    hops = "hop" if len(trace.hops) == 1 else "hops"
    lines = [
        f"{len(trace.hops)} {hops} from {one_line(trace.source)} to {one_line(trace.target)}",
        f"  start  {one_line(trace.source_label)}    {one_line(trace.source)}",
    ]
    lines.extend(
        f"  {number:>3}. {hop.arrow} {hop.verb} ({one_line(hop.tier_label)})    "
        f"{one_line(hop.label)}    [{one_line(hop.node)}] @{hop.line}"
        for number, hop in enumerate(trace.hops, start=1)
    )
    if trace.message:
        lines.append(f"  {trace.message}")
    return "\n".join(lines) + "\n"


def render_cycles(report: CycleReport) -> str:
    """Render module-level import cycles as arrow chains.

    Keyed on ``total``, which the service computes before truncation, rather
    than on the list the limit left behind. Branching on the truncated list
    made ``cycles --limit 0`` answer "no import cycles", exit 0, for a
    repository that has one -- a bound reported as a fact about the code. Of
    the three renderers that phrase an empty result as a statement about the
    repository, this is the one whose statement clears something.
    """
    if not report.total:
        return "no import cycles\n"

    cycles = "cycle" if report.total == 1 else "cycles"
    lines = [f"{report.total} import {cycles}"]
    for index, cycle in enumerate(report.cycles, start=1):
        lines.append(f"  {index:>3}. ({len(cycle.files)} files) {one_line(cycle.chain)}")
    if report.omitted:
        lines.append(f"  ... {report.omitted} more cycles not listed")
    return "\n".join(lines) + "\n"


def _render_tiers(heading: str, groups: Sequence[TierGroup]) -> str:
    """Render one section of an explanation, grouped strongest tier first."""
    total = sum(group.total for group in groups)
    if not total:
        return f"{heading}: none\n"

    lines = [f"{heading}: {total}"]
    for group in groups:
        lines.append(f"  {one_line(group.tier_label)} ({group.total})")
        lines.extend(
            f"    {one_line(row.relation)} {one_line(row.label)}    "
            f"[{one_line(row.node)}] @{row.line}"
            for row in group.rows
        )
        if group.omitted:
            lines.append(f"    ... {group.omitted} more at this tier")
    return "\n".join(lines) + "\n"


def _render_imports(explanation: Explanation) -> str:
    """Render the import relationships of the file the symbol lives in."""
    lines = ["imports"]
    if explanation.imports_out:
        lines.extend(
            f"    declares  {one_line(row.module)} -> {one_line(row.other)}    "
            f"{one_line(row.path)}:{row.line}"
            for row in explanation.imports_out
        )
        if explanation.imports_out.omitted:
            lines.append(f"    ... {explanation.imports_out.omitted} more declared, not listed")
    else:
        lines.append("    declares  none resolved inside this repository")
    if explanation.imports_in:
        lines.extend(
            f"    imported by  {one_line(row.path)}:{row.line}  as {one_line(row.module)}"
            for row in explanation.imports_in
        )
        if explanation.imports_in.omitted:
            lines.append(f"    ... {explanation.imports_in.omitted} more importers, not listed")
    else:
        lines.append("    imported by  nothing in this repository")
    return "\n".join(lines) + "\n"


def render_skipped_files(skipped: Sequence[SkippedFile]) -> str:
    """Render a scan's skipped files as one warning line, or nothing.

    A skipped file is invisible to the ranking and the matching, so an answer
    built over it reads as affirmative absence -- "no matching symbols" about
    a repository whose relevant file was never parsed. The warning rides at
    the top of the body, like the map's unresolved-seeds note, because it
    changes how everything below it must be read. Each path carries its skip
    reason, which already names the remedy (raise the cap, run warmup); the
    JSON forms carry every entry, this line caps the listing.
    """
    if not skipped:
        return ""
    shown = skipped[:SKIPPED_FILES_SHOWN]
    listed = "; ".join(f"{one_line(entry.path)} ({one_line(entry.reason)})" for entry in shown)
    if len(skipped) > len(shown):
        listed += f"; ... {len(skipped) - len(shown)} more"
    return MESSAGES.scan_skipped_files.format(count=len(skipped), listed=listed)


def render_map(files: Sequence[MapFile]) -> str:
    """Render ranked files as code-shaped signature blocks.

    An empty result says only that, because this function is handed the rows
    and nothing else. It used to answer "nothing in this repository parsed
    into symbols", which is a claim about the repository that the rows cannot
    support: ``MapService._pack`` also returns zero when the first candidate's
    render exceeds the budget, so a parsed repository read as one that
    parses into nothing. The service knows which happened and says so -- see
    ``MapService.render_text``.
    """
    if not files:
        return "no ranked files\n"

    blocks: list[str] = []
    for map_file in files:
        lines = [f"{one_line(map_file.path)}  (rank {map_file.rank:.4f})"]
        for entry in map_file.entries:
            lines.append(
                f"{line_prefix(entry.line, LINE_NUMBER_WIDTH)}"
                f"{'    ' * entry.depth}{one_line(entry.signature)}  "
                f"[{one_line(entry.stable_id)}]"
            )
            lines.extend(
                f"{line_prefix(node.line, LINE_NUMBER_WIDTH)}"
                f"{'    ' * (entry.depth + 1)}# {one_line(node.kind).upper()}: "
                f"{one_line(node.text)}  "
                f"[{one_line(node.stable_id)} -> {one_line(node.parent_id)}]"
                for node in entry.rationales
            )
        if map_file.omitted:
            lines.append(f"      ... {map_file.omitted} more symbols in this file")
        blocks.append("\n".join(lines))
    return "\n\n".join(blocks) + "\n"


def render_symbol_cards(cards: Sequence[SymbolCard]) -> str:
    """Render incident cards for symbol lookups and expansions.

    A :class:`CardListing` says how many matches its limit left out and gets
    that line appended; a bare sequence is a caller whose cards were never cut
    by a limit -- the expansion path, which names every id it could not answer
    instead. The count is read off the listing rather than taken as a second
    argument, so it cannot get separated from the rows it describes.
    """
    if not cards:
        return "no matching symbols\n"

    body = "\n\n".join(_render_card(card) for card in cards) + "\n"
    if isinstance(cards, CardListing) and cards.omitted:
        return f"{body}  ... {cards.omitted} more matches not listed (limit {cards.limit})\n"
    return body


def render_ref_groups(groups: Sequence[RefGroup], target: str) -> str:
    """Render fan-in: references grouped by the file they were found in.

    The header is the *pre-limit* total when a :class:`RefListing` supplies
    one. Recomputing it from the groups that survived is what made a fan-in of
    fifty-two sites answer "10 references to widget" and say nothing else --
    an agent reads that as the blast radius and ships against it.
    """
    if not groups:
        return f"no references to {one_line(target)} outside its own definition\n"

    listed = sum(len(group.sites) for group in groups)
    total = groups.total if isinstance(groups, RefListing) else listed
    blocks = [f"{total} references to {one_line(target)}"]
    for group in groups:
        labelled = f", {one_line(group.tier_label)}" if group.tier_label else ""
        lines = [f"{one_line(group.path)}  ({len(group.sites)} references{labelled})"]
        lines.extend(_render_site(site) for site in group.sites)
        blocks.append("\n".join(lines))
    if isinstance(groups, RefListing) and groups.omitted:
        note = f"  ... {groups.omitted} more references not listed (limit {groups.limit})"
        if groups.files_omitted:
            note += f", including every reference in {groups.files_omitted} more files"
        blocks.append(note)
    return "\n\n".join(blocks) + "\n"


def render_shared_callers(listing: SharedCallerListing, target: str) -> str:
    """Render the adjacency view: symbols called by the same callers.

    Ranked strongest first with production-defined candidates ahead of every
    test-defined one, and every line -- the candidate and each caller --
    carries its own ``file:line``, so the DRY question is answered with
    somewhere to go rather than with a list of names. Each candidate lists at
    most :data:`SHARED_CALLERS_SHOWN` callers before cutting to a count, and
    candidates past the listing's limit are counted rather than shown.
    """
    if not listing.rows:
        return f"no symbols share callers with {one_line(target)}\n"

    lines = [f"symbols sharing callers with {one_line(target)}"]
    tests_heading_shown = False
    for row in listing.rows:
        if row.in_tests and not tests_heading_shown:
            lines.append("  defined in tests (ranked below all production candidates):")
            tests_heading_shown = True
        files = "file" if row.shared_files == 1 else "files"
        lines.append(
            f"  [{one_line(row.stable_id)}] @{row.line}  "
            f"({row.overlap} shared callers in {row.shared_files} {files}, "
            f"score {row.score:.3f})"
        )
        shown = row.callers[:SHARED_CALLERS_SHOWN]
        lines.extend(
            f"      {one_line(caller.qualname)}    {one_line(caller.path)}:{caller.line}"
            for caller in shown
        )
        if len(row.callers) > len(shown):
            lines.append(f"      ... {len(row.callers) - len(shown)} more callers not listed")
    if listing.omitted:
        lines.append(f"  ... {listing.omitted} more candidates not listed")
    return "\n".join(lines) + "\n"


def _render_card(card: SymbolCard) -> str:
    """Render one incident card without repeating the path inside its stable id."""
    owner = f" in class {one_line(card.parent_class)}" if card.parent_class else ""
    span = (
        str(card.start_line)
        if card.start_line == card.end_line
        else f"{card.start_line}-{card.end_line}"
    )
    lines = [
        f"[{one_line(card.stable_id)}] @{span}",
        f"  {one_line(card.kind)}{owner} ({one_line(card.language)})",
    ]
    if card.body:
        lines.extend(f"  {one_line(line)}" for line in card.body.split("\n"))
    else:
        lines.append(f"  {one_line(card.signature)}")
    return "\n".join(lines)


def _render_site(site: RefSite) -> str:
    """Render one reference row beneath the file header that locates it."""
    suffix = f"  [{one_line(site.stable_id)}]" if site.stable_id else ""
    prefix = line_prefix(site.line, LINE_NUMBER_WIDTH)
    return f"{prefix}{one_line(site.enclosing)}{suffix}"
