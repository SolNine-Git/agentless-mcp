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

A position is spelled one way: ``@line`` or ``@start-end``, appended to the
row. The ``N| `` gutter means something else and only that -- this line is
verbatim repository content -- so it belongs to the views that quote a file
(``read(slice)``, ``symbols(expand)``, ``symbols(locate)``, the overview
body) and not to the views that cite a position. The map and fan-in rows used
the gutter for a citation, which read as a promise of verbatim text that a
normalized signature does not keep: a symbol whose definition spans eight
lines was rendered on one, behind a number that said it was that line.

In a view grouped under a file header, the header spells the path and the
rows below carry the qualified name alone. The id pattern is printed once,
under the header, by :func:`_stable_ids_line`.

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

**Structural markers sit in the first column, and every row is indented.**
An omission line is the one signal that separates a bounded answer from a
complete one, and it used to be distinguished from verbatim repository text
only by an indent that the text also got: a symbol body line and the notice
below it both opened with two spaces, so a file containing
``... 7 more matches not listed (limit 3)`` rendered a byte-identical cut.
:func:`_omitted_line` is the one home for that line, and it emits it
unindented for exactly that reason.

Two other modules render line-oriented answers and carry the same rule:
:func:`agentless_mcp.core.treewalk.render_tree` and
:mod:`agentless_mcp.application.envelope`. ``core/mermaid`` has its own
stricter escape because its grammar is not this one.
"""

from abc import ABC, abstractmethod
from collections.abc import Iterator, Sequence
from dataclasses import dataclass
from typing import Any, overload

from agentless_mcp.core.gitinfo import CHURN_WINDOW_DAYS
from agentless_mcp.core.patchlint import Severity
from agentless_mcp.core.refs import SkippedFile
from agentless_mcp.core.symbols import StableId, language_prefix
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

# What the id pattern stands the symbol's own name in for. Spelled once so the
# pattern line and the guide agree.
QUALIFIED_NAME_PLACEHOLDER = "<QualifiedName>"

# What an overview header names when the file has no language: a path that
# reached no grammar, or one the walk refused. The header needs *some*
# tool-authored fact -- see :func:`_omitted_line` for why every grouped header
# carries one -- and "the view could not say" is the honest one here.
UNKNOWN_LANGUAGE = "unknown"

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
# property of the destination, not of the diagram. `core.patches` spells the
# same three characters for the opposite job -- recognising a fence a model
# wrote around a patch -- and that dialect is not ours to change, so the emit
# side and the parse side keep separate copies on purpose.
FENCE = "```"
MERMAID_FENCE = FENCE + "mermaid"

# Below this, a partition is a hint rather than a boundary. The published
# reading of modularity is calibrated for resolution 1.0 alone, so this
# constant may only ever be compared against
# `CommunityReport.standard_modularity` -- see `weak_partition`, which is the
# one place that comparison lives.
WEAK_MODULARITY_THRESHOLD = 0.3

# The severities of `core.patchlint`, most urgent first. The spellings come
# from the enum so a rename there breaks the import rather than silently
# reordering a summary line; the order is a rendering decision, so it lives
# here. Sorting on the spellings instead put `warning` last, behind
# `not_checked`, which is the reading order of an alphabet and not of a
# severity.
SEVERITY_ORDER = (Severity.WARNING.value, Severity.ADVISORY.value, Severity.NOT_CHECKED.value)


def _omitted_line(omitted: int, noun: str, *, limit: int = 0) -> str:
    """Render the one line a bounded view announces its cut with.

    One spelling for every listing in this module, and no indent. Every row
    rendered here is indented -- a card body by two spaces, a community member
    by seven, a map symbol or reference row by :data:`ROW_INDENT` -- so a
    marker in the first column is the one line repository text placed on a row
    cannot forge. ``ROW_INDENT`` is load-bearing for that reason: the map and
    fan-in rows used to be indented by the width of their line-number gutter,
    and dropping the gutter without replacing the indent would have put
    repository-derived text in the first column.

    A grouped view's *file header* is the exception to the indent, so it
    carries the other half of the rule instead: every one of them appends a
    tool-authored ``  (fact)`` -- the rank on a map, the reference count and
    tier on a fan-in, the language on an overview. A header is therefore
    never a bare repository value, and no filename can spell a whole line
    this function could have written. :func:`overview_block` shipped for one
    release without that suffix, and a file named
    ``... 7 more matches not listed (limit 3)`` rendered a header byte-identical
    to this marker.

    Eleven hand-spelled variants used to differ in whether they named the
    limit and in where they put the comma, so an agent asking how much was
    left out had eleven patterns to match.
    """
    bound = f" (limit {limit})" if limit else ""
    return f"... {omitted} more {noun} not listed{bound}"


def _locator(stable_id: str, *, parent: str = "", line: int | str | None = None) -> str:
    """Render the navigable fragment a row is read for.

    A stable id already carries the repository-relative path, so a location
    appends only the line or span. ``parent`` links a rationale comment to the
    symbol it annotates. One home, because this fragment is what the whole
    product exists to emit and it was re-derived at six sites.
    """
    named = one_line(stable_id) if not parent else f"{one_line(stable_id)} -> {one_line(parent)}"
    return f"[{named}]" if line is None else f"[{named}] @{line}"


ROW_INDENT = "  "


def _stable_ids_line(id_prefix: str, path: str) -> str:
    """Render the id pattern a grouped view prints once under its file header.

    The rows below it then carry the qualified name alone. A stable id spells
    the language and the repository-relative path, and the header directly
    above the rows already spells the path, so repeating both on every row
    bought nothing: measured 2026-08-25 on this repository with
    ``agentless-mcp map --no-cache --max-files 10``, that prefix was 3855 of
    13628 characters, 28% of the answer. ``symbols(overview)`` has printed this
    line and no per-row id since it shipped; this is the same line.

    Empty when the caller knows no prefix, which keeps the whole id on the row
    rather than printing a pattern that cannot rebuild one. Built through
    :class:`~agentless_mcp.core.symbols.StableId` so the pattern and a real id
    come out of the same grammar.
    """
    if not id_prefix:
        return ""
    pattern = str(StableId(id_prefix, path, QUALIFIED_NAME_PLACEHOLDER))
    return f"{ROW_INDENT}{MESSAGES.stable_ids_pattern.format(pattern=one_line(pattern))}"


def overview_block(path: str, language: str, error: str, text: str) -> str:
    """Render one file's overview block: the header, the id pattern, the body.

    One home because there were two, and they had drifted. Both adapters
    headed the block with a markdown ``###`` no other grouped view in this
    package uses, and only the MCP one printed the ``stable ids:`` pattern
    line -- so an agent that reached this view through the CLI had no id to
    escalate with, and the same view answered differently depending on which
    door you came through.

    The escaping travels with it. A header sits in the first column and a
    newline is legal in a POSIX filename, so ``path`` has to reach
    :func:`one_line` exactly like every other repository-derived value on a
    row -- the rule this module states and the adapters were outside of.
    ``text`` does not: it is a rendered file body, which owns its own line
    grammar and carries a line-number gutter to say so.

    ``one_line`` is not enough on its own. Dropping the ``###`` left the
    header a bare repository value alone on a line, and this is the one
    grouped header with nothing after the path, so a file could be named for
    a whole tool-authored line: ``... 7 more matches not listed (limit 3)``
    rendered a header this module's own omission marker could not be told
    from. The language suffix closes that, and puts this header in the
    grammar the map and fan-in headers already use -- the path, then a
    parenthesized fact the view knows. See :func:`_omitted_line`.

    An errored file keeps its block, with the reason indented under the
    header where a reader cannot mistake it for the file's contents. Its
    language is often empty -- a path that reached no grammar has none -- so
    the suffix reads :data:`UNKNOWN_LANGUAGE` rather than going missing on
    exactly the paths an untrusted repository controls.
    """
    header = f"{one_line(path)}  ({one_line(language) or UNKNOWN_LANGUAGE})"
    if error:
        return f"{header}\n{ROW_INDENT}{one_line(error)}"
    pattern = str(StableId(language_prefix(language), path, QUALIFIED_NAME_PLACEHOLDER))
    ids_line = MESSAGES.overview_stable_ids.format(pattern=one_line(pattern))
    return f"{header}\n{ROW_INDENT}{ids_line}\n{text}"


def _file_site(path: str, line: int) -> str:
    """Render the ``path:line`` fragment a row uses when it names no symbol."""
    return f"{one_line(path)}:{line}"


def _references(count: int) -> str:
    """Spell the noun a reference count takes."""
    return "reference" if count == 1 else "references"


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
    # The qualified name the stable id addresses this symbol by, which is the
    # id with its language and path prefix removed. Carried rather than
    # sliced off ``stable_id`` here: this module renders and does not parse,
    # and the prefix is exactly what the file header above the row already
    # spells. Empty falls the row back to the whole id.
    name: str = ""

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
class MapFile(_Bounded):
    """One ranked file in a repository map, with the symbols that fit.

    ``total`` is how many of this file's symbols competed for a place, so the
    omitted count is the subtraction :class:`_Bounded` owns rather than a
    number the service works out again. Passed in as a difference, the two
    granularities computed it against two different denominators.
    """

    path: str
    rank: float
    entries: tuple[MapEntry, ...] = ()
    total: int = 0
    # The id prefix this file's stable ids carry ("py", "ts", ...). With the
    # path it spells the id pattern printed once under the header, which is
    # what lets every row below drop the prefix. Empty keeps the whole id on
    # each row.
    id_prefix: str = ""
    # Whether the ranking walk reached this file from the focus. False is what
    # makes the omission line say so instead of offering a bigger budget: no
    # budget renders a symbol the focus has no path to, so the ordinary
    # wording would be advice that cannot work. Defaults to True because an
    # unfocused walk reaches every file, and because a caller building one of
    # these by hand is describing a file it chose to include.
    reached: bool = True
    # Commit activity inside the churn window, for the model to weigh itself
    # -- deliberately data beside the rank, never folded into it. None means
    # git could not answer and the header stays bare; a known-quiet file
    # carries 0 with no "last" part, so absence and quiet cannot be confused.
    commits_window: int | None = None
    last_commit_days: int | None = None

    @property
    def shown(self) -> int:
        """How many of this file's symbols the map lists."""
        return len(self.entries)

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this file.

        ``reached`` is here because ``omitted`` alone cannot answer the one
        question a caller asks it: would raising the budget show the rest? The
        text render answers that by branching on ``reached`` for its omission
        line, and for one release the JSON form did not carry the fact at all,
        so the two doors onto the same map disagreed. ``id_prefix`` is not
        here and does not belong here: it compresses a *rendered* row, and
        every symbol entry below still carries its whole stable id.
        """
        return {
            "path": self.path,
            "rank": round(self.rank, 6),
            "symbols": [entry.as_dict() for entry in self.entries],
            "omitted": self.omitted,
            "reached": self.reached,
            "commits_90d": self.commits_window,
            "last_commit_days": self.last_commit_days,
        }


@dataclass(frozen=True)
class TestCompanion:
    """One test file that exercises the files a map ranked, and where it does it.

    Edges run referrer to definer, so a test file is a pure source in the
    reference graph: it has no inbound weight and the ranking that scores
    inbound weight can never place it. This row is how a test reaches the map
    at all, and it is listed outside the ranked-file budget because it answers
    a different question from "what is this repository about".

    ``start`` and ``end`` are a real span inside the test file -- the
    referencing symbol that connects it to the files above -- never the whole
    file. A reader handed a bare path has to open the file to find the lines,
    and a whole-file span is the same answer with the work hidden.

    ``covers`` names the ranked or seeded files this one test file reaches,
    which is what makes one row per test file honest: a broad suite that
    touches six of them says so on its own row instead of taking six.
    """

    path: str
    start: int
    end: int
    covers: tuple[str, ...] = ()
    depth: int = 1
    weight: float = 0.0

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this companion."""
        return {
            "path": self.path,
            "start": self.start,
            "end": self.end,
            "covers": list(self.covers),
            "depth": self.depth,
            "weight": round(self.weight, 6),
        }


