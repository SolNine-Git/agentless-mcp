"""Type-2 clone blocks per file and clone groups across files, for the tier-1 languages.

A block is a function, loop, branch, or try node with at least :data:`MIN_SLOC`
source lines. Its key is :func:`agentless_mcp.core.normalize.clone_key`, so two
blocks that differ only in names, literals, comments, and Python docstrings
share a key. Python follows scb-check v0.1.3's ``detect_clones``. This module
opens no file.
"""

import bisect
import tokenize
from collections import defaultdict
from collections.abc import Callable, Iterable, Mapping, Sequence
from dataclasses import dataclass
from types import MappingProxyType
from typing import Final

from tree_sitter import Node

from agentless_mcp.core import complexity, grammars
from agentless_mcp.core.extractor import error_nodes
from agentless_mcp.core.normalize import clone_key
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util.errors import UnmeasurableSource

MIN_SLOC: Final = 3
MIN_INSTANCES: Final = 2


def _never(_node: Node) -> bool:
    return False


@dataclass(frozen=True)
class CandidateRules:
    """One language's clone block node types, and the blocks it skips while it walks into them."""

    functions: frozenset[str]
    blocks: frozenset[str]
    skip: Callable[[Node], bool] = _never


@dataclass(frozen=True)
class Block:
    """One clone block in one text, with its Type-2 key and its exact key."""

    path: str
    start_line: int
    end_line: int
    key: str
    exact: str
    function: bool


@dataclass(frozen=True)
class FileBlocks:
    """One file's clone blocks in source order, and its sorted SLOC lines."""

    path: str
    blocks: tuple[Block, ...]
    sloc_lines: tuple[int, ...]


@dataclass(frozen=True)
class CloneGroup:
    """Two or more blocks that share one Type-2 key, sorted by path and line."""

    key: str
    instances: tuple[Block, ...]


def file_blocks(source: str, path: str, language: str) -> FileBlocks:
    """Return the clone blocks of ``source``, or raise UnmeasurableSource."""
    rules = CANDIDATES.get(language)
    if rules is None:
        message = MESSAGES.clone_language_unsupported.format(
            path=path, language=language, languages=", ".join(CANDIDATES)
        )
        raise UnmeasurableSource(message)

    root = grammars.get_parser(language).parse(source.encode("utf-8")).root_node
    if root.has_error:
        line = next(error_nodes(root)).start_point[0] + 1
        raise UnmeasurableSource(MESSAGES.clone_does_not_parse.format(path=path, line=line))
    try:
        lines = tuple(sorted(complexity.RULES[language].sloc_lines(source, root)))
    except (tokenize.TokenError, SyntaxError) as exc:
        reason = exc.args[0] if exc.args else ""
        message = MESSAGES.clone_tokenizer_rejects.format(
            path=path, detail=f"{type(exc).__name__}: {reason}"
        )
        raise UnmeasurableSource(message) from exc
    return FileBlocks(path=path, blocks=_blocks(root, path, language, lines), sloc_lines=lines)


def clone_groups(files: Iterable[FileBlocks]) -> tuple[CloneGroup, ...]:
    """Return every Type-2 key that two or more blocks of ``files`` share, as groups."""
    grouped: dict[str, list[Block]] = defaultdict(list)
    for indexed in files:
        for block in indexed.blocks:
            grouped[block.key].append(block)
    groups = [
        CloneGroup(key=key, instances=tuple(sorted(blocks, key=place)))
        for key, blocks in grouped.items()
        if len(blocks) >= MIN_INSTANCES
    ]
    return tuple(sorted(groups, key=lambda group: place(group.instances[0])))


def clone_sloc_lines(
    files: Sequence[FileBlocks], groups: Iterable[CloneGroup]
) -> Mapping[str, frozenset[int]]:
    """Return, per path, the SLOC lines inside any instance of ``groups``."""
    sloc = {indexed.path: indexed.sloc_lines for indexed in files}
    found: dict[str, set[int]] = defaultdict(set)
    for group in groups:
        for block in group.instances:
            found[block.path].update(lines_within(sloc[block.path], block))
    return MappingProxyType({path: frozenset(lines) for path, lines in sorted(found.items())})


def place(block: Block) -> tuple[str, int, int]:
    """Return the order blocks are listed in: path, then start line, then end line."""
    return (block.path, block.start_line, block.end_line)


