"""Symbol-level resolution: which definition a spelled name actually names.

:mod:`agentless_mcp.core.refs` deliberately keeps a reference as a
``(file, name, line)`` triple, because binding a name to a declaration needs
type information this tool does not have. Syntactic roles and lexical bindings
travel with each reference, and assignments, parameters, loop targets,
imports, labels, and unrelated attribute members produce no bare symbol edge.
This module goes as far past that as
evidence allows and stops there: it uses the imports the file itself declares,
and the file the name is spelled in, to sort candidate definitions into four
**discrete tiers**. There is no score, no threshold and no weighting -- a tier
is a statement about what kind of evidence exists, and a reader can audit it.

**The tiers, strongest first.**

``same_file``
    The file spelling the name defines it. A local definition shadows an
    import in every language this package parses, so this outranks everything.
``imported``
    A file this file imports defines the name, and the import either names it
    directly (``from x import n``) or brings in the whole module. The
    strongest cross-file evidence there is: a declared dependency.
``unique``
    Exactly one definition of that name exists in the repository. Nothing
    connects the two files, but there is nothing else it could be either.
``ambiguous``
    Several definitions, no import and no local one. Every candidate is kept
    and reported -- picking one silently is the failure mode this tier exists
    to prevent.

Precedence is that order, applied to the whole candidate set at once: the
first tier with any candidate wins and the others are not consulted. It
mirrors Python's scoping intuition without encoding any language's rules --
"defined here" beats "declared as coming from there" beats "there is only one"
-- and it is a total function of the scan, so two resolutions of an unchanged
tree agree edge for edge.

**Attribution.** A reference is attributed to the innermost symbol whose span
contains it, which the scan already knows how to find; a reference outside
every symbol is attributed to its file. Both are nodes in the graph this
module builds, and a path may run through either.

Nothing here is persisted. The edge set is a pure function of one
:class:`~agentless_mcp.core.refs.RepoScan`, rebuilt per call, so its freshness
is exactly the freshness of the scan that produced it -- which the per-file
sha256 gate already guarantees.
"""

from collections.abc import Callable, Iterable, Iterator, Mapping, Sequence
from dataclasses import dataclass, replace
from enum import Enum
from typing import Protocol

from agentless_mcp.core import graph
from agentless_mcp.core.extractor import IdentifierRole
from agentless_mcp.core.imports import ImportStatement
from agentless_mcp.core.refs import Definition, FileFacts, RefIndex, RepoScan, line_owners
from agentless_mcp.core.symbols import ASTSymbol, qualname, symbol_stable_id

# A path search that has looked at this many nodes has stopped answering the
# question it was asked. The bound is a parameter everywhere it matters; this
# is what a caller gets for not naming one.
DEFAULT_MAX_VISITED = 20_000

# A component of one file is not a cycle: a module that imports itself is a
# typo, not a dependency knot, and the import pass drops that edge anyway.
_SMALLEST_CYCLE = 2

# Bases arrive as source text: `Generic[T]`, `enum.Enum`, `metaclass=ABCMeta`.
# Only the last dotted component of the un-subscripted head is a name this
# module can look up.
_SUBSCRIPT_OPEN = "["
_KEYWORD_BASE = "="


class FileImports(Protocol):
    """The two facts import resolution reads off a file.

    Narrower than :class:`~agentless_mcp.core.refs.FileFacts` on purpose.
    :func:`import_graph` is reached with facts whose symbol and reference
    tables belong to the *pre-patch* text -- :mod:`agentless_mcp.core.patchlint`
    hands over exactly that -- so a parameter naming the whole record would
    license reading a field the caller has already been told is stale. This
    names what is read, which makes the caveat unrepresentable rather than
    documented: an edit that reaches for ``symbols`` here does not type-check.
    """

    @property
    def path(self) -> str:
        """The file's repository-relative path."""

    @property
    def imports(self) -> Sequence[ImportStatement]:
        """The import statements the file declares."""


@dataclass(frozen=True)
class PathEdgePolicy:
    """Select which non-binding evidence tiers a path may traverse."""

    include_unique: bool = False
    include_ambiguous: bool = False


DEFAULT_PATH_EDGE_POLICY = PathEdgePolicy()


class Tier(str, Enum):
    """How strong the evidence behind one resolved edge is.

    ``value`` is the wire form and ``label`` the phrase rendered for a reader;
    the two differ because ``ambiguous`` on its own reads as a defect rather
    than as "this row matched on the name alone".
    """

    SAME_FILE = "same_file"
    IMPORTED = "imported"
    UNIQUE = "unique"
    AMBIGUOUS = "ambiguous"

    @property
    def label(self) -> str:
        """Return the phrase this tier is rendered as."""
        return _TIER_LABELS[self]