@dataclass(frozen=True)
class TestCompanionListing(_Bounded):
    """The test companions the cap kept, and how many it left out.

    The listing rather than a bare tuple, for the reason
    :class:`SharedCallerListing` gives: the renderer is handed nothing else,
    and a section that cannot say what it left out is read as the complete
    set of tests for the files above it.

    Two different cuts can shorten this section and they are not the same
    fact. ``limit`` and ``omitted`` say the rows were trimmed *after* every
    test was found, so what is missing is known and counted. ``exhausted``
    says the backward walk stopped before it finished looking, so what is
    missing was never counted and ``total`` is a floor rather than a total.
    A reader told only the first reads a truncated walk as a complete one,
    which is the failure :class:`~agentless_mcp.core.graph.Flood` carries the
    flag to prevent.
    """

    rows: tuple[TestCompanion, ...] = ()
    total: int = 0
    limit: int = 0
    exhausted: bool = False

    @property
    def shown(self) -> int:
        """How many test files the listing kept."""
        return len(self.rows)

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this listing."""
        return {
            "total": self.total,
            "limit": self.limit,
            "omitted": self.omitted,
            "exhausted": self.exhausted,
            "rows": [row.as_dict() for row in self.rows],
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
    # The qualified name the stable id addresses this site's symbol by. It is
    # ``enclosing`` for everything unique in its file and ``enclosing`` plus a
    # ``#2`` ordinal for a collision, which is the only case where the two
    # differ -- and the case where the id's spelling is the precise one. The
    # row prints this alone; printing both spelled the same string twice.
    name: str = ""

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
    sites: tuple[RefSite, ...] = ()
    tier: str = ""
    tier_label: str = ""
    # As on :class:`MapFile`: with the path, the id pattern this group prints
    # once so its rows carry the qualified name alone.
    id_prefix: str = ""

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
    """One import cycle as the chain of files that closes it.

    ``files`` is non-empty by construction: a cycle the detector reports has
    at least one file in it. ``chain`` closes the ring by repeating the first
    entry, so an empty tuple would raise rather than render.
    """

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
    """Every import cycle found, already capped at the caller's limit.

    ``unresolved_imports`` is how many import statements named no file in this
    repository, counted by :mod:`agentless_mcp.core.resolve`, which owns the
    question. It travels with the cycle list because "no import cycles" and
    "few imports were resolved" render identically without it, and the first
    is a claim about the code that only the second can qualify.
    """

    cycles: tuple[CycleRow, ...]
    total: int
    limit: int
    unresolved_imports: int = 0

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
            "unresolved_imports": self.unresolved_imports,
            "cycles": [cycle.as_dict() for cycle in self.cycles],
        }


@dataclass(frozen=True)
class CommunityRow(_Bounded):
    """One community of files: its mechanical label and the members shown.

    A community's whole membership is the denominator its omitted count is
    taken against, so it is carried as :class:`_Bounded`'s ``total`` and the
    subtraction happens once, there. ``size`` is the same number under the
    name the JSON form has always spelled it. ``limit`` is the member bound
    that did the cutting, named on the omission line so a reader knows which
    knob to raise.
    """

    label: str
    total: int
    members: tuple[str, ...]
    internal_weight: float
    total_weight: float
    limit: int = 0

    @property
    def size(self) -> int:
        """How many files this community holds, listed or not."""
        return self.total

    @property
    def shown(self) -> int:
        """How many of them this row lists."""
        return len(self.members)

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
    """A whole partition, already capped, with the scores behind it.

    ``modularity`` is the score at the resolution the partition was found at,
    which is what the header prints. ``standard_modularity`` is the same
    partition scored at resolution 1.0, and it is the only one of the two that
    :attr:`weak_partition` may be keyed on.
    """

    communities: tuple[CommunityRow, ...]
    total: int
    limit: int
    modularity: float
    standard_modularity: float
    resolution: float
    files: int

    @property
    def shown(self) -> int:
        """How many communities the limit kept."""
        return len(self.communities)

    @property
    def weak_partition(self) -> bool:
        """Whether the partition is too weak for an architectural claim.

        Keyed on the resolution-1.0 score rather than on the one the header
        prints, because :data:`WEAK_MODULARITY_THRESHOLD` is a constant and
        the printed score is scaled by a knob the caller sets. Compared
        against the scaled score, ``--resolution 0.25`` suppressed this note
        on an unchanged tree: measured 2026-08-23 on this package, the scaled
        score reads 0.723 at 0.25 and 0.148 at 4.0 while the standard score of
        the very same partitions stays inside 0.156 to 0.319. The note is a
        statement about the repository, so a caller's rollup setting must not
        be able to turn it off.
        """
        return self.standard_modularity < WEAK_MODULARITY_THRESHOLD

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this report."""
        return {
            "total": self.total,
            "limit": self.limit,
            "omitted": self.omitted,
            "modularity": _no_negative_zero(round(self.modularity, 6)),
            "standard_modularity": _no_negative_zero(round(self.standard_modularity, 6)),
            "weak_partition": self.weak_partition,
            "resolution": self.resolution,
            "files": self.files,
            "communities": [community.as_dict() for community in self.communities],
        }


