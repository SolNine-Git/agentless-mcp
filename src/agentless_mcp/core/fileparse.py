"""One parse of one file gives its facts, its complexity units, and its clone blocks.

The extractor must never import the metric modules, so this module owns the parse
and hands the same tree to each of them. The tag cache stores what it returns. This
module opens no file.
"""

import sys
from collections.abc import Callable
from dataclasses import dataclass
from functools import cache, partial
from typing import Final, TypeVar

from tree_sitter import Node

from agentless_mcp.core import clones, complexity, grammars
from agentless_mcp.core.clones import FileBlocks
from agentless_mcp.core.complexity import SlocLines, Unit
from agentless_mcp.core.extractor import ParsedFacts, TreeSitterExtractor
from agentless_mcp.util.deadline import recaching
from agentless_mcp.util.errors import UnmeasurableSource

# Bump on any change to what `complexity.measure` or `clones.file_blocks` returns for one
# text: their tables, SLOC rules, normalizer, token count, or messages. test_fileparse pins it.
METRICS_ALGORITHM: Final = 2

_Metric = TypeVar("_Metric")

# One text's complexity units and clone blocks, as the two text forms return them.
UnitsAndBlocks = tuple[tuple[Unit, ...], FileBlocks]


@dataclass(frozen=True)
class Unmeasured:
    """Why a metric could not be taken from one text: the UnmeasurableSource message, verbatim."""

    reason: str


@dataclass(frozen=True)
class FileMetrics:
    """One text's complexity units and clone blocks, each a value or the reason it has none."""

    stamp: str
    language: str
    units: tuple[Unit, ...] | Unmeasured
    blocks: FileBlocks | Unmeasured


@dataclass(frozen=True)
class ParsedFile:
    """One file's facts and, for a language that has them, its metrics, from one parse."""

    facts: ParsedFacts
    metrics: FileMetrics | None


def metrics_stamp(language: str) -> str | None:
    """Return the stamp a cached metric row of ``language`` carries, or ``None`` for no metrics."""
    if language not in complexity.RULES:
        return None
    # The SLOC rules read the running interpreter's tokenizer and Unicode tables.
    major, minor = sys.version_info[:2]
    return f"{METRICS_ALGORITHM}/py{major}.{minor}"


def parse_file(extractor: TreeSitterExtractor, text: str, language: str, path: str) -> ParsedFile:
    """Parse ``text`` once and return its facts, units, and clone blocks."""
    stamp = metrics_stamp(language)
    if stamp is None:
        return parse_facts(extractor, text, language, path)
    with recaching():
        data = text.encode("utf-8")
        tree = extractor.parse(data, language)
        root = tree.root_node
        lines = _sloc_once(root, text, language)
        metrics = FileMetrics(
            stamp=stamp,
            language=language,
            units=_measured(lambda: complexity.measure_tree(root, path, language, lines)),
            blocks=_measured(lambda: clones.file_blocks_tree(root, path, language, lines)),
        )
        return ParsedFile(extractor.facts_from_tree(tree, data, language, path), metrics)


def parse_facts(extractor: TreeSitterExtractor, text: str, language: str, path: str) -> ParsedFile:
    """Parse ``text`` for its facts alone, with no metrics."""
    return ParsedFile(extractor.extract_facts(text, language, path), None)


def parse_metrics(text: str, language: str, path: str) -> UnitsAndBlocks:
    """Return or raise what `complexity.measure` then `clones.file_blocks` would, from one parse."""
    if metrics_stamp(language) is None:
        # The text forms refuse the language before they parse, so their message wins.
        return complexity.measure(text, path, language), clones.file_blocks(text, path, language)
    root = grammars.get_parser(language).parse(text.encode("utf-8")).root_node
    lines = _sloc_once(root, text, language)
    units = complexity.measure_tree(root, path, language, lines)
    return units, clones.file_blocks_tree(root, path, language, lines)


def _sloc_once(root: Node, text: str, language: str) -> SlocLines:
    # Both metrics read one SLOC pass. A tokenizer rejection is not cached, so each
    # metric raises it again and keeps its own message.
    return cache(partial(complexity.sloc_line_numbers, root, text, language))


def _measured(measure: Callable[[], _Metric]) -> _Metric | Unmeasured:
    try:
        return measure()
    except UnmeasurableSource as exc:
        return Unmeasured(str(exc))