_TIER_LABELS: dict[Tier, str] = {
    Tier.SAME_FILE: "same-file",
    Tier.IMPORTED: "resolved-via-import",
    Tier.UNIQUE: "unique",
    Tier.AMBIGUOUS: "name-only-ambiguous",
}

# Strongest first. The order is the precedence rule, in one place, so a
# renderer grouping by tier and the resolver choosing one cannot disagree.
TIER_ORDER: tuple[Tier, ...] = (Tier.SAME_FILE, Tier.IMPORTED, Tier.UNIQUE, Tier.AMBIGUOUS)

_TIER_RANK: dict[Tier, int] = {tier: rank for rank, tier in enumerate(TIER_ORDER)}


class Relation(str, Enum):
    """What one edge says the source does to the target."""

    REFERENCES = "references"
    IMPORTS = "imports"
    INHERITS = "inherits"


@dataclass(frozen=True)
class Endpoint:
    """One end of an edge: a symbol, or a whole file.

    ``node`` is the identity a path search and a lookup use -- a stable id for
    a symbol, the repository-relative path for a file. The rest is
    denormalized so a rendered row carries where to go without a join.
    """

    node: str
    path: str
    line: int
    label: str
    is_symbol: bool

    @property
    def location(self) -> str:
        """Return ``file:line``, or just the file for a whole-file endpoint."""
        return f"{self.path}:{self.line}" if self.line else self.path


@dataclass(frozen=True)
class SymbolEdge:
    """One resolved relationship, with the evidence tier behind it."""

    source: Endpoint
    target: Endpoint
    name: str
    relation: Relation
    tier: Tier

    @property
    def sort_key(self) -> tuple[str, str, str, int, str, int, int]:
        """Return the total order every edge list is sorted by."""
        return (
            self.source.node,
            self.target.node,
            self.relation.value,
            _TIER_RANK[self.tier],
            self.name,
            self.source.line,
            self.target.line,
        )


@dataclass(frozen=True)
class Resolution:
    """The tier one name resolved at, and every candidate at that tier."""

    name: str
    tier: Tier
    candidates: tuple[Definition, ...]


@dataclass(frozen=True)
class ResolvedImport:
    """One import statement and the repository file it named, if any.

    ``resolved`` is ``None`` when no file in this repository answered the
    module string, which is what
    :func:`agentless_mcp.core.graph.resolve_import_target` returns for that
    case. It used to be flattened to an empty string inside a bare
    ``tuple[str, int, str]``, so the one value meaning "this import proves
    nothing" was spelled as a name.
    """

    module: str
    line: int
    resolved: str | None
    # Whether the statement was spelled against the importing file's own
    # directory. Kept because the module string cannot be asked afterwards:
    # `from .missing import x` records the module `missing`, which names
    # nothing about this repository, while the statement names nothing else.
    relative: bool = False


@dataclass(frozen=True)
class ImportScope:
    """What one file's own import statements bind, resolved to repository files.

    ``modules`` holds the files imported wholesale -- ``import pricing``,
    ``import "./pricing"`` -- where every definition inside is reachable.
    ``named`` holds the ``from x import n`` form, where only ``n`` is.
    """

    wholesale: frozenset[str]
    module_bindings: Mapping[str, frozenset[str]]
    named: Mapping[str, frozenset[str]]
    statements: tuple[ResolvedImport, ...]

    def resolved_edges(self, path: str) -> Iterator[tuple[str, int, str]]:
        """Yield ``(module, line, target)`` for the statements that are edges.

        The one home for "which resolved statement becomes an import edge".
        The rule had drifted into two: :func:`_import_edges` here and
        ``graph_service._import_pairs``, which fed the module diagram. Two
        copies of one predicate is how a solid edge on a diagram comes to
        disagree with the edge list ``explain``, ``path`` and ``cycles`` are
        drawn from.

        A file that imports itself is dropped for the reason :func:`_edge`
        gives: it is real, and it says nothing about how two files relate.
        """
        for statement in self.statements:
            if statement.resolved is None or statement.resolved == path:
                continue
            yield statement.module, statement.line, statement.resolved

    def binds(self, name: str, path: str) -> bool:
        """True when this file's imports bring ``name`` in from ``path``.

        ``wholesale`` was called ``modules`` and held the target of every
        module import, which made "this file imported some module that
        happens to define this spelling" indistinguishable from "this file
        imported this name". Both answered at ``resolved-via-import``, the
        tier a caller is told to read as a caller.

        Reproduced against the shipped server on this repository:
        ``adapters/cli/main.py`` imports ``resolve_repo`` by name from
        ``application.repo_context`` and imports ``core.resolve`` as a module,
        and a bare ``resolve_repo`` resolved to *both* files at
        ``resolved-via-import`` -- so ``find_referencing_symbols`` on
        ``core.resolve.resolve_repo`` listed ``main.py`` as a caller of a
        function main.py does not import. 42 bare-reference sites on this
        repository rested on that arm.

        It now holds only the imports that genuinely bring every name in
        unqualified: C's ``#include`` and Python's ``from x import *``.
        """
        return path in self.wholesale or path in self.named.get(name, frozenset())