@dataclass(frozen=True)
class DiscountedTier:
    """How many edges of one weak evidence tier a degree count left out.

    The row carries this rather than the renderer inferring it, because
    "nothing references this symbol" and "one name-only-ambiguous match
    references this symbol" are different findings and only the second one
    names where a reader should look before deleting anything.
    """

    tier: str
    count: int

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this discounted tier."""
        return {"tier": self.tier, "count": self.count}


@dataclass(frozen=True)
class HealthSymbol:
    """One symbol in a health section, with the degree that placed it there.

    ``in_degree`` and ``out_degree`` count same-file and resolved-via-import
    edges alone. A repository-wide unique name match and a name-only-ambiguous
    match are retrieval evidence, not a binding, so counting them would report
    a symbol as reached because something somewhere spells the same word.
    They are not dropped either: each one lands in ``discounted`` under its
    tier, which is what makes an orphan row auditable instead of an assertion.
    """

    stable_id: str
    path: str
    line: int
    label: str
    kind: str
    in_degree: int
    out_degree: int
    discounted: tuple[DiscountedTier, ...] = ()

    @property
    def degree(self) -> int:
        """The counted edges at both ends of this symbol."""
        return self.in_degree + self.out_degree

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this row."""
        return {
            "stable_id": self.stable_id,
            "path": self.path,
            "line": self.line,
            "label": self.label,
            "kind": self.kind,
            "in_degree": self.in_degree,
            "out_degree": self.out_degree,
            "degree": self.degree,
            "discounted": [entry.as_dict() for entry in self.discounted],
        }


