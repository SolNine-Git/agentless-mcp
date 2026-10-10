"""Boundary globs and the syntactic contract rules that ``contract_drift`` runs over text."""

import re
from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass
from functools import cache, lru_cache
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
_SOME_SEGMENTS_THEN_SLASH = "(?:[^/]+/)*"
_ONE_OR_MORE_SEGMENTS = "[^/]+(?:/[^/]+)*"


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
    return any(_glob_pattern(glob).fullmatch(path) for glob in globs)


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


@lru_cache(maxsize=256)
def _glob_pattern(glob: str) -> re.Pattern[str]:
    segments = glob.split("/")
    last = len(segments) - 1
    parts: list[str] = []
    for index, segment in enumerate(segments):
        if segment == "**":
            parts.append(_ONE_OR_MORE_SEGMENTS if index == last else _SOME_SEGMENTS_THEN_SLASH)
        else:
            literal = "[^/]*".join(re.escape(piece) for piece in segment.split("*"))
            parts.append(literal if index == last else f"{literal}/")
    return re.compile("".join(parts))