@dataclass(frozen=True)
class Resolver:
    """The one place a name plus a file becomes a tier and a candidate set."""

    index: RefIndex
    scopes: Mapping[str, ImportScope]

    def resolve(self, name: str, path: str) -> Resolution | None:
        """Resolve ``name`` as spelled in ``path``, or None when nothing defines it."""
        candidates = self.index.definitions.get(name, ())
        if not candidates:
            return None

        ordered = _ordered(candidates)
        same_file = tuple(entry for entry in ordered if _in_module_scope(entry, path))
        if same_file:
            return Resolution(name=name, tier=Tier.SAME_FILE, candidates=same_file)

        scope = self.scopes.get(path)
        if scope is not None:
            imported = tuple(entry for entry in ordered if scope.binds(name, entry.path))
            if imported:
                return Resolution(name=name, tier=Tier.IMPORTED, candidates=imported)

        if len(ordered) == 1:
            return Resolution(name=name, tier=Tier.UNIQUE, candidates=ordered)
        return Resolution(name=name, tier=Tier.AMBIGUOUS, candidates=ordered)

    def resolve_module_attribute(
        self,
        name: str,
        path: str,
        qualifier: str,
    ) -> Resolution | None:
        """Resolve ``qualifier.name`` only through that repository module."""
        scope = self.scopes.get(path)
        if scope is None:
            return None
        targets = scope.module_bindings.get(qualifier, frozenset())
        candidates = tuple(
            entry
            for entry in _ordered(self.index.definitions.get(name, ()))
            if entry.path in targets
        )
        if not candidates:
            return None
        return Resolution(name=name, tier=Tier.IMPORTED, candidates=candidates)


@dataclass(frozen=True)
class ResolvedGraph:
    """Every resolved edge in one repository, in one deterministic order.

    Two counts, because one of them cannot answer the question it is asked.

    ``unresolved_imports`` counts every import statement that named no file in
    this repository, which is literally true of ``import json`` and always will
    be. Measured on this repository (2026-08-23): 518 of 1238 statements, and a
    repository whose own imports all resolve would still report several
    hundred. Read as coverage it says a repository was barely searched when
    nothing was missed.

    ``unresolved_internal_imports`` counts only the statements that named
    something this repository holds -- a relative import, or one whose leading
    segment is a directory or file here -- and failed anyway. That is the
    number a claim drawn from these edges has to be read against, the cycle
    list above all, because it is the one that is zero when nothing was
    missed. Measured on the same tree, the same day: 6.

    Both travel with the graph rather than being recomputed beside it, because
    "nothing found" and "little was searched" must not render identically and
    only one of the two numbers can tell them apart.
    """

    edges: tuple[SymbolEdge, ...]
    definitions: Mapping[str, Definition]
    files: tuple[str, ...]
    unresolved_imports: int = 0
    unresolved_internal_imports: int = 0

    def outgoing(self) -> dict[str, tuple[SymbolEdge, ...]]:
        """Return the edges leaving each node, keyed by node id."""
        return _group(self.edges, lambda edge: edge.source.node)

    def incoming(self) -> dict[str, tuple[SymbolEdge, ...]]:
        """Return the edges arriving at each node, keyed by node id."""
        return _group(self.edges, lambda edge: edge.target.node)

    def import_edges(self) -> tuple[SymbolEdge, ...]:
        """Return the module-level import edges alone."""
        return tuple(edge for edge in self.edges if edge.relation is Relation.IMPORTS)


@dataclass(frozen=True)
class Hop:
    """One step along a path, and whether it was walked with or against the edge."""

    edge: SymbolEdge
    forward: bool

    @property
    def origin(self) -> Endpoint:
        """The endpoint this hop started from."""
        return self.edge.source if self.forward else self.edge.target

    @property
    def arrival(self) -> Endpoint:
        """The endpoint this hop reached."""
        return self.edge.target if self.forward else self.edge.source


@dataclass(frozen=True)
class PathResult:
    """The outcome of one path search: hops, or why there are none."""

    source: str
    target: str
    hops: tuple[Hop, ...]
    found: bool
    visited: int
    exhausted: bool