@dataclass(frozen=True)
class HealthSection(_Bounded):
    """One health finding, capped, with the size of the finding it came from.

    ``total`` is counted before the cap, so a section that lists twenty rows
    of two hundred says two hundred. A section is a listing rather than a bare
    tuple for the reason :class:`TestCompanionListing` gives: read without the
    count, twenty rows are the whole answer.
    """

    rows: tuple[HealthSymbol, ...] = ()
    total: int = 0
    limit: int = 0

    @property
    def shown(self) -> int:
        """How many rows the cap kept."""
        return len(self.rows)

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this section."""
        return {
            "total": self.total,
            "limit": self.limit,
            "omitted": self.omitted,
            "rows": [row.as_dict() for row in self.rows],
        }


@dataclass(frozen=True)
class HealthReport:
    """The three structural-health findings over one repository, in one answer.

    ``symbols`` is how many definitions the three sections were computed over
    and ``excluded`` how many the test and fixture rule left out, because a
    section that found nothing and a section that was handed nothing render
    the same sentence without them.
    """

    orphans: HealthSection
    unused_exports: HealthSection
    hubs: HealthSection
    symbols: int = 0
    excluded: int = 0

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this report."""
        return {
            "symbols": self.symbols,
            "excluded": self.excluded,
            "orphans": self.orphans.as_dict(),
            "unused_exports": self.unused_exports.as_dict(),
            "hubs": self.hubs.as_dict(),
        }