def lines_within(lines: tuple[int, ...], block: Block) -> tuple[int, ...]:
    """Return the sorted ``lines`` that fall inside ``block``'s line span."""
    return lines[
        bisect.bisect_left(lines, block.start_line) : bisect.bisect_right(lines, block.end_line)
    ]


def _blocks(root: Node, path: str, language: str, lines: tuple[int, ...]) -> tuple[Block, ...]:
    rules = CANDIDATES[language]
    found: list[Block] = []
    stack = [root]
    while stack:
        node = stack.pop()
        stack.extend(node.children)
        function = node.type in rules.functions
        if not (function or node.type in rules.blocks) or rules.skip(node):
            continue
        start, end = node.start_point[0] + 1, node.end_point[0] + 1
        if bisect.bisect_right(lines, end) - bisect.bisect_left(lines, start) < MIN_SLOC:
            continue
        key = clone_key(node, language)
        found.append(Block(path, start, end, key.type2, key.exact, function))
    return tuple(sorted(found, key=lambda block: (block.start_line, -block.end_line)))


_TYPE_CHECKING = frozenset({b"TYPE_CHECKING", b"typing.TYPE_CHECKING"})


def _is_type_checking_block(node: Node) -> bool:
    # scb-check skips the block itself and still walks into it (clones.py:128).
    condition = node.child_by_field_name("condition")
    return condition is not None and condition.text in _TYPE_CHECKING


_ECMASCRIPT = CandidateRules(
    functions=frozenset(
        {
            "function_declaration",
            "generator_function_declaration",
            "method_definition",
            "arrow_function",
            "function_expression",
            "generator_function",
        }
    ),
    blocks=frozenset(
        {
            "if_statement",
            "switch_statement",
            "for_statement",
            "for_in_statement",
            "while_statement",
            "do_statement",
            "try_statement",
        }
    ),
)
_C_BLOCKS = frozenset(
    {"if_statement", "switch_statement", "for_statement", "while_statement", "do_statement"}
)

CANDIDATES: Mapping[str, CandidateRules] = MappingProxyType(
    {
        "bash": CandidateRules(
            functions=frozenset({"function_definition"}),
            blocks=frozenset(
                {
                    "if_statement",
                    "case_statement",
                    "for_statement",
                    "c_style_for_statement",
                    "while_statement",
                }
            ),
        ),
        "c": CandidateRules(functions=frozenset({"function_definition"}), blocks=_C_BLOCKS),
        "cpp": CandidateRules(
            functions=frozenset({"function_definition"}),
            blocks=_C_BLOCKS | {"for_range_loop", "try_statement"},
        ),
        "go": CandidateRules(
            functions=frozenset({"function_declaration", "method_declaration"}),
            blocks=frozenset(
                {
                    "if_statement",
                    "expression_switch_statement",
                    "type_switch_statement",
                    "select_statement",
                    "for_statement",
                }
            ),
        ),
        "java": CandidateRules(
            functions=frozenset(
                {
                    "method_declaration",
                    "constructor_declaration",
                    "compact_constructor_declaration",
                }
            ),
            blocks=frozenset(
                {
                    "if_statement",
                    "switch_expression",
                    "for_statement",
                    "enhanced_for_statement",
                    "while_statement",
                    "do_statement",
                    "try_statement",
                    "try_with_resources_statement",
                }
            ),
        ),
        "javascript": _ECMASCRIPT,
        "lua": CandidateRules(
            functions=frozenset({"function_declaration", "function_definition"}),
            blocks=frozenset(
                {"if_statement", "for_statement", "while_statement", "repeat_statement"}
            ),
        ),
        "python": CandidateRules(
            functions=frozenset({"function_definition"}),
            blocks=frozenset(
                {
                    "if_statement",
                    "for_statement",
                    "while_statement",
                    "with_statement",
                    "try_statement",
                    "match_statement",
                }
            ),
            skip=_is_type_checking_block,
        ),
        "ruby": CandidateRules(
            functions=frozenset({"method", "singleton_method"}),
            blocks=frozenset(
                {"if", "unless", "case", "case_match", "while", "until", "for", "begin"}
            ),
        ),
        "rust": CandidateRules(
            functions=frozenset({"function_item"}),
            blocks=frozenset(
                {
                    "if_expression",
                    "match_expression",
                    "for_expression",
                    "while_expression",
                    "loop_expression",
                }
            ),
        ),
        "tsx": _ECMASCRIPT,
        "typescript": _ECMASCRIPT,
    }
)
