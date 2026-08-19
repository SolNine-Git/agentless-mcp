"""Code-shaped renderers and the view models the services fill in.

Two research constraints decide everything in this module. Flattened,
code-shaped text beats structured dumps for localization, so a map is rendered
as numbered signature lines under a filename rather than as a table. And
denormalized "incident cards" carrying names, not opaque handles, measurably
beat id-only encodings -- so every card repeats the file, the line span and
the enclosing class instead of making the reader join them back together.

The view models live here rather than in the services because this module owns
the output vocabulary: a service decides *what* is worth showing, this decides
what it looks like. Nothing here reads the filesystem or parses anything.

Every path is repository-relative with forward slashes, and every row carries
``file:line``.
"""

from collections.abc import Sequence
from dataclasses import dataclass, field
from typing import Any

MODULE_LEVEL = "(module level)"


@dataclass(frozen=True)
class MapEntry:
    """One symbol line in a repository map."""

    line: int
    signature: str
    stable_id: str
    depth: int = 0

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this entry."""
        return {
            "line": self.line,
            "signature": self.signature,
            "stable_id": self.stable_id,
            "depth": self.depth,
        }


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
    """A denormalized symbol record: everything a reader needs, in one place."""

    stable_id: str
    path: str
    start_line: int
    end_line: int
    kind: str
    language: str
    signature: str
    parent_class: str = ""
    body: str = ""

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
    candidate's name is across the repository. The ranking reads off
    ``score``, so the row carries all three rather than a number a reader
    would have to take on trust.
    """

    stable_id: str
    path: str
    line: int
    overlap: int
    shared_files: int
    score: float
    callers: tuple[CallerRef, ...]

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this adjacency row."""
        return {
            "stable_id": self.stable_id,
            "path": self.path,
            "line": self.line,
            "overlap": self.overlap,
            "shared_files": self.shared_files,
            "score": round(self.score, 6),
            "shared_callers": [caller.as_dict() for caller in self.callers],
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
class TierGroup:
    """The edges of one evidence tier, already capped at the section limit."""

    tier: str
    tier_label: str
    rows: tuple[EdgeRow, ...]
    total: int

    @property
    def omitted(self) -> int:
        """How many rows of this tier the limit left out."""
        return max(0, self.total - len(self.rows))

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
    fan_out: tuple[TierGroup, ...]
    fan_in: tuple[TierGroup, ...]
    imports_out: tuple[ImportRow, ...]
    imports_in: tuple[ImportRow, ...]

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this explanation."""
        return {
            "target": self.target,
            "symbol": self.card.as_dict() if self.card is not None else None,
            "message": self.message,
            "alternatives": list(self.alternatives),
            "fan_out": [group.as_dict() for group in self.fan_out],
            "fan_in": [group.as_dict() for group in self.fan_in],
            "imports": {
                "declared": [row.as_dict() for row in self.imports_out],
                "importers": [row.as_dict() for row in self.imports_in],
            },
        }


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
            "include_ambiguous": self.include_ambiguous,
        }


