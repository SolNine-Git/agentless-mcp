"""Write the syntax test fixtures: sources with the parse, rule results and token streams of Python.

Development-time only. Run it from the repository root, in the project environment:

    uv run --frozen python scripts/syntax_fixtures.py

With --check, the script compares its output with the committed fixtures and writes nothing.
"""

import hashlib
import json
import random
import sys
from collections.abc import Callable
from pathlib import Path

from fixture_files import Fixture, write_or_check_files
from fixture_files import render as render_sections
from syntax_tables import present
from tree_sitter import Node, Query, QueryCursor, Tree, TreeCursor

from agentless_mcp.core import clones, complexity, grammars, normalize
from agentless_mcp.core.extractor import (
    COMMENT_NODE_TYPES,
    INDENT_BLOCK_NODE_TYPES,
    TreeSitterExtractor,
    rust_impl_owner,
)

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates" / "syntax" / "fixtures"
SAMPLES = ROOT / "tests" / "characterization" / "fixtures"
MUTATIONS_PER_SOURCE = 8
# Letters, digits and spaces outside ASCII: the scanners classify them through the ctype shim.
LETTERS = (0xE9, 0xDF, 0x416, 0x3BB, 0x1C5, 0x3A3, 0xFB01, 0x130, 0x663, 0x216B, 0x1D518)
SPACES = (0xA0, 0x2003, 0x3000)
NON_ASCII = tuple(chr(code) for code in (*LETTERS, *SPACES))
PUNCTUATION = (")", "}", "]", '"', "'", "`", "/*", "#", "\\", "$(", "<<", ":", ";", "end", "\n")

QUERIES = {
    "python": (
        "(block (string) @statement_string) (module (string) @statement_string)",
        '(call function: (identifier) @_f (#any-of? @_f "len" "print" "range" "sum")) @hit',
        '(return_statement (identifier) @_x (#eq? @_x "total")) @hit (identifier) @name',
    ),
    "javascript": (
        "(call_expression function: (member_expression property: (property_identifier) @p)) @hit",
    ),
    "rust": ("(function_item name: (identifier) @name) @function",),
}

Input = tuple[str, str, str]
Result = int | bool


def main() -> int:
    """Write the fixtures, or compare them with --check, and return the exit status."""
    return write_or_check_files(__doc__.splitlines()[0], FIXTURES, contents)


def contents() -> dict[Path, bytes]:
    sources = inputs()
    measured = [source for source in sources if source[1] in complexity.RULES]
    queried = [source for source in sources if source[1] in QUERIES]
    return {
        FIXTURES / "parse.json": render([case(*source) for source in sources]),
        FIXTURES / "rules.json": render([rule_case(*source) for source in measured]),
        FIXTURES / "queries.json": render([query_case(*source) for source in queried]),
        FIXTURES / "stream.json": render_sections(streams(sources)),
    }


def inputs() -> list[Input]:
    found = []
    for path in samples():
        language = TreeSitterExtractor.SUPPORTED_EXTENSIONS[path.suffix]
        source = path.read_text(encoding="utf-8")
        name = path.relative_to(SAMPLES).as_posix()
        found.append((name, language, source))
        found.extend(
            (f"{name}#{index}", language, mutated(source, random.Random(f"{name}#{index}")))
            for index in range(MUTATIONS_PER_SOURCE)
        )
    return found


def samples() -> list[Path]:
    directories = [*sorted(SAMPLES.glob("complexity_*")), SAMPLES / "tier1", SAMPLES / "tier2"]
    return [
        path
        for directory in directories
        for path in sorted(directory.iterdir())
        if path.suffix in TreeSitterExtractor.SUPPORTED_EXTENSIONS
    ]


def mutated(source: str, rng: random.Random) -> str:
    for _ in range(rng.randint(1, 3)):
        at = rng.randrange(len(source) + 1)
        kind = rng.randrange(3)
        if kind == 0:
            source = source[:at] + source[at + rng.randint(1, 20) :]
        elif kind == 1:
            text = "".join(rng.choice(NON_ASCII) for _ in range(rng.randint(1, 3)))
            source = source[:at] + text + source[at:]
        else:
            source = source[:at] + rng.choice(PUNCTUATION) + source[at:]
    return source


def parsed(language: str, source: str) -> Tree:
    return grammars.get_parser(language).parse(source.encode("utf-8"))


def preorder(tree: Tree) -> list[tuple[Node, str | None]]:
    nodes: list[tuple[Node, str | None]] = []
    cursor = tree.walk()
    while True:
        node = cursor.node
        if node is None:
            message = "the cursor lost its node"
            raise ValueError(message)
        nodes.append((node, cursor.field_name))
        if not next_node(cursor):
            return nodes