@dataclass(frozen=True)
class DiagramView:
    """Rendered mermaid text, plus what the render left out.

    ``text`` is bare mermaid with no fence: the CLI writes it into a document
    the caller fences, and the MCP tool fences it into a response body.
    ``message`` is non-empty only when there is no diagram to show.

    ``elided`` counts the modules the node bound dropped and
    ``edges_over_bound`` the reference edges the edge bound dropped, kept
    apart for the reason
    :class:`agentless_mcp.core.htmlgraph.HtmlExport` keeps them apart: one
    number for two cuts sends a reader to raise the wrong knob.

    ``rank_converged`` is false when the ranking that chose which modules to
    draw ran out of iterations. Which modules a bounded picture keeps is
    exactly what that ranking decides, so a partial ranking makes the
    selection a partial answer and the picture must say so.
    """

    text: str
    nodes: int
    elided: int
    grouped: bool
    focus: str
    message: str
    edges_over_bound: int = 0
    rank_converged: bool = True

    @property
    def caveat(self) -> str:
        """The qualifications a bounded diagram has to carry.

        A subgraph is titled after its whole community, and the rank bound
        drops members out of the picture without changing that title. Left
        unsaid, a reader counts the boxes inside a group and believes the
        count.
        """
        notes: list[str] = []
        if not self.rank_converged:
            notes.append(
                "note: the ranking that chose these modules did not converge, "
                "so which ones the bound kept is a partial answer"
            )
        if self.grouped and self.elided > 0:
            notes.append(
                "note: subgraph titles name whole communities, including the "
                f"{self.elided} module(s) the rank bound left out of this diagram"
            )
        return "\n".join(notes)

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this diagram."""
        return {
            "mermaid": self.text,
            "nodes": self.nodes,
            "elided": self.elided,
            "edges_over_bound": self.edges_over_bound,
            "grouped": self.grouped,
            "focus": self.focus,
            "message": self.message,
            "caveat": self.caveat,
            "rank_converged": self.rank_converged,
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
    scaled = _no_negative_zero(round(report.modularity, 3))
    standard = _no_negative_zero(round(report.standard_modularity, 3))
    lines = [
        (
            f"{report.total} {groups} over {report.files} files "
            f"(modularity {scaled:.3f} at resolution {report.resolution:g})"
        )
    ]
    # Named only when the knob moved it. At resolution 1.0 the two scores are
    # the same float, and printing it twice would be noise; away from 1.0 the
    # weak-partition note below is keyed on a number the header does not show,
    # which reads as a contradiction unless the number is on the page.
    if standard != scaled:
        lines.append(
            f"  modularity at resolution 1: {standard:.3f}; "
            "compare this number against 0.3, not the one above"
        )
    if report.weak_partition:
        lines.append("  note: weak partition; use these communities as a hint, not a boundary")
    for index, community in enumerate(report.communities, start=1):
        files = "file" if community.size == 1 else "files"
        lines.append(f"  {index:>3}. {one_line(community.label)}  ({community.size} {files})")
        lines.extend(f"       {one_line(member)}" for member in community.members)
        if community.omitted:
            lines.append(
                _omitted_line(community.omitted, "files in this community", limit=community.limit)
            )
    if report.omitted:
        lines.append(_omitted_line(report.omitted, "communities", limit=report.limit))
    return "\n".join(lines) + "\n"


def render_diagram(view: DiagramView) -> str:
    """Render a diagram for a response body: fenced, with its caveat below.

    The fence is added here rather than in
    :mod:`agentless_mcp.core.mermaid` because it is a property of where the
    text is going. The CLI writes ``view.text`` straight out so a caller can
    paste it into a document and choose their own fence.
    """
    if not view.text:
        return one_line(view.message or "no diagram").rstrip("\n") + "\n"

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
    """Summarise one candidate's findings, most urgent severity first."""
    tally: dict[str, int] = {}
    for finding in findings:
        tally[finding.severity] = tally.get(finding.severity, 0) + 1
    if not tally:
        return "no findings"
    return ", ".join(
        f"{tally[severity]} {one_line(severity)}" for severity in sorted(tally, key=_urgency)
    )


def _urgency(severity: str) -> tuple[int, str]:
    """Order a severity spelling by urgency, unknown spellings last."""
    known = SEVERITY_ORDER.index(severity) if severity in SEVERITY_ORDER else len(SEVERITY_ORDER)
    return (known, severity)