@dataclass(frozen=True)
class Cycle:
    """One import cycle, as the chain of files that closes it."""

    files: tuple[str, ...]

    @property
    def chain(self) -> str:
        """Render the cycle as ``a -> b -> a``."""
        return " -> ".join([*self.files, self.files[0]])


def _bind_module_object(
    facts: FileImports,
    statement: ImportStatement,
    known: frozenset[str],
    module_bindings: dict[str, set[str]],
) -> None:
    """Record the local name a module-object import binds, and what it names.

    The name and its target are one question, and `import a.b` answers both
    differently from `import a.b as ab`. Unaliased, Python binds `a` -- the
    *package* -- so `a.in_init()` is a call into `a/__init__.py` and not into
    the submodule the statement happens to name. Aliased, it binds `ab` to the
    submodule, and `a` is a name the file never binds at all.

    So the target is resolved from the module the local name refers to, not
    from the statement's own dotted path. Attributing `a.in_init()` to
    `a/b.py` names the one file that does not define it.
    """
    referenced = statement.module if statement.alias else statement.module.split(".")[0]
    binding = statement.alias or referenced
    if not binding:
        return
    target = graph.resolve_import_target(facts.path, replace(statement, module=referenced), known)
    if target is None or target == facts.path:
        return
    module_bindings.setdefault(binding, set()).add(target)


def build_file_scopes(files: Sequence[FileImports]) -> dict[str, ImportScope]:
    """Resolve every file's import statements to repository files, once.

    Takes the file facts rather than a whole scan, because a caller holding a
    *hypothetical* set of files -- the repository as one patch would leave it
    -- has no scan to hand over and must not have to invent one.

    Import resolution is best effort by construction (see
    :func:`agentless_mcp.core.graph.resolve_import_target`): a module string is
    written for the importing language's own resolver. An unresolved import
    contributes no evidence, which costs a tier rather than an answer -- the
    reference still resolves as ``unique`` or ``ambiguous``.
    """
    known = frozenset(facts.path for facts in files)
    scopes: dict[str, ImportScope] = {}

    for facts in files:
        wholesale: set[str] = set()
        module_bindings: dict[str, set[str]] = {}
        named: dict[str, set[str]] = {}
        statements: list[ResolvedImport] = []

        for statement in facts.imports:
            target = graph.resolve_import_target(facts.path, statement, known)
            statements.append(
                ResolvedImport(
                    module=statement.module,
                    line=statement.line_number,
                    resolved=target,
                    relative=statement.is_relative or bool(statement.relative_level),
                )
            )
            # Named rather than a bool so the narrowing is the type's, not a
            # reader's: a file importing itself is not evidence about itself.
            bound = target if target is not None and target != facts.path else None
            if statement.binds_all and bound is not None:
                # `#include` and `from x import *`: every name the target
                # defines is spellable here, so a bare reference to one of
                # them really is evidence of this import.
                wholesale.add(bound)
            if not statement.names:
                _bind_module_object(facts, statement, known, module_bindings)
                continue
            for member, local in statement.bound_names():
                dotted = f"{statement.module}.{member}" if statement.module else member
                submodule = graph.resolve_imported_submodule(
                    facts.path,
                    statement,
                    member,
                    known,
                )
                if submodule is not None and submodule != facts.path:
                    # `from pkg import mod` binds the name `mod` to a module
                    # object. It does not bring `mod`'s contents into this
                    # file, which is why the submodule is no longer added to
                    # the wholesale set: a bare reference to something
                    # `pkg/mod.py` defines is a NameError here.
                    #
                    # Keyed on the local name: `from pkg import mod as m`
                    # spells `m` here, and `mod` is a name this file never
                    # binds.
                    named.setdefault(local, set()).add(submodule)
                    module_bindings.setdefault(local, set()).add(submodule)
                    statements.append(
                        ResolvedImport(
                            module=dotted,
                            line=statement.line_number,
                            resolved=submodule,
                            relative=statement.is_relative or bool(statement.relative_level),
                        )
                    )
                elif bound is not None:
                    named.setdefault(local, set()).add(bound)

        scopes[facts.path] = ImportScope(
            wholesale=frozenset(wholesale),
            module_bindings={name: frozenset(paths) for name, paths in module_bindings.items()},
            named={name: frozenset(paths) for name, paths in named.items()},
            statements=tuple(statements),
        )

    return scopes


def build_resolver(scan: RepoScan, index: RefIndex) -> Resolver:
    """Build the resolver one call's views share."""
    return Resolver(index=index, scopes=build_file_scopes(scan.files))


