"""Deterministic mermaid rendering of the module-level reference graph.

Mermaid is presentation, never data interchange. An agent reading this
package's answers reads the flattened text views; a diagram is for the human
looking over its shoulder, which is why this module renders on demand, returns
a string, touches no filesystem and is never a side effect of another call.

Three properties, in the order they matter.

**Safety.** Node labels are repository content: a file in an analysed
repository can be named anything its filesystem accepts, and a label is where
that name reaches a renderer. Two rules make an injected label inert rather
than merely unlikely to fire. Ids are *synthetic* -- ``n0``, ``n1``, assigned
in sorted path order and never derived from a path -- so no repository content
ever reaches the position where mermaid expects an identifier, and mermaid's
reserved words (``end``, ``click``, ``class``) cannot appear there. Labels are
*always double-quoted* and reduced to an allowlist of characters
(:func:`safe_label`), so the bracket, quote and semicolon that would close the
label and open a directive are simply not in the output. This module emits no
``click``, ``style``, ``class`` or ``linkStyle`` line at all, under any input:
the vocabulary it can produce is ``flowchart``, ``subgraph``, ``end``, node
declarations, ``-->`` and ``-.->`` edges, and ``%%`` comment lines whose text
is fixed in this module plus an integer count -- no repository content ever
reaches a comment.

**Honesty about edge kinds.** The file graph merges two different facts into
one edge set: declared imports and name-reference coincidences. Drawn with one
arrow, a pair of reference edges reads as a mutual import -- a cycle the
``cycles`` view just said does not exist. A caller that knows which pairs are
declared imports passes them as ``imports``; those render solid, reference-only
edges render dashed, and a legend comment says so. Reference edges are also
the hairball: past :data:`DEFAULT_MAX_EDGES` total edges they are left out
wholesale -- never a sampled subset, which would be a diagram lying about
which references exist -- and a comment counts what was left out. Import
edges are always drawn. Without ``imports`` the render is the undifferentiated
one it always was, because inventing a distinction the caller did not supply
would be a guess drawn as a fact.

**Determinism.** Node ids follow sorted path order, edges are emitted in id
order, communities in the order the partition gives them, and no float is ever
printed. The same graph renders to the same bytes, which is what lets a
committed diagram be regenerated and diffed rather than trusted.

**Bounding.** A repository has more modules than a diagram can show, so the
render is PageRank-bounded to :data:`DEFAULT_MAX_NODES` nodes and says what it
left out on an explicit elision line. A bounded view that does not announce
its bound is the failure this package exists to prevent.

Optional grouping draws each community from :mod:`agentless_mcp.core.communities`
as a subgraph, giving repeated labels an ascending ordinal so that two boxes
never carry the same title; optional focus keeps only the seed and its
neighbours within :data:`DEFAULT_FOCUS_DISTANCE` hops, over undirected
adjacency, before the same bounding applies. A subgraph's title describes the
whole community, including members the rank bound left out of the picture.

The output is bare mermaid text with no markdown fence. Fencing is the
caller's decision, because the CLI writes into a document and the MCP tool
writes into a response body.
"""

import re
import string
from collections.abc import Iterator, Mapping, Sequence
from collections.abc import Set as AbstractSet
from dataclasses import dataclass

from agentless_mcp.core.communities import Community, CommunityPartition
from agentless_mcp.core.graph import RefGraph

# How many modules a default diagram shows. Chosen for legibility, not for
# capacity: past roughly this many nodes a flowchart stops being readable and
# the ranked text views are the better answer.
DEFAULT_MAX_NODES = 40

# How many edges a diagram carries before reference edges stop being drawn.
# The same legibility reasoning as DEFAULT_MAX_NODES: past this many arrows a
# flowchart is a hairball, and the import edges alone are the load-bearing
# picture. Import edges are never dropped by this bound.
DEFAULT_MAX_EDGES = 40

# Hops from the focus seed. Two is the neighbourhood a "what touches this"
# question means: what it calls, and what those call.
DEFAULT_FOCUS_DISTANCE = 2

DEFAULT_DIRECTION = "LR"

# Mermaid's own flowchart directions. A caller passing anything else is a bug
# in the caller, not repository content, so it is refused rather than damped.
VALID_DIRECTIONS = frozenset({"TB", "TD", "BT", "RL", "LR"})