def render_explanation(explanation: Explanation) -> str:
    """Render one symbol card with its tiered fan-out, fan-in and imports."""
    if explanation.card is None:
        return one_line(explanation.message).rstrip("\n") + "\n"

    lines = [_render_card(explanation.card)]
    lines.extend(f"  also defined at {one_line(entry)}" for entry in explanation.alternatives)
    if explanation.rationales:
        lines.append("")
        lines.append("rationale")
        lines.extend(
            f"  {one_line(node.kind).upper()}  {one_line(node.text)}    "
            f"{_locator(node.stable_id, parent=node.parent_id, line=node.line)}"
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
        return one_line(trace.message).rstrip("\n") + "\n"

    hops = "hop" if len(trace.hops) == 1 else "hops"
    lines = [
        f"{len(trace.hops)} {hops} from {one_line(trace.source)} to {one_line(trace.target)}",
        f"  start  {one_line(trace.source_label)}    {one_line(trace.source)}",
    ]
    lines.extend(
        f"  {number:>3}. {one_line(hop.arrow)} {one_line(hop.verb)} "
        f"({one_line(hop.tier_label)})    "
        f"{one_line(hop.label)}    {_locator(hop.node, line=hop.line)}"
        for number, hop in enumerate(trace.hops, start=1)
    )
    if trace.message:
        lines.append(f"  {one_line(trace.message)}")
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
    note = _unresolved_imports_note(report.unresolved_imports)
    if not report.total:
        return "\n".join(["no import cycles", *note]) + "\n"

    cycles = "cycle" if report.total == 1 else "cycles"
    lines = [f"{report.total} import {cycles}", *note]
    for index, cycle in enumerate(report.cycles, start=1):
        lines.append(f"  {index:>3}. ({len(cycle.files)} files) {one_line(cycle.chain)}")
    if report.omitted:
        lines.append(_omitted_line(report.omitted, "cycles", limit=report.limit))
    return "\n".join(lines) + "\n"


def _unresolved_imports_note(count: int) -> list[str]:
    """Say how much of the import graph the resolver could not build.

    Returned as a list so the caller can splice it into either branch: the
    empty result needs this line more than the populated one does, because
    "no import cycles" is the reading an agent stops on.
    """
    if not count:
        return []
    statements = "statement" if count == 1 else "statements"
    return [
        (
            f"  note: {count} import {statements} named no file in this repository, "
            "so a cycle through them is not listed"
        )
    ]


def render_health(report: HealthReport) -> str:
    """Render the three structural-health findings under one shared header.

    The header states the denominator and the counting rule once, so no
    section has to repeat them and no row can be read against a rule it was
    not computed under. Each section names its own criterion in the same
    place, and every section is present even when it found nothing: an absent
    section reads as a section that was not run.
    """
    excluded = f"; {report.excluded} excluded as test or fixture paths" if report.excluded else ""
    lines = [
        f"health over {report.symbols} {_symbols(report.symbols)}{excluded}",
        (
            "degree counts same-file and resolved-via-import edges only; "
            "unique and name-only-ambiguous matches are discounted and named per row"
        ),
        (
            "methods are ranked as hubs and never reported as orphans: a call through a "
            "selector resolves to no edge, so every method would be a permanent candidate"
        ),
    ]
    lines.extend(_health_section(report.orphans, _ORPHAN_WORDING))
    lines.extend(_health_section(report.unused_exports, _UNUSED_WORDING))
    lines.extend(_health_section(report.hubs, _HUB_WORDING))
    return "\n".join(lines) + "\n"


def _symbols(count: int) -> str:
    """Spell the noun a symbol count takes."""
    return "symbol" if count == 1 else "symbols"


@dataclass(frozen=True)
class _SectionWording:
    """What one health section calls itself, whether or not it found anything.

    ``omitted`` is the noun the cut line takes when it differs from ``plural``.
    The hub section counts every symbol with an edge, so its heading has to say
    that, while the rows it cut are hubs and its cut line has to say that.
    """

    singular: str
    plural: str
    criterion: str
    empty: str
    omitted: str = ""


_ORPHAN_WORDING = _SectionWording(
    singular="orphan candidate",
    plural="orphan candidates",
    criterion="function, no counted edge in or out",
    empty="no orphan candidates: every function has a counted edge",
)

_UNUSED_WORDING = _SectionWording(
    singular="unused export",
    plural="unused exports",
    criterion="public function, no counted edge in",
    empty="no unused exports: every public function is reached",
)

# The heading counts symbols rather than hubs on purpose: every symbol with an
# edge is in the ranking, and "1164 hubs" would read as a finding about the
# repository rather than as the size of a ranked list.
_HUB_WORDING = _SectionWording(
    singular="symbol carries a counted edge",
    plural="symbols carry a counted edge",
    criterion="function, method or class, highest counted degree first",
    empty="no hubs: no function, method or class carries a counted edge",
    omitted="hubs",
)


def _health_section(section: HealthSection, wording: _SectionWording) -> list[str]:
    """Render one health section, keyed on the count before the cap.

    Keyed on ``total`` for the reason :func:`render_cycles` gives: read off
    the rows the cap left behind, ``--limit`` would turn a finding into the
    statement that the repository has none.
    """
    if not section.total:
        return ["", wording.empty]

    noun = wording.singular if section.total == 1 else wording.plural
    lines = ["", f"{section.total} {noun} ({wording.criterion})"]
    lines.extend(_health_rows(section.rows))
    if section.omitted:
        lines.append(
            _omitted_line(section.omitted, wording.omitted or wording.plural, limit=section.limit)
        )
    return lines


def _health_rows(rows: Sequence[HealthSymbol]) -> Iterator[str]:
    """Yield one row per symbol: where it is, what it is, and its two degrees."""
    for row in rows:
        discounted = ", ".join(f"{entry.count} {one_line(entry.tier)}" for entry in row.discounted)
        note = f"  -- discounted: {discounted}" if discounted else ""
        yield (
            f"  {_locator(row.stable_id, line=row.line)}  {one_line(row.label)}  "
            f"{one_line(row.kind)}  in {row.in_degree}  out {row.out_degree}{note}"
        )


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
            f"{_locator(row.node, line=row.line)}"
            for row in group.rows
        )
        if group.omitted:
            lines.append(_omitted_line(group.omitted, "edges at this tier"))
    return "\n".join(lines) + "\n"