def import_graph(files: Sequence[FileImports]) -> ResolvedGraph:
    """Assemble the module-level import edges alone, resolving no references.

    The cycle question is about modules, so it needs one third of
    :func:`build_graph` and none of its cost. Separating it is what lets a
    caller build the import graph of a repository *twice* -- once as it is,
    once as a patch would leave it -- without resolving every identifier in
    the tree twice to find out whether a knot appeared.

    The parameter names only ``path`` and ``imports``, which is all this call
    reads, so a caller may hand over facts whose symbol and reference tables
    belong to the pre-patch text; the returned graph has no symbol definitions
    for the same reason.
    """
    scopes = build_file_scopes(files)
    edges: list[SymbolEdge] = []
    for facts in files:
        edges.extend(_import_edges(facts, scopes.get(facts.path)))

    return ResolvedGraph(
        edges=tuple(sorted(set(edges), key=lambda edge: edge.sort_key)),
        definitions={},
        files=tuple(sorted(facts.path for facts in files)),
        unresolved_imports=_unresolved_imports(scopes.values()),
        unresolved_internal_imports=_unresolved_internal_imports(scopes.values(), scopes.keys()),
    )


def build_graph(scan: RepoScan, resolver: Resolver) -> ResolvedGraph:
    """Resolve every reference, import and base class in ``scan`` into edges.

    Three passes over the same files, all deterministic, and the result is
    sorted explicitly at the end: an unchanged tree produces a byte-identical
    edge list whatever order the filesystem hands the walk back in.

    Identical edges collapse. A function that calls another twenty times is
    one relationship, and the graph is asked how things relate, not how often;
    ``find_referencing_symbols`` is where every individual site is still
    listed with its own line number. References outside every symbol are the
    exception: they are attributed to the file *at their own line*, so a file
    that names a symbol at three top-level lines keeps three of them -- the
    import line and the module-level use are different facts.
    """
    edges: list[SymbolEdge] = []
    definitions: dict[str, Definition] = {}

    for facts in scan.files:
        owners = line_owners(facts)
        for symbol in facts.symbols:
            definitions[symbol_stable_id(symbol)] = Definition(path=facts.path, symbol=symbol)

        edges.extend(_reference_edges(facts, owners, resolver))
        edges.extend(_inherit_edges(facts, resolver))
        edges.extend(_import_edges(facts, resolver.scopes.get(facts.path)))

    return ResolvedGraph(
        edges=tuple(sorted(set(edges), key=lambda edge: edge.sort_key)),
        definitions=definitions,
        files=tuple(sorted(facts.path for facts in scan.files)),
        unresolved_imports=_unresolved_imports(resolver.scopes.values()),
        unresolved_internal_imports=_unresolved_internal_imports(
            resolver.scopes.values(), resolver.scopes.keys()
        ),
    )


def resolve_repo(scan: RepoScan, index: RefIndex) -> tuple[Resolver, ResolvedGraph]:
    """Build the resolver and the whole edge set for one scan."""
    resolver = build_resolver(scan, index)
    return resolver, build_graph(scan, resolver)


def shortest_path(
    resolved: ResolvedGraph,
    source: str,
    target: str,
    *,
    edge_policy: PathEdgePolicy = DEFAULT_PATH_EDGE_POLICY,
    max_visited: int = DEFAULT_MAX_VISITED,
) -> PathResult:
    """Find the fewest-hop path between two nodes, edges treated as undirected.

    Undirected on purpose: "how are these two related" is not a question about
    call direction, and a caller reading the hops is shown each edge's real
    direction anyway. ``unique`` and ``ambiguous`` edges are excluded unless
    asked for: repository-wide name uniqueness is useful retrieval evidence,
    but it is not binding evidence strong enough for a path that reads like an
    architecture claim.

    The search is bounded by ``max_visited`` and reports whether it hit that
    bound, so "no path" and "stopped looking" are never the same answer.
    """
    usable = [
        edge
        for edge in resolved.edges
        if (edge_policy.include_unique or edge.tier is not Tier.UNIQUE)
        and (edge_policy.include_ambiguous or edge.tier is not Tier.AMBIGUOUS)
    ]
    adjacency = _undirected(usable)

    if source == target:
        # Not checked against the graph: an endpoint reaches this function
        # already resolved to a node (`application.graph_service`), and a node
        # is trivially related to itself whether or not it carries edges.
        return PathResult(
            source=source, target=target, hops=(), found=True, visited=1, exhausted=False
        )

    previous: dict[str, tuple[str, Hop]] = {}
    seen = {source}
    frontier = [source]
    visited = 0
    exhausted = False

    while frontier and not exhausted:
        following: list[str] = []
        for node in frontier:
            visited += 1
            if visited > max_visited:
                exhausted = True
                break
            for hop in adjacency.get(node, ()):
                reached = hop.arrival.node
                if reached in seen:
                    continue
                seen.add(reached)
                previous[reached] = (node, hop)
                if reached == target:
                    return PathResult(
                        source=source,
                        target=target,
                        hops=_unwind(previous, source, target),
                        found=True,
                        visited=visited,
                        exhausted=False,
                    )
                following.append(reached)
        frontier = following

    return PathResult(
        source=source,
        target=target,
        hops=(),
        found=False,
        visited=visited,
        exhausted=exhausted,
    )


