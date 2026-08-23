"""The repository map: rank files, pick symbols, pack to a token budget.

The pipeline is one pass and no state: walk, extract, collect references,
build the file graph, rank it with personalized PageRank, spread each file's
rank across the symbols inside it, then pack as many symbols as the budget
allows and render them code-shaped.

Three defaults come straight from the research the plan records, and each is
a number rather than a knob-with-no-answer:

* **Function granularity, ten files.** Function-level localization beats both
  file-level and line-level (45.6% / 42.6% / 43.6%), and the file stage is
  capped at ten because a longer list stops being a funnel.
* **A budget that scales with the repository.** ``auto`` is the candidate
  set's own size divided by six, clamped to 2k-8k tokens. Roughly 6x
  compression measurably *raises* resolve rate over full context, while 22-50x
  is worse than either -- so the objective is minimal sufficient context, not
  the highest ratio available.
* **Seeds take the whole teleport mass.** ``--focus`` is not a filter, it is
  the personalization vector: the files a caller names pull rank toward
  whatever they depend on, which is how a map answers "what else does this
  touch" rather than "what is big".

Packing is a binary search over the number of included symbols rather than a
greedy fill, so the answer is the largest prefix of the score ordering that
fits -- deterministic, and independent of the order files were walked in.
"""

from dataclasses import dataclass
from pathlib import PurePosixPath
from typing import Any

from agentless_mcp.application import render
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.application.symbol_service import rationale_nodes
from agentless_mcp.core import projectconfig, refs
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.graph import build_graph, personalized_pagerank, rank_order
from agentless_mcp.core.projectconfig import MAX_MAX_FILES, MIN_MAX_FILES
from agentless_mcp.core.symbols import ASTSymbol, qualname, symbol_stable_id
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util import bounds
from agentless_mcp.util.tokens import TokenCounter

DEFAULT_MAX_FILES = 10
GRANULARITY_FUNCTION = "function"
GRANULARITY_FILE = "file"
GRANULARITIES = (GRANULARITY_FUNCTION, GRANULARITY_FILE)

# "auto": aim at ~6x compression of the candidate set, then refuse to go
# below a map that could not say anything or above one that stops being a map.
AUTO_BUDGET_DIVISOR = 6
AUTO_BUDGET_MIN = 2_000
AUTO_BUDGET_MAX = 8_000


@dataclass(frozen=True)
class MapRequest:
    """What a caller asked the map for, before the defaults are filled in.

    ``None`` means "the caller did not say", which is a different fact from
    any particular value and the one the precedence rule needs: an explicit
    argument beats the repository's ``.agentless-mcp.json``, which beats the
    built-in default. Resolving that here rather than in each adapter is what
    keeps the CLI and the MCP server from answering the same question two
    ways.

    ``budget`` is the exception: ``None`` means auto-size, which is a real
    answer rather than an absent one, so the adapters resolve it before
    building the request.
    """

    focus: tuple[str, ...] = ()
    budget: int | None = None
    max_files: int | None = None
    granularity: str | None = None