# Words mermaid's flowchart grammar reserves. Generated ids are `n<int>` and
# `s<int>` and so cannot collide today; the check in `_identifier` is what
# keeps that true if the prefixes ever change.
MERMAID_RESERVED_IDS = frozenset(
    {
        "graph",
        "flowchart",
        "subgraph",
        "end",
        "click",
        "call",
        "href",
        "class",
        "classdef",
        "style",
        "linkstyle",
        "direction",
        "default",
        "interpolate",
        "o",
        "x",
    }
)

# The id of the elision node. Outside the `n<int>` sequence on purpose, so a
# reader cannot mistake the marker for one of the modules.
ELISION_ID = "n_elided"

# The legend emitted whenever edges are drawn with kinds. Fixed text: it is
# the one line that makes the two arrow styles self-describing, and it never
# carries repository content.
EDGE_LEGEND = "%% solid: imports, dashed: references"

# Labels are truncated so one pathological filename cannot make a diagram
# unreadable. The marker is three dots, which the allowlist keeps.
LABEL_MAX_CHARS = 60

# The only characters that reach a rendered label. An allowlist rather than a
# blocklist: a blocklist has to be complete against every mermaid version's
# grammar and every HTML entity form, and this one has to be complete against
# what a path looks like.
ALLOWED_LABEL_CHARS = frozenset(string.ascii_letters + string.digits + " ._-/")

# What a disallowed character becomes. A single visible stand-in, so a reader
# can see that something was removed rather than reading a silently joined name.
REPLACEMENT_CHAR = "_"

# What a label reduced to nothing becomes.
UNNAMED_LABEL = "unnamed"

_RUN_OF_REPLACEMENTS = re.compile(re.escape(REPLACEMENT_CHAR) + "{2,}")
_RUN_OF_SPACES = re.compile(r" {2,}")

_INDENT = "    "


@dataclass(frozen=True)
class DiagramOptions:
    """The knobs of one render.

    A value object rather than five keyword arguments because these travel
    together from a CLI flag set or an MCP request to the renderer, and
    because a diagram is defined by them: two renders with the same options
    and the same graph are the same diagram.
    """

    max_nodes: int = DEFAULT_MAX_NODES
    max_edges: int = DEFAULT_MAX_EDGES
    focus: str | None = None
    focus_distance: int = DEFAULT_FOCUS_DISTANCE
    direction: str = DEFAULT_DIRECTION


def render_flowchart(
    graph: RefGraph,
    rank: Mapping[str, float],
    *,
    partition: CommunityPartition | None = None,
    options: DiagramOptions | None = None,
    imports: AbstractSet[tuple[str, str]] | None = None,
) -> str:
    """Render ``graph`` as mermaid flowchart text.

    ``rank`` is what bounds the diagram -- normally the personalized PageRank
    :mod:`agentless_mcp.core.graph` computed for the same graph. A node the
    ranking does not mention ranks zero rather than being dropped, so a caller
    that ranked a subset still gets every node considered.

    ``partition`` groups the selected nodes into subgraphs; nodes belonging to
    no community are drawn at the top level after them. ``options.focus``
    restricts the diagram to one module's neighbourhood before bounding.

    ``imports`` is the set of ``(source, target)`` pairs connected by a
    declared import. When given, those edges render solid, reference-only
    edges render dashed behind the ``options.max_edges`` legibility bound, and
    a legend line names the encoding. When ``None`` every edge renders solid,
    because the caller supplied no kind to distinguish.
    """
    settings = options if options is not None else DiagramOptions()
    _validate(settings)

    candidates = _candidates(graph, settings)
    selected = set(selected_nodes(graph, rank, settings))
    identifiers = {node: _identifier("n", index) for index, node in enumerate(sorted(selected))}
    edge_lines = _edge_lines(graph, identifiers, imports, settings.max_edges)

    lines = [f"flowchart {settings.direction}"]
    if imports is not None and edge_lines:
        lines.append(f"{_INDENT}{EDGE_LEGEND}")
    lines.extend(_node_lines(identifiers, partition))
    lines.extend(edge_lines)

    elided = len(candidates) - len(selected)
    if elided > 0:
        lines.append(f'{_INDENT}{ELISION_ID}["... {elided} more modules"]')

    return "\n".join(lines) + "\n"