def import_cycles(resolved: ResolvedGraph) -> tuple[Cycle, ...]:
    """Return the import cycles the resolved imports prove, deterministically.

    Not every cycle the repository holds. An import this package could not
    resolve to a file contributes no edge, so it can hide a knot that is
    really there. ``resolved.unresolved_internal_imports`` is how many
    statements were in that position *and* named something this repository
    holds, and it is what an empty result has to be read against;
    ``unresolved_imports`` counts every unresolved statement, most of which
    name the standard library and never could have resolved.

    Tarjan's strongly connected components over the import edges alone: two
    files are in one cycle exactly when each can reach the other by following
    imports. Every component of more than one file is a cycle, and the chain
    reported for it is a real walk back to where it started -- not the
    component's members in sorted order, which need not be an edge sequence.
    """
    adjacency: dict[str, list[str]] = {}
    for edge in resolved.import_edges():
        adjacency.setdefault(edge.source.node, []).append(edge.target.node)
    ordered = {node: tuple(sorted(set(targets))) for node, targets in adjacency.items()}

    cycles: list[Cycle] = []
    for component in _components(resolved.files, ordered):
        if len(component) < _SMALLEST_CYCLE:
            continue
        chain = _cycle_chain(component, ordered)
        if chain:
            cycles.append(Cycle(files=chain))

    cycles.sort(key=lambda cycle: (len(cycle.files), cycle.files))
    return tuple(cycles)


def _unresolved_imports(scopes: Iterable[ImportScope]) -> int:
    """Count the import statements that named no file in this repository."""
    return sum(
        1 for scope in scopes for statement in scope.statements if statement.resolved is None
    )


def _repository_segments(paths: Iterable[str]) -> frozenset[str]:
    """Return every directory name and file stem the repository spells.

    What an import has to lead with to be about this repository at all.
    ``agentless_mcp`` is here because a directory carries that name;
    ``json`` is not, unless the tree happens to hold a file or directory of
    that name -- in which case an import leading with it really could have
    meant this repository, and counting it is the safe direction.
    """
    segments: set[str] = set()
    for path in paths:
        parts = path.split("/")
        segments.update(parts[:-1])
        segments.add(parts[-1].rpartition(".")[0] or parts[-1])
    return frozenset(segments)


def _unresolved_internal_imports(scopes: Iterable[ImportScope], paths: Iterable[str]) -> int:
    """Count the unresolved imports that named something this repository holds.

    A relative import always did: it is spelled against the importing file's
    own directory and can name nothing else, which is why
    :class:`ResolvedImport` keeps that fact rather than re-reading the module
    string -- ``from .missing import x`` records the module ``missing``. An
    absolute one did when its
    leading segment is a directory or a file here -- which is what separates
    ``from agentless_mcp.application import X``, a resolution this package
    owes an answer to, from ``import json``, which it never could.
    """
    segments = _repository_segments(paths)
    return sum(
        1
        for scope in scopes
        for statement in scope.statements
        if statement.resolved is None
        and (statement.relative or _names_this_repository(statement.module, segments))
    )


def _names_this_repository(module: str, segments: frozenset[str]) -> bool:
    """True when a module string leads with a name this repository spells."""
    text = module.strip()
    if not text:
        return False
    lead = text.replace("\\", "/").replace("::", "/").replace(".", "/").split("/", 1)[0]
    return bool(lead) and lead in segments


def base_name(text: str) -> str:
    """Return the looked-up name of a base-class expression, or an empty string.

    ``Generic[T]`` is ``Generic``, ``enum.Enum`` is ``Enum``, and
    ``metaclass=ABCMeta`` is nothing at all: a keyword argument in a base list
    is not a base.
    """
    head = text.split(_SUBSCRIPT_OPEN, 1)[0].strip()
    if not head or _KEYWORD_BASE in head:
        return ""
    return head.rpartition(".")[2].strip()


