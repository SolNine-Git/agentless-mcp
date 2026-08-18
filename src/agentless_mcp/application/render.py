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
    """The references to one symbol from one file."""

    path: str
    sites: tuple[RefSite, ...] = field(default_factory=tuple)

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this group."""
        return {
            "path": self.path,
            "count": len(self.sites),
            "sites": [site.as_dict() for site in self.sites],
        }


@dataclass(frozen=True)
class SharedCaller:
    """A symbol that shares callers with the target of a refs query."""

    stable_id: str
    overlap: int
    callers: tuple[str, ...]

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this adjacency row."""
        return {
            "stable_id": self.stable_id,
            "overlap": self.overlap,
            "shared_callers": list(self.callers),
        }


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
        lines = [f"{group.path}  ({len(group.sites)} references)"]
        lines.extend(_render_site(group.path, site) for site in group.sites)
        blocks.append("\n".join(lines))
    return "\n\n".join(blocks) + "\n"


def render_shared_callers(rows: Sequence[SharedCaller], target: str) -> str:
    """Render the adjacency view: symbols called by the same callers."""
    if not rows:
        return f"no symbols share callers with {target}\n"

    lines = [f"symbols sharing callers with {target}"]
    lines.extend(
        f"  {row.stable_id}  ({row.overlap} shared: {', '.join(row.callers)})" for row in rows
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