def _render_imports(explanation: Explanation) -> str:
    """Render the import relationships of the file the symbol lives in."""
    lines = ["imports"]
    if explanation.imports_out:
        lines.extend(
            f"    declares  {one_line(row.module)} -> {one_line(row.other)}    "
            f"{_file_site(row.path, row.line)}"
            for row in explanation.imports_out
        )
        if explanation.imports_out.omitted:
            lines.append(
                _omitted_line(
                    explanation.imports_out.omitted,
                    "declared imports",
                    limit=explanation.imports_out.limit,
                )
            )
    else:
        lines.append("    declares  none resolved inside this repository")
    if explanation.imports_in:
        lines.extend(
            f"    imported by  {_file_site(row.path, row.line)}  as {one_line(row.module)}"
            for row in explanation.imports_in
        )
        if explanation.imports_in.omitted:
            lines.append(
                _omitted_line(
                    explanation.imports_in.omitted,
                    "importers",
                    limit=explanation.imports_in.limit,
                )
            )
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

    # The churn clause exists to tell ranked files apart, so a value every
    # file shares says nothing: a single-commit snapshot decorated 100% of a
    # pilot's map responses with one identical pair on every row. Two or more
    # files, one distinct pair, and the text drops the clause; a single file
    # keeps it, because there the absolute recency is the information. The
    # JSON keeps the fields either way -- suppression is a token decision,
    # not a data one.
    churn_pairs = {(entry.commits_window, entry.last_commit_days) for entry in files}
    wallpaper_churn = len(files) > 1 and len(churn_pairs) == 1

    blocks: list[str] = []
    for map_file in files:
        churn = ""
        if map_file.commits_window is not None and not wallpaper_churn:
            churn = f", {map_file.commits_window}c/{CHURN_WINDOW_DAYS}d"
            if map_file.last_commit_days is not None:
                churn += f", last {map_file.last_commit_days}d"
        lines = [f"{one_line(map_file.path)}  (rank {map_file.rank:.4f}{churn})"]
        pattern = _stable_ids_line(map_file.id_prefix, map_file.path)
        if pattern:
            lines.append(pattern)
        for entry in map_file.entries:
            lines.append(
                f"{ROW_INDENT}{'    ' * entry.depth}{one_line(entry.signature)}  "
                f"{_locator(entry.name or entry.stable_id, line=entry.line)}"
            )
            lines.extend(
                f"{ROW_INDENT}{'    ' * (entry.depth + 1)}"
                f"# {one_line(node.kind).upper()}: {one_line(node.text)}  "
                f"{_locator(node.stable_id, parent=node.parent_id, line=node.line)}"
                for node in entry.rationales
            )
        if map_file.omitted and not map_file.reached:
            # A different fact, so a different line. The ordinary marker says
            # "more ... not listed", which an agent reads as an invitation to
            # ask for the rest; here there is no budget that would produce
            # them, and saying so is the whole point of ranking this file
            # without expanding it.
            lines.append(MESSAGES.map_file_unreached.format(count=map_file.omitted))
        elif map_file.omitted:
            lines.append(_omitted_line(map_file.omitted, "symbols in this file"))
        blocks.append("\n".join(lines))
    return "\n\n".join(blocks) + "\n"


def render_test_companions(listing: TestCompanionListing) -> str:
    """Render the test files that exercise a map's ranked files, or nothing.

    An empty listing renders the empty string rather than a heading over no
    rows. Most repositories a map is asked about hold no test the ranked files
    reach, and a heading that always appears spends the caller's budget saying
    the section found nothing -- which the section's absence already says.

    Each row is one test file: its span, then the ranked files it covers. The
    span comes first because it is the part a caller pastes into a read, and
    the covered files explain why the row is here at all.

    The two are stated as separate clauses because they are measured over
    different extents. The span is one referencing symbol; the covered files
    are everything the *whole file* reaches. A row that ran them together as
    "``path:50-60`` covers a, b, c" reads as a promise that lines 50 to 60
    mention all three, and a test whose other function is what reaches ``a``
    breaks that promise. Naming the file as the subject of the coverage
    clause keeps the span honest about being an entry point rather than the
    evidence for every name beside it.
    """
    if not listing.rows:
        return "".join(_reach_exhausted_note(listing.exhausted))

    lines = ["tests exercising the files above:"]
    for row in listing.rows:
        covers = ", ".join(one_line(path) for path in row.covers)
        located = f"{one_line(row.path)}:{row.start}-{row.end}"
        lines.append(f"  {located}  -- file references {covers}" if covers else f"  {located}")
    if listing.omitted:
        lines.append(_omitted_line(listing.omitted, "test files", limit=listing.limit))
    lines.extend(_reach_exhausted_note(listing.exhausted))
    return "\n".join(lines) + "\n"


def _reach_exhausted_note(exhausted: bool) -> list[str]:
    """Say that the walk behind this section stopped before it finished.

    Returned as a list so the caller can splice it into either branch, for the
    reason :func:`_unresolved_imports_note` gives: the empty section needs the
    line more than the populated one does, because "no tests exercise these
    files" is the reading an agent stops on, and a capped walk did not earn it.
    """
    if not exhausted:
        return []
    return [
        (
            "  note: the walk out to these tests hit its node bound before it "
            "finished, so this section is a floor rather than every test"
        )
    ]


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
    # `_Bounded` owns the arithmetic, so it is what the notice is keyed on:
    # any bounded listing handed here announces its cut, and a listing added
    # later does not render as a complete answer by default. Only the concrete
    # listing knows the flag a reader would raise, so the limit is read there.
    limit = cards.limit if isinstance(cards, CardListing) else 0
    if isinstance(cards, _Bounded) and cards.omitted:
        return f"{body}{_omitted_line(cards.omitted, 'matches', limit=limit)}\n"
    return body