def _reference_edges(
    facts: FileFacts,
    owners: Mapping[int, ASTSymbol],
    resolver: Resolver,
) -> list[SymbolEdge]:
    """Resolve one file's identifier references into edges."""
    declarations = {(symbol.name, symbol.line_number) for symbol in facts.symbols}
    edges: list[SymbolEdge] = []

    for ref in facts.refs:
        if not ref.is_resolvable:
            # A syntactic binding, label, declaration, or attribute member is
            # not a bare repository reference at any evidence tier.
            continue
        if (ref.name, ref.line) in declarations:
            # The identifier in `def quote` is the declaration, not a use of
            # it. Only Python records that as a role the filter above can
            # read; `collect_refs` calls every other identifier a REFERENCE,
            # so this is where the other nineteen languages are answered.
            #
            # It is a proxy -- "a symbol of this name starts on this line" --
            # and two tighter keys were measured against the edge set of this
            # repository (175 files, 20 languages, 10,731 reference edges).
            # Marking the tree-sitter `name`-field child a DECLARATION in the
            # extractor adds 35,939 edges and drops 215: JSON, YAML and TOML
            # keys sit behind no `name` field, so every key becomes a
            # reference to every other key of that spelling. Dropping only the
            # first match per line reproduces this edge set exactly here, and
            # on `Helper Helper() { return new Helper(); }` it keeps the
            # declaration identifier and drops the return type -- a wrong edge
            # traded for a missing one, which is the wrong direction.
            #
            # So the cost is paid where it is cheapest: 1,086 of 14,947
            # resolvable references are dropped, none of them a second
            # occurrence sharing a line with a declaration. Closing it for
            # real means recording the role per language in the extractor.
            continue
        resolution = (
            resolver.resolve_module_attribute(ref.name, facts.path, ref.qualifier)
            if ref.role is IdentifierRole.MODULE_ATTRIBUTE
            else resolver.resolve(ref.name, facts.path)
        )
        if resolution is None:
            continue

        owner = owners.get(ref.line)
        source = (
            _symbol_endpoint(facts.path, owner)
            if owner is not None
            else file_endpoint(facts.path, ref.line)
        )
        _add(edges, source, resolution, Relation.REFERENCES)

    return edges


def _inherit_edges(facts: FileFacts, resolver: Resolver) -> list[SymbolEdge]:
    """Resolve one file's declared base classes into edges.

    Only symbols that already carry ``bases`` produce these: the extractor is
    the authority on what a declaration says, and this module does not reach
    past what it recorded.
    """
    edges: list[SymbolEdge] = []
    for symbol in facts.symbols:
        source = _symbol_endpoint(facts.path, symbol)
        for base in symbol.bases:
            name = base_name(base)
            if not name:
                continue
            resolution = resolver.resolve(name, facts.path)
            if resolution is None:
                continue
            _add(edges, source, resolution, Relation.INHERITS)
    return edges


def _add(
    edges: list[SymbolEdge],
    source: Endpoint,
    resolution: Resolution,
    relation: Relation,
) -> None:
    """Append one edge per candidate, dropping the ones that only self-point."""
    for candidate in resolution.candidates:
        edge = _edge(
            source,
            _definition_endpoint(candidate),
            resolution.name,
            relation,
            resolution.tier,
        )
        if edge is not None:
            edges.append(edge)


def _import_edges(facts: FileImports, scope: ImportScope | None) -> list[SymbolEdge]:
    """Turn one file's resolved imports into module-level edges."""
    if scope is None:
        return []

    edges: list[SymbolEdge] = []
    for module, line, target in scope.resolved_edges(facts.path):
        edge = _edge(
            file_endpoint(facts.path, line),
            file_endpoint(target),
            module,
            Relation.IMPORTS,
            Tier.IMPORTED,
        )
        if edge is not None:
            edges.append(edge)
    return edges


def _edge(
    source: Endpoint,
    target: Endpoint,
    name: str,
    relation: Relation,
    tier: Tier,
) -> SymbolEdge | None:
    """Build one edge, or None when it would only point at itself.

    A recursive call and a module that imports itself are both real, and both
    say nothing about how two things relate -- which is the only question this
    graph is asked.
    """
    if source.node == target.node:
        return None
    return SymbolEdge(source=source, target=target, name=name, relation=relation, tier=tier)


def file_endpoint(path: str, line: int = 0) -> Endpoint:
    """Build the endpoint standing for a whole file."""
    return Endpoint(node=path, path=path, line=line, label=path, is_symbol=False)


def _symbol_endpoint(path: str, symbol: ASTSymbol) -> Endpoint:
    """Build the endpoint standing for one defined symbol."""
    return Endpoint(
        node=symbol_stable_id(symbol),
        path=path,
        line=symbol.line_number,
        label=qualname(symbol),
        is_symbol=True,
    )