def selected_nodes(
    graph: RefGraph,
    rank: Mapping[str, float],
    options: DiagramOptions | None = None,
) -> tuple[str, ...]:
    """Return the nodes a render with these options would draw, in id order.

    The same focus restriction and rank bound :func:`render_flowchart` applies,
    exposed on its own so a caller can say how many modules a diagram shows and
    how many it left out without reading the diagram back. Counting node
    declarations in the rendered text would count subgraph titles too, which is
    exactly the kind of "parse your own output" answer this returns instead.
    """
    settings = options if options is not None else DiagramOptions()
    _validate(settings)
    return tuple(sorted(_bounded(_candidates(graph, settings), rank, settings)))


def safe_label(text: str) -> str:
    """Reduce untrusted text to something inert inside a quoted mermaid label.

    Every character outside :data:`ALLOWED_LABEL_CHARS` becomes
    :data:`REPLACEMENT_CHAR`, runs of which are collapsed so that a name made
    mostly of punctuation does not render as a wall of underscores. The quote,
    bracket, semicolon and backtick that would end the label and start a
    directive are all outside the allowlist, so a filename spelling a mermaid
    directive renders as the text of a label and nothing else.
    """
    replaced = "".join(
        character if character in ALLOWED_LABEL_CHARS else REPLACEMENT_CHAR for character in text
    )
    collapsed = _RUN_OF_SPACES.sub(" ", _RUN_OF_REPLACEMENTS.sub(REPLACEMENT_CHAR, replaced))
    trimmed = collapsed.strip()
    if len(trimmed) > LABEL_MAX_CHARS:
        trimmed = trimmed[: LABEL_MAX_CHARS - 3] + "..."
    return trimmed or UNNAMED_LABEL


def _validate(settings: DiagramOptions) -> None:
    """Refuse option values that would render a diagram nobody asked for."""
    if settings.direction not in VALID_DIRECTIONS:
        message = f"direction must be one of {sorted(VALID_DIRECTIONS)}"
        raise ValueError(message)
    if settings.max_nodes < 1:
        message = "max_nodes must be at least 1"
        raise ValueError(message)
    if settings.max_edges < 0:
        message = "max_edges must not be negative"
        raise ValueError(message)
    if settings.focus_distance < 0:
        message = "focus_distance must not be negative"
        raise ValueError(message)


def _identifier(prefix: str, index: int) -> str:
    """Return a generated identifier that is not a mermaid reserved word.

    The guard keys on the invariant -- "this text is not a word mermaid gives
    a meaning to" -- rather than on the fact that today's prefixes happen to
    produce ``n0``. Suffixing rather than raising keeps the render total.
    """
    candidate = f"{prefix}{index}"
    if candidate.lower() in MERMAID_RESERVED_IDS:
        return candidate + "_"
    return candidate


def _candidates(graph: RefGraph, settings: DiagramOptions) -> set[str]:
    """Return the nodes eligible for the diagram, before rank bounding."""
    nodes = set(graph.nodes)
    if settings.focus is None:
        return nodes
    if settings.focus not in nodes:
        message = "focus is not a module in this graph"
        raise ValueError(message)
    return _neighbourhood(graph, settings.focus, settings.focus_distance)


def _neighbourhood(graph: RefGraph, seed: str, distance: int) -> set[str]:
    """Return the nodes within ``distance`` undirected hops of ``seed``.

    Undirected on purpose: "what is near this module" includes what imports it
    as much as what it imports, and a directed walk would answer only half of
    a blast-radius question.
    """
    known = set(graph.nodes)
    adjacent: dict[str, set[str]] = {}
    for source, target in graph.edges:
        if source in known and target in known:
            adjacent.setdefault(source, set()).add(target)
            adjacent.setdefault(target, set()).add(source)

    seen = {seed}
    frontier = [seed]
    for _ in range(distance):
        following: list[str] = []
        for node in frontier:
            for neighbour in sorted(adjacent.get(node, ())):
                if neighbour not in seen:
                    seen.add(neighbour)
                    following.append(neighbour)
        frontier = following
    return seen