@dataclass(frozen=True)
class CycleReport:
    """Every import cycle found, already capped at the caller's limit."""

    cycles: tuple[CycleRow, ...]
    total: int
    limit: int

    @property
    def omitted(self) -> int:
        """How many cycles the limit left out."""
        return max(0, self.total - len(self.cycles))

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this report."""
        return {
            "total": self.total,
            "limit": self.limit,
            "omitted": self.omitted,
            "cycles": [cycle.as_dict() for cycle in self.cycles],
        }


def render_explanation(explanation: Explanation) -> str:
    """Render one symbol card with its tiered fan-out, fan-in and imports."""
    if explanation.card is None:
        return explanation.message.rstrip("\n") + "\n"

    lines = [_render_card(explanation.card)]
    lines.extend(f"  also defined at {entry}" for entry in explanation.alternatives)
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
        f"{len(trace.hops)} {hops} from {trace.source} to {trace.target}",
        f"  start  {trace.source_label}    {trace.source}",
    ]
    lines.extend(
        f"  {number:>3}. {hop.arrow} {hop.verb} ({hop.tier_label})    "
        f"{hop.label}    {hop.path}:{hop.line}    [{hop.node}]"
        for number, hop in enumerate(trace.hops, start=1)
    )
    if trace.message:
        lines.append(f"  {trace.message}")
    return "\n".join(lines) + "\n"


def render_cycles(report: CycleReport) -> str:
    """Render module-level import cycles as arrow chains."""
    if not report.cycles:
        return "no import cycles\n"

    cycles = "cycle" if report.total == 1 else "cycles"
    lines = [f"{report.total} import {cycles}"]
    for index, cycle in enumerate(report.cycles, start=1):
        lines.append(f"  {index:>3}. ({len(cycle.files)} files) {cycle.chain}")
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
        lines.append(f"  {group.tier_label} ({group.total})")
        lines.extend(
            f"    {row.relation} {row.label}    {row.path}:{row.line}    [{row.node}]"
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
            f"    declares  {row.module} -> {row.other}    {row.path}:{row.line}"
            for row in explanation.imports_out
        )
    else:
        lines.append("    declares  none resolved inside this repository")
    if explanation.imports_in:
        lines.extend(
            f"    imported by  {row.path}:{row.line}  as {row.module}"
            for row in explanation.imports_in
        )
    else:
        lines.append("    imported by  nothing in this repository")
    return "\n".join(lines) + "\n"


def render_map(files: Sequence[MapFile]) -> str:
    """Render ranked files as code-shaped signature blocks."""
    if not files:
        return "no ranked files: nothing in this repository parsed into symbols\n"

    blocks: list[str] = []
    for map_file in files:
        lines = [f"{map_file.path}  (rank {map_file.rank:.4f})"]
        lines.extend(
            f"{entry.line:>5}| {'    ' * entry.depth}{entry.signature}  [{entry.stable_id}]"
            for entry in map_file.entries
        )
        if map_file.omitted:
            lines.append(f"      ... {map_file.omitted} more symbols in this file")
        blocks.append("\n".join(lines))
    return "\n\n".join(blocks) + "\n"


def render_symbol_cards(cards: Sequence[SymbolCard]) -> str:
    """Render incident cards for symbol lookups and expansions."""
    if not cards:
        return "no matching symbols\n"
    return "\n\n".join(_render_card(card) for card in cards) + "\n"


def render_ref_groups(groups: Sequence[RefGroup], target: str) -> str:
    """Render fan-in: references grouped by the file they were found in."""
    if not groups:
        return f"no references to {target} outside its own definition\n"

    total = sum(len(group.sites) for group in groups)
    blocks = [f"{total} references to {target}"]
    for group in groups:
        labelled = f", {group.tier_label}" if group.tier_label else ""
        lines = [f"{group.path}  ({len(group.sites)} references{labelled})"]
        lines.extend(_render_site(group.path, site) for site in group.sites)
        blocks.append("\n".join(lines))
    return "\n\n".join(blocks) + "\n"


def render_shared_callers(rows: Sequence[SharedCaller], target: str) -> str:
    """Render the adjacency view: symbols called by the same callers.

    Ranked strongest first, and every line -- the candidate and each caller --
    carries its own ``file:line``, so the DRY question is answered with
    somewhere to go rather than with a list of names.
    """
    if not rows:
        return f"no symbols share callers with {target}\n"

    lines = [f"symbols sharing callers with {target}"]
    for row in rows:
        files = "file" if row.shared_files == 1 else "files"
        lines.append(
            f"  {row.stable_id}    {row.path}:{row.line}  "
            f"({row.overlap} shared callers in {row.shared_files} {files}, "
            f"score {row.score:.3f})"
        )
        lines.extend(
            f"      {caller.qualname}    {caller.path}:{caller.line}" for caller in row.callers
        )
    return "\n".join(lines) + "\n"


def number_lines(text: str, first_line: int = 1) -> str:
    """Render ``text`` with ``N| `` prefixes starting at ``first_line``."""
    lines = text.split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    return "\n".join(f"{number}| {line}" for number, line in enumerate(lines, start=first_line))


def _render_card(card: SymbolCard) -> str:
    """Render one incident card: id, location line, signature, optional body."""
    owner = f" in class {card.parent_class}" if card.parent_class else ""
    lines = [
        card.stable_id,
        f"  {card.path}:{card.start_line}-{card.end_line}  {card.kind}{owner} ({card.language})",
    ]
    if card.body:
        lines.extend(f"  {line}" for line in card.body.split("\n"))
    else:
        lines.append(f"  {card.signature}")
    return "\n".join(lines)


def _render_site(path: str, site: RefSite) -> str:
    """Render one reference row: file:line plus who encloses it."""
    suffix = f"  [{site.stable_id}]" if site.stable_id else ""
    return f"{site.line:>5}| {site.enclosing}{suffix}    {path}:{site.line}"