def render_ref_groups(
    groups: Sequence[RefGroup], target: str, *, complete_over: int | None = None
) -> str:
    """Render fan-in: references grouped by the file they were found in.

    The header is the *pre-limit* total when a :class:`RefListing` supplies
    one. Recomputing it from the groups that survived is what made a fan-in of
    fifty-two sites answer "10 references to widget" and say nothing else --
    an agent reads that as the blast radius and ships against it.

    ``complete_over`` is the file count of a scan that skipped nothing, and
    only an empty listing spends it: "no references" over a complete scan is
    an absence claim the caller can act on without a verification search,
    while the same words alone say only that this listing is empty. A caller
    that cannot vouch for the scan passes None and the line stays as it was.
    """
    if not groups:
        line = f"no references to {one_line(target)} outside its own definition"
        if complete_over is not None:
            files = "file" if complete_over == 1 else "files"
            line += f" (complete scan: {complete_over} {files}, 0 skipped)"
        return line + "\n"

    listed = sum(len(group.sites) for group in groups)
    total = groups.total if isinstance(groups, _Bounded) else listed
    # Files, not sites: the sites are cut before they are grouped, so a file
    # whose every site fell past the limit is absent from the listing
    # altogether. Only a `RefListing` counts them, so only it can say so.
    files_omitted = groups.files_omitted if isinstance(groups, RefListing) else 0
    limit = groups.limit if isinstance(groups, RefListing) else 0
    blocks = [f"{total} {_references(total)} to {one_line(target)}"]
    for group in groups:
        labelled = f", {one_line(group.tier_label)}" if group.tier_label else ""
        sites = len(group.sites)
        lines = [f"{one_line(group.path)}  ({sites} {_references(sites)}{labelled})"]
        pattern = _stable_ids_line(group.id_prefix, group.path)
        if pattern:
            lines.append(pattern)
        lines.extend(_render_site(site) for site in group.sites)
        blocks.append("\n".join(lines))
    if isinstance(groups, _Bounded) and groups.omitted:
        note = _omitted_line(groups.omitted, "references", limit=limit)
        if files_omitted:
            note += f", including every reference in {files_omitted} more files"
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

    # Partitioned here rather than trusted from the caller. The heading makes
    # a claim about every row beneath it, and emitting it at the first
    # test-defined row made that claim true only while the service happened to
    # sort on `in_tests` first.
    production = [row for row in listing.rows if not row.in_tests]
    from_tests = [row for row in listing.rows if row.in_tests]

    lines = [f"symbols sharing callers with {one_line(target)}"]
    lines.extend(_candidate_rows(production))
    if from_tests:
        lines.append("  defined in tests (ranked below all production candidates):")
        lines.extend(_candidate_rows(from_tests))
    if listing.omitted:
        lines.append(_omitted_line(listing.omitted, "candidates", limit=listing.limit))
    return "\n".join(lines) + "\n"


def _candidate_rows(rows: Sequence[SharedCaller]) -> Iterator[str]:
    """Yield one adjacency candidate and the callers that evidence it."""
    for row in rows:
        files = "file" if row.shared_files == 1 else "files"
        yield (
            f"  {_locator(row.stable_id, line=row.line)}  "
            f"({row.overlap} shared callers in {row.shared_files} {files}, "
            f"score {row.score:.3f})"
        )
        shown = row.callers[:SHARED_CALLERS_SHOWN]
        yield from (
            f"      {one_line(caller.qualname)}    {_file_site(caller.path, caller.line)}"
            for caller in shown
        )
        if len(row.callers) > len(shown):
            yield _omitted_line(
                len(row.callers) - len(shown), "callers", limit=SHARED_CALLERS_SHOWN
            )


def _render_card(card: SymbolCard) -> str:
    """Render one incident card without repeating the path inside its stable id."""
    # "in class" is the method's wording; a nested function's parent is the
    # enclosing chain, which may be a function, so it gets the bare "in".
    holder = "in class" if card.kind == "method" else "in"
    owner = f" {holder} {one_line(card.parent_class)}" if card.parent_class else ""
    span = (
        str(card.start_line)
        if card.start_line == card.end_line
        else f"{card.start_line}-{card.end_line}"
    )
    lines = [
        _locator(card.stable_id, line=span),
        f"  {one_line(card.kind)}{owner} ({one_line(card.language)})",
    ]
    if card.body:
        lines.extend(f"  {one_line(line)}" for line in card.body.split("\n"))
    else:
        lines.append(f"  {one_line(card.signature)}")
    return "\n".join(lines)


def _render_site(site: RefSite) -> str:
    """Render one reference row beneath the file header that locates it.

    One name and one position. The row used to carry the enclosing symbol's
    name and, beside it, a stable id whose tail was that same name -- the two
    differ only when a file spells one name twice and the id takes a ``#2``
    ordinal, which is the case where the id's spelling is the precise one. So
    the id's name is what the row prints, and the file header's pattern line
    rebuilds the whole id from it.
    """
    if not site.stable_id:
        return f"{ROW_INDENT}{one_line(site.enclosing)} @{site.line}"
    return f"{ROW_INDENT}{_locator(site.name or site.stable_id, line=site.line)}"