def _bounded(candidates: set[str], rank: Mapping[str, float], settings: DiagramOptions) -> set[str]:
    """Keep the highest-ranked candidates, the focus seed always among them."""
    ordered = sorted(candidates, key=lambda node: (-rank.get(node, 0.0), node))
    if settings.focus is not None and settings.focus in candidates:
        ordered = [settings.focus, *(node for node in ordered if node != settings.focus)]
    return set(ordered[: settings.max_nodes])


def _node_lines(
    identifiers: Mapping[str, str], partition: CommunityPartition | None
) -> Iterator[str]:
    """Yield the node declarations, grouped into subgraphs when asked."""
    if partition is None:
        yield from (_declaration(identifiers[node], node, _INDENT) for node in sorted(identifiers))
        return

    grouped: set[str] = set()
    drawn: dict[str, int] = {}
    index = 0
    for community in partition.communities:
        members = sorted(member for member in community.members if member in identifiers)
        if not members:
            continue
        grouped.update(members)
        yield f'{_INDENT}subgraph {_identifier("s", index)}["{_group_label(community, drawn)}"]'
        yield from (_declaration(identifiers[member], member, _INDENT * 2) for member in members)
        yield f"{_INDENT}end"
        index += 1

    yield from (
        _declaration(identifiers[node], node, _INDENT)
        for node in sorted(identifiers)
        if node not in grouped
    )


def _group_label(community: Community, drawn: dict[str, int]) -> str:
    """Return a subgraph title that names one community and no other.

    Two communities can carry the same mechanical label -- ``repository root``
    covers every group whose members straddle directories -- and two boxes
    with the same title tell a reader nothing. Repeats therefore take an
    ascending ordinal in the order the partition lists them, which is stable
    because that order is. The first occurrence is left bare so the common
    case, where every label is already distinct, reads as the paths do.
    """
    label = safe_label(community.label)
    drawn[label] = drawn.get(label, 0) + 1
    if drawn[label] == 1:
        return label
    return f"{label} {drawn[label]}"


def _declaration(identifier: str, path: str, indent: str) -> str:
    """Render one node: a generated id and a quoted, sanitised label."""
    return f'{indent}{identifier}["{safe_label(path)}"]'


def _edge_lines(
    graph: RefGraph,
    identifiers: Mapping[str, str],
    imports: AbstractSet[tuple[str, str]] | None,
    max_edges: int,
) -> list[str]:
    """Render the edges with both endpoints in the diagram, in id order.

    Weights are not drawn. They are floats derived from name-collision counts,
    so printing them would put a number nobody can act on into the diagram and
    would make the output depend on float formatting.

    With ``imports`` given, declared imports render ``-->`` and reference-only
    edges ``-.->``. Reference edges are all-or-nothing against ``max_edges``:
    a sampled subset would draw "these files reference each other" for some
    pairs and silence for others with no way to tell elision from absence, so
    past the bound they are left out wholesale and a comment counts them.
    """
    drawn = sorted(
        {
            (
                identifiers[source],
                identifiers[target],
                imports is None or (source, target) in imports,
            )
            for source, target in graph.edges
            if source in identifiers and target in identifiers and source != target
        },
        key=lambda entry: _identifier_sort_key(entry[:2]),
    )
    if imports is None:
        return [f"{_INDENT}{source} --> {target}" for source, target, _ in drawn]

    references = sum(1 for _, _, declared in drawn if not declared)
    fits = len(drawn) <= max_edges
    lines: list[str] = []
    for source, target, declared in drawn:
        if declared:
            lines.append(f"{_INDENT}{source} --> {target}")
        elif fits:
            lines.append(f"{_INDENT}{source} -.-> {target}")
    if references and not fits:
        lines.append(f"{_INDENT}%% {references} reference edges not drawn (edge bound {max_edges})")
    return lines


def _identifier_sort_key(pair: Sequence[str]) -> tuple[int, int]:
    """Order edges by the numeric part of their generated ids."""
    return (_identifier_index(pair[0]), _identifier_index(pair[1]))


def _identifier_index(identifier: str) -> int:
    """Return the integer an ``n<int>`` identifier carries."""
    return int(identifier.lstrip(string.ascii_letters).rstrip("_"))