@dataclass(frozen=True)
class MapResult:
    """A rendered-ready map plus the numbers that explain its shape."""

    files: tuple[render.MapFile, ...]
    budget: int
    included: int
    candidates: int
    seeds: tuple[str, ...]
    skipped: tuple[refs.SkippedFile, ...]
    unresolved_seeds: tuple[str, ...] = ()
    # How many files the ranking produced before --max-files cut it. Carried
    # so an empty map can say which of three things happened rather than
    # asserting the one an agent stops on: nothing parsed, the file limit
    # kept none, or the token budget fitted none. Added alongside the
    # existing fields rather than replacing one, because this is a shipped
    # JSON shape.
    ranked: int = 0

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this map."""
        return {
            "budget_tokens": self.budget,
            "symbols_included": self.included,
            "symbols_available": self.candidates,
            "files_ranked": self.ranked,
            "seeds": list(self.seeds),
            "unresolved_seeds": list(self.unresolved_seeds),
            "files": [map_file.as_dict() for map_file in self.files],
            "skipped": [{"path": entry.path, "reason": entry.reason} for entry in self.skipped],
        }


@dataclass(frozen=True)
class _Candidate:
    """One symbol competing for a place in the map."""

    score: float
    path: str
    symbol: ASTSymbol


class MapService:
    """Builds repository maps. Holds no per-repository state."""

    def __init__(self, extractor: TreeSitterExtractor, counter: TokenCounter) -> None:
        self._extractor = extractor
        self._counter = counter

    def build(self, ctx: RepoContext, request: MapRequest) -> MapResult:
        """Rank, score, pack and return the map for one repository."""
        max_files = projectconfig.resolve(
            request.max_files, ctx.config.max_files, DEFAULT_MAX_FILES
        )
        # The declared contract is projectconfig's MIN/MAX_MAX_FILES, and it
        # was enforced for a config file and on the MCP wire but not here, so
        # the CLI -- which declares --max-files as a bare type=int -- honoured
        # anything. Enforced in the service means both doors inherit it.
        bounds.within(max_files, MIN_MAX_FILES, MAX_MAX_FILES, "max_files")
        granularity = projectconfig.resolve(
            request.granularity, ctx.config.granularity, GRANULARITY_FUNCTION
        )
        scan = refs.scan_repo(ctx.root, self._extractor, source=ctx.symbols)
        index = refs.build_ref_index(scan)
        # The stoplist is a property of the repository rather than of the
        # request: it says which names collide in *this* codebase, which the
        # codebase is better placed to know than the caller.
        graph = build_graph(scan, index, stoplist=ctx.config.stoplist)

        seeding = seed_weights(request.focus, scan, index)
        seeds = seeding.weights
        rank = personalized_pagerank(graph, seeds or None)
        chosen = rank_order(rank)[: max(0, max_files)]

        # `chosen` is keyed the same way as `by_path` and `rank`: all three
        # come from this one scan, so a missing key is a desynchronisation
        # worth raising on rather than reading as an empty file.
        by_path = scan.by_path()
        if granularity == GRANULARITY_FILE:
            files = tuple(
                render.MapFile(
                    path=path,
                    rank=rank[path],
                    omitted=len(by_path[path].symbols),
                )
                for path in chosen
            )
            return MapResult(
                files=files,
                budget=0,
                included=0,
                candidates=sum(len(by_path[path].symbols) for path in chosen),
                ranked=len(rank),
                seeds=tuple(sorted(seeds)),
                skipped=scan.skipped,
                unresolved_seeds=seeding.unresolved,
            )

        candidates = _score_symbols(chosen, by_path, index, rank)
        budget = (
            request.budget if request.budget is not None else self._auto_budget(candidates, rank)
        )
        included = self._pack(candidates, rank, budget)

        return MapResult(
            files=_group(candidates[:included], candidates, rank),
            budget=budget,
            included=included,
            candidates=len(candidates),
            ranked=len(rank),
            seeds=tuple(sorted(seeds)),
            skipped=scan.skipped,
            unresolved_seeds=seeding.unresolved,
        )

    def render_text(self, result: MapResult) -> str:
        """Render a map result as code-shaped text.

        Unresolved seeds and skipped files are named at the top rather than at
        the bottom: both notes change how the ranking below them should be
        read, and a reader who stops at the first interesting filename has to
        have seen them.
        """
        body = render.render_map(result.files)
        notes: list[str] = []
        if not result.files:
            notes.append(self._why_nothing_ranked(result))
        if result.unresolved_seeds:
            listed = ", ".join(result.unresolved_seeds)
            notes.append(MESSAGES.map_unresolved_seeds.format(seeds=listed))
        warning = render.render_skipped_files(result.skipped)
        if warning:
            notes.append(warning)
        if not notes:
            return body
        return "\n".join(notes) + "\n\n" + body

    def _why_nothing_ranked(self, result: MapResult) -> str:
        """Say which of the two empty results this is.

        `render_map` is handed rows and nothing else, so it can only report
        that there are none. This layer holds the candidate count and the
        budget, which is what tells "the repository parsed into no symbols"
        apart from "the budget left room for none of them" -- and the second
        reads as the first to an agent that then stops looking.
        """
        if not result.ranked:
            return "nothing in this repository parsed into symbols"
        if not result.candidates:
            return f"--max-files kept none of the {result.ranked} ranked files"
        return (
            f"the {result.budget}-token budget left room for none of "
            f"{result.candidates} symbols; raise --budget"
        )

    def _auto_budget(self, candidates: list[_Candidate], rank: dict[str, float]) -> int:
        """Size the budget from the candidate set, clamped to the useful band."""
        full = render.render_map(_group(candidates, candidates, rank))
        estimate = self._counter.count(full) // AUTO_BUDGET_DIVISOR
        return max(AUTO_BUDGET_MIN, min(AUTO_BUDGET_MAX, estimate))

    def _pack(self, candidates: list[_Candidate], rank: dict[str, float], budget: int) -> int:
        """Return the largest number of symbols whose render fits ``budget``."""
        if not candidates:
            return 0

        low, high = 0, len(candidates)
        while low < high:
            middle = (low + high + 1) // 2
            text = render.render_map(_group(candidates[:middle], candidates, rank))
            if self._counter.count(text) <= budget:
                low = middle
            else:
                high = middle - 1
        return low


@dataclass(frozen=True)
class Seeding:
    """What a request's ``--focus`` arguments resolved to, and what they did not.

    Both halves travel together because reporting one without the other is
    the defect this type exists to close: a map that answers with an empty
    personalization vector and says nothing about it reads as an unfocused
    map somebody asked for.
    """

    weights: dict[str, float]
    unresolved: tuple[str, ...]


def seed_weights(
    focus: tuple[str, ...],
    scan: refs.RepoScan,
    index: refs.RefIndex,
) -> Seeding:
    """Turn ``--focus`` entries into personalization weights over files.

    A focus entry is a path when it names one and a symbol otherwise, and a
    symbol seeds every file that defines it. Two rules make that fair:

    * **One entry, one vote.** An entry's mass is split across the files it
      resolved to rather than added once per file, so ``--focus Validate``
      naming twenty files cannot drown out ``--focus config/config.go``. The
      seed vector then says "these entries", not "whichever entry happened to
      be spelled with a common name".
    * **Nothing is lost quietly.** An entry matching no file and no symbol is
      still not an error -- the map answers unfocused rather than empty -- but
      it comes back in ``unresolved`` and every renderer says so. A seed
      silently dropped is a caller believing the map was focused when it was
      not.
    """
    known = {facts.path for facts in scan.files}
    weights: dict[str, float] = {}
    unresolved: list[str] = []

    for entry in focus:
        cleaned = entry.strip()
        if not cleaned:
            # A blank entry is the absence of an entry, not a seed that
            # failed to resolve -- the same thing an empty ``--focus`` string
            # already means -- so reporting it would name nothing back at the
            # caller. "Nothing is lost quietly" is about entries that spell
            # something the scan could not find.
            continue

        paths = focus_paths(cleaned, known, index)
        if not paths:
            unresolved.append(cleaned)
            continue

        share = 1.0 / len(paths)
        for path in paths:
            weights[path] = weights.get(path, 0.0) + share

    return Seeding(weights=weights, unresolved=tuple(dict.fromkeys(unresolved)))


def focus_paths(entry: str, known: set[str], index: refs.RefIndex) -> list[str]:
    """Resolve one focus entry to the files it names.

    Public because "a file path, a path suffix, or a symbol name" is what
    *every* view means by a focus argument, and the diagram takes one too. Two
    views resolving the same word two ways would be a defect a reader could
    only find by comparing outputs.

    Five shapes, tried in order of how specific they are:

    1. A repository-relative path, exactly as the scan spells it.
    2. A path suffix -- ``config.go`` for ``config/config.go``.
    3. A module stem -- the same suffix rule with the extension stripped, so
       ``envelope`` matches ``application/envelope.py`` exactly as typing
       ``envelope.py`` would, whatever the extension turns out to be. The
       most natural spelling of a module wins as a path, which also means a
       stem that names a file beats a symbol coincidentally sharing the name.
    4. A qualified symbol name: ``ServerInfo.Validate``, or the qualified
       half of a whole stable id. Narrowing to the definitions that really
       carry that owner is what keeps a receiver-qualified method from
       seeding every file defining any ``Validate``.
    5. A bare function, method or type name, matched exactly against the
       extracted symbols the same way ``find_symbol`` matches -- because a
       method name is the most natural seed an issue report yields, and the
       tool description promises symbol names work.

    A path-shaped entry that matches no file stops at step 3. Falling through
    would take the text after its last dot -- a file extension -- and look
    that up as a symbol, which turns a mistyped path into a confident seed on
    an unrelated file.
    """
    normalized = PurePosixPath(entry).as_posix()
    if normalized in known:
        return [normalized]

    suffix_matches = sorted(path for path in known if path.endswith(f"/{normalized}"))
    if suffix_matches:
        return suffix_matches

    stem_matches = sorted(path for path in known if _matches_stem(path, normalized))
    if stem_matches:
        return stem_matches

    qualified = entry.rpartition("::")[2] or entry
    if "/" in qualified:
        return []

    name = qualified.rpartition(".")[2] or qualified
    defining = [
        definition for definition in index.definitions.get(name, ()) if definition.path in known
    ]
    owned = [definition for definition in defining if qualname(definition.symbol) == qualified]
    return sorted({definition.path for definition in (owned or defining)})


def _matches_stem(path: str, entry: str) -> bool:
    """Say whether ``entry`` is ``path`` with its extension stripped, or a suffix of it.

    ``envelope`` matches ``application/envelope.py``; so does
    ``application/envelope``. Only the path's final suffix comes off, and dots
    inside ``entry`` are treated as part of the name -- an entry spelling the
    file's real extension already had its chance as a literal path suffix.
    """
    bare = PurePosixPath(path).with_suffix("").as_posix()
    return bare == entry or bare.endswith(f"/{entry}")


def _score_symbols(
    paths: list[str],
    by_path: dict[str, refs.FileFacts],
    index: refs.RefIndex,
    rank: dict[str, float],
) -> list[_Candidate]:
    """Spread each file's rank across its symbols by inbound reference weight.

    A file's rank says how much the repository points at the file; the inbound
    reference count says which symbol inside it the repository is pointing at.
    Multiplying keeps both: a hot symbol in a cold file still loses to a warm
    symbol in a hot one, which is the ordering a funnel wants.

    ``paths``, ``by_path`` and ``rank`` are keyed off the same scan, so a
    missing key is a desynchronisation and raises here rather than dropping
    the file from the map or ranking it alongside the genuinely cold ones.
    """
    candidates: list[_Candidate] = []
    for path in paths:
        facts = by_path[path]
        file_rank = rank[path]
        for symbol in facts.symbols:
            inbound = sum(1 for ref in index.sites.get(symbol.name, ()) if ref.path != path)
            candidates.append(
                _Candidate(score=file_rank * (1.0 + inbound), path=path, symbol=symbol)
            )

    candidates.sort(key=lambda item: (-item.score, item.path, item.symbol.line_number))
    return candidates


def _group(
    included: list[_Candidate],
    candidates: list[_Candidate],
    rank: dict[str, float],
) -> tuple[render.MapFile, ...]:
    """Group the included symbols back into rank-ordered files.

    Every candidate file is listed, including the ones whose symbols all lost
    the budget: they appear with an empty body and their omitted count. A file
    that vanishes entirely because it placed no symbols is the
    bounded-view-mistaken-for-complete failure -- the reader would have no way
    to know it was ever a candidate. The header costs a line, and the packing
    search pays for it, because it renders through this same function.
    """
    per_file: dict[str, list[_Candidate]] = {}
    for candidate in included:
        per_file.setdefault(candidate.path, []).append(candidate)

    totals: dict[str, int] = {}
    for candidate in candidates:
        totals[candidate.path] = totals.get(candidate.path, 0) + 1

    order = sorted(totals, key=lambda path: (-rank[path], path))

    files: list[render.MapFile] = []
    for path in order:
        chosen = sorted(per_file.get(path, []), key=lambda item: item.symbol.line_number)
        entries = tuple(
            render.MapEntry(
                line=candidate.symbol.line_number,
                signature=candidate.symbol.signature,
                stable_id=symbol_stable_id(candidate.symbol),
                depth=1 if candidate.symbol.parent_class else 0,
                rationales=rationale_nodes(candidate.symbol),
            )
            for candidate in chosen
        )
        files.append(
            render.MapFile(
                path=path,
                rank=rank[path],
                entries=entries,
                omitted=totals.get(path, 0) - len(entries),
            )
        )
    return tuple(files)