def next_node(cursor: TreeCursor) -> bool:
    if cursor.goto_first_child():
        return True
    while not cursor.goto_next_sibling():
        if not cursor.goto_parent():
            return False
    return True


def case(name: str, language: str, source: str) -> dict[str, object]:
    nodes = preorder(parsed(language, source))
    rows = [row(node, field) for node, field in nodes]
    errors = (index for index, (node, _) in enumerate(nodes) if node.is_error or node.is_missing)
    return {
        "name": name,
        "language": language,
        "source": source,
        "nodes": len(rows),
        "first_error": next(errors, -1),
        "digest": hashlib.sha256("\n".join(rows).encode("utf-8")).hexdigest(),
    }


def row(node: Node, field: str | None) -> str:
    flags = (node.is_named, node.is_missing, node.is_error, node.has_error)
    bits = "".join("1" if flag else "0" for flag in flags)
    return f"{node.type}|{node.start_byte}|{node.end_byte}|{field or ''}|{bits}"


def rule_case(name: str, language: str, source: str) -> dict[str, object]:
    nodes = [node for node, _ in preorder(parsed(language, source))]
    index = {node.id: position for position, node in enumerate(nodes)}

    def found(node: Node | None) -> int:
        return -1 if node is None else index[node.id]

    results = [
        [position, rule, value]
        for position, node in enumerate(nodes)
        for rule, value in rule_results(language, node, found)
    ]
    return {"name": name, "language": language, "results": results}


def rule_results(
    language: str, node: Node, found: Callable[[Node | None], int]
) -> list[tuple[str, Result]]:
    units = complexity.RULES[language].units
    conditional = complexity.RULES[language].decisions.conditional
    candidates = clones.CANDIDATES[language]
    kind = node.type
    results: list[tuple[str, Result]] = []
    if kind in units.units | units.bound | units.scopes:
        results.append(("name_of", found(units.name_of(node))))
    if kind in units.bound and units.binding_of is not None:
        results.append(("binding_of", found(units.binding_of(node))))
    if kind in units.units and units.owner_of is not None:
        results.append(("owner_of", found(units.owner_of(node))))
    if kind in conditional:
        results.append(("condition", conditional[kind](node)))
    if kind in candidates.functions | candidates.blocks:
        results.append(("skips", candidates.skip(node)))
    if language == "rust" and kind == "impl_item":
        results.append(("rust_impl_owner", found(rust_impl_owner(node))))
    return results


def query_case(name: str, language: str, source: str) -> dict[str, object]:
    tree = parsed(language, source)
    index = {node.id: position for position, (node, _) in enumerate(preorder(tree))}
    runs = [
        {"query": text, **query_run(Query(grammars.get_language(language), text), tree, index)}
        for text in QUERIES[language]
    ]
    return {"name": name, "language": language, "runs": runs}


def query_run(query: Query, tree: Tree, index: dict[int, int]) -> dict[str, object]:
    names = [query.capture_name(position) for position in range(query.capture_count)]
    matches = [
        [
            pattern,
            sorted(
                [names.index(name), index[node.id]]
                for name, nodes in found.items()
                for node in nodes
            ),
        ]
        for pattern, found in QueryCursor(query).matches(tree.root_node)
    ]
    # py-tree-sitter's captures() order differs from run to run, so the fixture holds sorted sets.
    found = QueryCursor(query).captures(tree.root_node)
    captures = {name: sorted(index[node.id] for node in found[name]) for name in sorted(found)}
    return {"matches": matches, "captures": captures}


def streams(sources: list[Input]) -> Fixture:
    # Each language's comment kinds that its grammar knows, and its indent-block kinds.
    tables: list[object] = [
        [
            language,
            present(language, COMMENT_NODE_TYPES),
            sorted(INDENT_BLOCK_NODE_TYPES.get(language, ())),
        ]
        for language in grammars.ALL_LANGUAGES
    ]
    return {"tables": tables, "cases": [stream_case(*source) for source in sources]}


def stream_case(name: str, language: str, source: str) -> dict[str, object]:
    tree = parsed(language, source)
    nodes = [node for node, _ in preorder(tree)]
    omit = len(nodes) // 2
    without = normalize.node_stream(tree.root_node, language, omit=nodes[omit])
    return {
        "name": name,
        "language": language,
        "stream": normalize.node_stream(tree.root_node, language),
        "omit": omit,
        "without": hashlib.sha256(without.encode("utf-8")).hexdigest(),
    }


def render(cases: list[dict[str, object]]) -> bytes:
    # One case per line, so a changed case shows as one changed line in a diff.
    lines = ",\n".join(f" {json.dumps(case)}" for case in cases)
    return ("[\n" + lines + "\n]\n").encode("ascii")


if __name__ == "__main__":
    sys.exit(main())
