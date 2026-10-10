"""Boundary globs and the syntactic contract rules that ``contract_drift`` runs over text."""

import re
from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass
from functools import cache
from types import MappingProxyType

from tree_sitter import Language, Node, Query, QueryCursor, QueryError

from agentless_mcp.core import complexity, grammars
from agentless_mcp.core.complexity import Unit
from agentless_mcp.core.extractor import COMMENT_NODE_TYPES
from agentless_mcp.prompts import CONTRACT_RULES, ContractRule, PromptDataError

_HIT_CAPTURE = "hit"
# Unknown predicates compile and are then ignored, so a typo would widen a rule.
_PREDICATES = frozenset({"eq?", "not-eq?", "any-of?", "not-any-of?", "match?", "not-match?"})

_PREDICATE_NAME = re.compile(r"\(#([A-Za-z-]+[?!])")
_GLOBSTAR = "**"


@dataclass(frozen=True)
class CompiledRule:
    """One contract rule and its query, compiled against the rule's grammar."""

    rule: ContractRule
    query: Query


@dataclass(frozen=True)
class Hit:
    """One node a rule matched, the innermost unit that holds it, and its normalized text."""

    rule: ContractRule
    unit: Unit | None
    line: int
    capture: str


def in_boundary(path: str, globs: Sequence[str]) -> bool:
    """Return whether the repo-relative POSIX ``path`` matches any of ``globs``."""
    segments = path.split("/")
    return any(_glob_matches(glob.split("/"), segments) for glob in globs)


def rules_for(language: str) -> tuple[ContractRule, ...]:
    """Return the contract rules for ``language``, empty for a language that has none."""
    return _BY_LANGUAGE.get(language, ())


@cache
def compiled(language: str) -> tuple[CompiledRule, ...]:
    """Compile every rule for ``language``, or raise PromptDataError naming the rule that fails."""
    rules = rules_for(language)
    if not rules:
        return ()
    grammar = grammars.get_language(language)
    return tuple(_compile(rule, grammar) for rule in rules)


def hits(source: str, language: str, units: Sequence[Unit]) -> tuple[Hit, ...]:
    """Return every rule hit in ``source``, which must parse, with its unit from ``units``."""
    rules = compiled(language)
    if not rules:
        return ()
    encoded = source.encode("utf-8")
    root = grammars.get_parser(language).parse(encoded).root_node
    found: list[tuple[int, int, Hit]] = []
    for order, entry in enumerate(rules):
        for node in _hit_nodes(entry.query, root):
            hit = Hit(
                rule=entry.rule,
                unit=_enclosing(units, node.start_byte),
                line=node.start_point[0] + 1,
                capture=_capture(node, encoded),
            )
            found.append((node.start_byte, order, hit))
    found.sort(key=lambda item: (item[0], item[1]))
    return tuple(hit for _, _, hit in found)


def _index(rules: Sequence[ContractRule]) -> Mapping[str, tuple[ContractRule, ...]]:
    grouped: dict[str, list[ContractRule]] = {}
    for rule in rules:
        if rule.language not in complexity.RULES:
            message = (
                f"contract rule {rule.id!r} names language {rule.language!r}, which has no "
                f"pair-check rules (languages: {', '.join(complexity.RULES)})"
            )
            raise PromptDataError(message)
        grouped.setdefault(rule.language, []).append(rule)
    return MappingProxyType({language: tuple(group) for language, group in grouped.items()})


_BY_LANGUAGE = _index(CONTRACT_RULES)


def _compile(rule: ContractRule, grammar: Language) -> CompiledRule:
    where = f"contract rule {rule.id!r} for {rule.language}"
    try:
        query = Query(grammar, rule.query)
    except QueryError as exc:
        message = f"{where} does not compile: {exc}"
        raise PromptDataError(message) from exc

    names = [query.capture_name(index) for index in range(query.capture_count)]
    if _HIT_CAPTURE not in names or any(
        name != _HIT_CAPTURE and not name.startswith("_") for name in names
    ):
        message = f"{where} must capture @{_HIT_CAPTURE}, and every other capture must start with _"
        raise PromptDataError(message)

    unknown = sorted(set(_PREDICATE_NAME.findall(rule.query)) - _PREDICATES)
    if unknown:
        message = f"{where} uses predicates {unknown}, outside {sorted(_PREDICATES)}"
        raise PromptDataError(message)
    return CompiledRule(rule=rule, query=query)


def _hit_nodes(query: Query, root: Node) -> list[Node]:
    spans: dict[tuple[int, int], Node] = {}
    for _pattern, captures in QueryCursor(query).matches(root):
        for node in captures.get(_HIT_CAPTURE, ()):
            spans.setdefault((node.start_byte, node.end_byte), node)
    return [spans[span] for span in sorted(spans)]


def _enclosing(units: Sequence[Unit], offset: int) -> Unit | None:
    holding = [unit for unit in units if unit.start_byte <= offset < unit.end_byte]
    return max(holding, key=lambda unit: unit.start_byte, default=None)


def _capture(node: Node, encoded: bytes) -> str:
    # Comments are cut out so that editing one does not turn an old hit into a new one.
    kept: list[bytes] = []
    cursor = node.start_byte
    for comment in _comments(node):
        kept.append(encoded[cursor : comment.start_byte])
        cursor = comment.end_byte
    kept.append(encoded[cursor : node.end_byte])
    return " ".join(b"".join(kept).decode("utf-8").split())


def _comments(node: Node) -> Iterator[Node]:
    stack = [node]
    while stack:
        current = stack.pop()
        if current.type in COMMENT_NODE_TYPES:
            yield current
            continue
        stack.extend(reversed(current.children))


def _glob_matches(elements: Sequence[str], segments: Sequence[str]) -> bool:
    # reached[j]: the elements so far consume exactly segments[:j]. One pass per element,
    # so the cost grows with glob length times path length and nothing backtracks.
    reached = [True] + [False] * len(segments)
    last = len(elements) - 1
    for index, element in enumerate(elements):
        if element == _GLOBSTAR:
            reached = _globstar(reached, segments, final=index == last)
        else:
            reached = [False] + [
                was and _segment_matches(element, segment)
                for was, segment in zip(reached[:-1], segments, strict=True)
            ]
    return reached[-1]


def _globstar(reached: Sequence[bool], segments: Sequence[str], *, final: bool) -> list[bool]:
    # ** consumes whole non-empty segments: one or more at the end of a glob, any number inside.
    spread = [False]
    for j, segment in enumerate(segments, start=1):
        spread.append(bool(segment) and (reached[j - 1] or spread[j - 1]))
    if final:
        return spread
    return [was or more for was, more in zip(reached, spread, strict=True)]


def _segment_matches(pattern: str, text: str) -> bool:
    # A star stays inside one segment, so each piece between stars takes its leftmost
    # place after the previous one, and no choice is ever undone.
    pieces = pattern.split("*")
    if len(pieces) == 1:
        return pattern == text
    head, *middle, tail = pieces
    end = len(text) - len(tail)
    if end < len(head) or not text.startswith(head) or not text.endswith(tail):
        return False
    position = len(head)
    for piece in middle:
        found = text.find(piece, position, end)
        if found < 0:
            return False
        position = found + len(piece)
    return True