def _definition_endpoint(definition: Definition) -> Endpoint:
    """Build the endpoint standing for one candidate definition."""
    return _symbol_endpoint(definition.path, definition.symbol)


def _in_module_scope(candidate: Definition, path: str) -> bool:
    """True when ``path``'s own module namespace binds this definition.

    "Defined in this file" is not the same claim as "in this file's module
    scope", and the tier is documented as the second one: a bare name cannot
    reach a class member in any language this package parses, so a method must
    not shadow -- let alone suppress -- the candidate a declared import
    supplies. A class member is still reachable at a weaker tier, which is
    where a candidate matched on spelling alone belongs.
    """
    return candidate.path == path and not candidate.symbol.parent_class


def _ordered(candidates: Sequence[Definition]) -> tuple[Definition, ...]:
    """Order candidate definitions so a tier's candidate list is stable."""
    return tuple(
        sorted(
            candidates,
            key=lambda entry: (entry.path, entry.symbol.line_number, qualname(entry.symbol)),
        )
    )


def _group(
    edges: Iterable[SymbolEdge],
    key: Callable[[SymbolEdge], str],
) -> dict[str, tuple[SymbolEdge, ...]]:
    """Bucket edges by one endpoint's node id, preserving the sorted order."""
    collected: dict[str, list[SymbolEdge]] = {}
    for edge in edges:
        collected.setdefault(key(edge), []).append(edge)
    return {node: tuple(bucket) for node, bucket in collected.items()}


def _undirected(edges: Sequence[SymbolEdge]) -> dict[str, tuple[Hop, ...]]:
    """Build the adjacency a path search walks, both ways along every edge."""
    collected: dict[str, list[Hop]] = {}
    for edge in edges:
        collected.setdefault(edge.source.node, []).append(Hop(edge=edge, forward=True))
        collected.setdefault(edge.target.node, []).append(Hop(edge=edge, forward=False))
    return {node: tuple(hops) for node, hops in collected.items()}


def _unwind(
    previous: Mapping[str, tuple[str, Hop]],
    source: str,
    target: str,
) -> tuple[Hop, ...]:
    """Walk the search's parent links back to the source and reverse them."""
    hops: list[Hop] = []
    node = target
    while node != source:
        parent, hop = previous[node]
        hops.append(hop)
        node = parent
    return tuple(reversed(hops))


def _components(
    nodes: Sequence[str],
    adjacency: Mapping[str, tuple[str, ...]],
) -> list[tuple[str, ...]]:
    """Return Tarjan's strongly connected components, iteratively.

    Iterative rather than recursive: a repository whose import graph is one
    long chain would otherwise turn a cycle report into a RecursionError.
    """
    index: dict[str, int] = {}
    low: dict[str, int] = {}
    on_stack: set[str] = set()
    stack: list[str] = []
    found: list[tuple[str, ...]] = []
    counter = 0

    for start in nodes:
        if start in index:
            continue
        work: list[tuple[str, int]] = [(start, 0)]
        while work:
            node, position = work.pop()
            if position == 0:
                index[node] = low[node] = counter
                counter += 1
                stack.append(node)
                on_stack.add(node)

            neighbours = adjacency.get(node, ())
            if position < len(neighbours):
                work.append((node, position + 1))
                child = neighbours[position]
                if child not in index:
                    work.append((child, 0))
                elif child in on_stack:
                    low[node] = min(low[node], index[child])
                continue

            if low[node] == index[node]:
                component: list[str] = []
                while True:
                    member = stack.pop()
                    on_stack.discard(member)
                    component.append(member)
                    if member == node:
                        break
                found.append(tuple(sorted(component)))
            if work:
                parent = work[-1][0]
                low[parent] = min(low[parent], low[node])

    return found


def _cycle_chain(
    component: Sequence[str],
    adjacency: Mapping[str, tuple[str, ...]],
) -> tuple[str, ...]:
    """Return a real walk inside ``component`` that returns to where it began.

    Depth first from the component's first file, following only edges that
    stay inside it, taking the first neighbour in sorted order every time --
    so the chain reported for one component never depends on iteration order.
    """
    members = set(component)
    start = min(members)
    path: list[str] = [start]
    seen = {start}

    while True:
        node = path[-1]
        step = next(
            (child for child in adjacency.get(node, ()) if child in members and child not in seen),
            None,
        )
        if step is not None:
            path.append(step)
            seen.add(step)
            continue
        if start in adjacency.get(node, ()):
            return tuple(path)
        if len(path) == 1:
            return ()
        path.pop()
        # The dropped node stays in `seen`: it cannot close the cycle from
        # here, and re-entering it would not change that.
