"""Write the facts test fixtures: the facts, skeletons and views that the Python code computes.

Development-time only. Run it from the repository root, in the project environment:

    uv run --frozen python scripts/facts_fixtures.py

The inputs are every supported file under tests/characterization/fixtures/, two seeded mutations
of each, CRLF, BOM and tab variants of a few of them, and the extension sources at the end of this
file. With --check, the script compares its output with the committed fixtures and writes nothing.
"""

import random
import sys
from collections.abc import Callable, Iterable, Sequence
from dataclasses import dataclass
from pathlib import Path
from textwrap import dedent

from fixture_files import Fixture, refuse_real_caches, render, write_or_check_files
from syntax_fixtures import mutated

from agentless_mcp.core import extractor, grammars, locs, skeleton, slices
from agentless_mcp.core.extractor import TreeSitterExtractor, error_nodes
from agentless_mcp.core.symbols import (
    ASTSymbol,
    base_name,
    normalise_signature,
    parse_stable_id,
    rationale_stable_id,
    split_ordinal,
    symbol_stable_id,
)
from agentless_mcp.util.errors import OperationFailed

ROOT = Path(__file__).resolve().parents[1]
CORPUS = ROOT / "tests" / "characterization" / "fixtures"
FIXTURES = ROOT / "crates" / "facts" / "fixtures"
MUTATIONS = 2
INT_MAX_STR_DIGITS = 4300
EXTRACTOR = TreeSitterExtractor()


@dataclass(frozen=True)
class Source:
    """One input file: its name, which is also the path its ids carry, its language, and text."""

    name: str
    language: str
    text: str


def main() -> int:
    """Write the fixtures, or compare them with --check, and return the exit status."""
    refuse_real_caches()
    if sys.get_int_max_str_digits() != INT_MAX_STR_DIGITS:
        sys.exit(f"refused: the fixtures need int() to read {INT_MAX_STR_DIGITS} digits at most")
    return write_or_check_files(__doc__.splitlines()[0], FIXTURES, contents)


def contents() -> dict[Path, bytes]:
    sources = inputs()
    files = {FIXTURES / "views.json": render(views(sources))}
    for language in grammars.ALL_LANGUAGES:
        chosen = [source for source in sources if source.language == language]
        files[FIXTURES / f"extract_{language}.json"] = render(
            {"cases": [extract_case(source) for source in chosen]}
        )
        files[FIXTURES / f"skeleton_{language}.json"] = render(
            {"cases": [skeleton_case(source) for source in chosen]}
        )
    return files


def inputs() -> list[Source]:
    corpus = [
        Source(path.relative_to(CORPUS).as_posix(), language_of(path.name), read(path))
        for path in sorted(CORPUS.rglob("*"))
        if path.is_file()
        and "__pycache__" not in path.parts
        and path.suffix in TreeSitterExtractor.SUPPORTED_EXTENSIONS
    ]
    extension = [Source(name, language_of(name), text) for name, text in EXTENSION]
    variants = [
        Source(f"variant/{kind}/{source.name}", source.language, change(source.text))
        for kind, names, change in VARIANTS
        for source in corpus
        if source.name in names
    ]
    originals = corpus + extension
    mutations = [
        Source(
            f"{source.name}#{index}", source.language, mutated(source.text, seeded(source, index))
        )
        for source in originals
        for index in range(MUTATIONS)
    ]
    return originals + variants + mutations


def language_of(name: str) -> str:
    return TreeSitterExtractor.SUPPORTED_EXTENSIONS[Path(name).suffix]


def read(path: Path) -> str:
    return path.read_bytes().decode("utf-8")


def seeded(source: Source, index: int) -> random.Random:
    return random.Random(f"{source.name}#{index}")


def tabbed(text: str) -> str:
    """Indent each line with one tab per four leading spaces."""
    lines = []
    for line in text.split("\n"):
        body = line.lstrip(" ")
        depth = (len(line) - len(body)) // 4
        lines.append("\t" * depth + " " * ((len(line) - len(body)) % 4) + body)
    return "\n".join(lines)


def extract_case(source: Source) -> dict[str, object]:
    facts = EXTRACTOR.extract_facts(source.text, source.language, source.name)
    tree = EXTRACTOR.parse(source.text.encode("utf-8"), source.language)
    return {
        "name": source.name,
        "source": source.text,
        "symbols": [symbol_row(symbol) for symbol in facts.symbols],
        "imports": [
            {
                "module": item.module,
                "names": list(item.names),
                "local_names": list(item.local_names),
                "is_relative": item.is_relative,
                "relative_level": item.relative_level,
                "line": item.line_number,
                "binds_all": item.binds_all,
                "alias": item.alias,
            }
            for item in facts.imports
        ],
        "refs": [[ref.name, ref.line, ref.role.value, ref.qualifier] for ref in facts.refs],
        "errors": [
            [node.start_byte, node.end_byte]
            for node in error_nodes(tree.root_node)
            if node.is_error
        ],
    }


def symbol_row(symbol: ASTSymbol) -> dict[str, object]:
    return {
        "name": symbol.name,
        "kind": symbol.kind.value,
        "line": symbol.line_number,
        "end_line": symbol.end_line_number,
        "signature": symbol.signature,
        "parent": symbol.parent_class,
        "bases": list(symbol.bases),
        "is_public": symbol.is_public,
        "duplicate_index": symbol.duplicate_index,
        "id": symbol_stable_id(symbol),
        "rationales": [
            {
                "kind": rationale.kind,
                "text": rationale.text,
                "line": rationale.line_number,
                "citations": list(rationale.citations),
                "duplicate_index": rationale.duplicate_index,
                "handle": rationale_stable_id(symbol, rationale),
            }
            for rationale in symbol.rationales
        ],
    }


def skeleton_case(source: Source) -> dict[str, object]:
    def rendered(docstrings: bool, numbers: bool) -> str:
        return skeleton.skeletonize(
            source.text, source.language, docstrings=docstrings, number_lines=numbers
        )

    # The extract case of the same name holds the source.
    return {
        "name": source.name,
        "default": rendered(docstrings=False, numbers=False),
        "docstrings": rendered(docstrings=True, numbers=False),
        "numbers": rendered(docstrings=False, numbers=True),
        "docstrings_numbers": rendered(docstrings=True, numbers=True),
    }


def views(sources: Sequence[Source]) -> Fixture:
    return {
        "line_count": [[text, slices.line_count(text)] for text in LINE_COUNT],
        **source_views(sources),
        **text_views(),
    }


def source_views(sources: Sequence[Source]) -> Fixture:
    """Return the slice, location and mention sections, which read whole sources."""
    by_name = {source.name: source for source in sources}
    chosen = [by_name[name] for name in VIEW_SOURCES]
    return {
        "slice_lines": [case for source in chosen for case in slice_cases(source)],
        "resolve_locs": [case for source in chosen for case in locs_cases(source)]
        + [locs_case(Source("README", "", ""), (), LOCS, 10)],
        "unparsed_mentions": [case for source in sources for case in mention_cases(source)],
    }


def text_views() -> Fixture:
    """Return the sections of the id, ordinal, signature, name and deviation rules."""
    return {
        "stable_ids": [[text, parsed_id(text)] for text in STABLE_IDS],
        "split_ordinal": [[text, *split_ordinal(text)] for text in ORDINALS],
        "signatures": [[text, normalise_signature(text)] for text in SIGNATURES],
        "truncate": [
            [text, limit, extractor._truncate(text, limit)]
            for text in SIGNATURES
            for limit in (3, 10, 80)
        ],
        "upper_case": [
            [
                name,
                extractor._UPPER_CASE_RE.match(name) is not None,
                extractor._UPPER_CASE_RE.fullmatch(name) is not None,
            ]
            for name in UPPER_CASE
        ],
        "base_name": [[text, base_name(text)] for text in BASES],
        "deviations": deviations(),
        "depth": [[shape, depth, deep_outcome(shape, depth)] for shape, depth in DEPTHS],
    }


def deep_source(shape: str, depth: int) -> str:
    """Return a source whose definitions nest ``depth`` levels; the Rust test builds the same."""
    if shape == "defs":
        lines = "".join(" " * level + f"def f{level}():\n" for level in range(depth))
        return lines + " " * depth + "pass\n"
    if shape == "classes":
        indents = [(" " * level, level) for level in range(1, depth + 1)]
        lines = "".join(f"{indent}@d\n{indent}class C{level}:\n" for indent, level in indents)
        return "class C0:\n" + lines + " " * (depth + 1) + "pass\n"
    return "int " + "(*" * depth + "f(void)" + ")(int)" * depth + ";\n"


def deep_outcome(shape: str, depth: int) -> list[object]:
    """Return the names and kinds the reference extracts, or that it runs out of recursion."""
    language = "c" if shape == "c_declarators" else "python"
    try:
        symbols = EXTRACTOR.extract_from_source(deep_source(shape, depth), language, "deep")
    except RecursionError:
        return ["recursion_error"]
    return ["symbols", [[symbol.name, symbol.kind.value] for symbol in symbols]]


def symbols_of(source: Source) -> list[ASTSymbol]:
    return EXTRACTOR.extract_from_source(source.text, source.language, source.name)


def slice_cases(source: Source) -> list[list[object]]:
    symbols = symbols_of(source)
    total = slices.line_count(source.text)
    starts = [symbol.line_number + 1 for symbol in symbols[:6]]
    interval_sets = [
        [],
        [[1, 3]],
        *([[start, start + 1]] for start in starts),
        [[starts[-1] if starts else 2, total + 5]],
        [[total + 2, total + 9]],
        [[5, 3]],
        [[0, 2], [2, 6], [10, 12], [11, 14]],
        [[3, 4], [1, 2]],
    ]
    rows = [[symbol.kind.value, symbol.line_number, symbol.end_line_number] for symbol in symbols]
    return [
        [source.language, source.name, intervals, rows, sliced(source.text, intervals, symbols)]
        for intervals in interval_sets
    ]


def sliced(content: str, intervals: list[list[int]], symbols: list[ASTSymbol]) -> object:
    pairs = [(start, end) for start, end in intervals]
    try:
        return slices.line_wrap_content(content, pairs, symbols=symbols)
    except OperationFailed as refusal:
        return {"refusal": str(refusal)}


def locs_cases(source: Source) -> list[object]:
    symbols = symbols_of(source)
    general = LOCS if source.name == VIEW_SOURCES[0] else LOCS[:5]
    return [
        locs_case(source, symbols, locs_lines(symbols[:12]) + general, 10),
        locs_case(source, symbols, locs_lines(symbols[:3]), 0),
        locs_case(source, symbols, locs_lines(symbols[:3]), 3),
    ]


def locs_lines(symbols: Iterable[ASTSymbol]) -> list[str]:
    named: list[str] = []
    for symbol in symbols:
        named += [
            f"class: {symbol.name}",
            f"function: {symbol.name}",
            f"method: {symbol.name}",
            f"function: {symbol.parent_class}.{symbol.name}",
            f"{symbol.parent_class}.{symbol.name}",
            f"function: {symbol.parent_class}.{symbol.name}#{symbol.duplicate_index + 1}",
            f"function: {symbol.name}#9",
            f"variable: {symbol.name} trailing words",
        ]
    return named


def locs_case(
    source: Source, symbols: Sequence[ASTSymbol], lines: list[str], context: int
) -> dict[str, object]:
    total = slices.line_count(source.text)
    target = locs.LocTarget(
        path=source.name, language=source.language, symbols=tuple(symbols), total_lines=total
    )
    resolution = locs.resolve_locs(lines, target, context=context)
    return {
        "path": source.name,
        "language": source.language,
        "total": total,
        "symbols": [
            {
                "name": symbol.name,
                "kind": symbol.kind.value,
                "parent": symbol.parent_class,
                "line": symbol.line_number,
                "end_line": symbol.end_line_number,
                "duplicate_index": symbol.duplicate_index,
            }
            for symbol in symbols
        ],
        "locs": lines,
        "context": context,
        "stable_ids": list(resolution.stable_ids),
        "spans": [list(span) for span in resolution.spans],
        "intervals": [list(interval) for interval in resolution.intervals],
        "unrecognized": [[item.loc, item.reason] for item in resolution.unrecognized],
    }


def mention_cases(source: Source) -> list[list[object]]:
    """Return the mention cases of a source that does not parse, each naming its extract case."""
    tree = EXTRACTOR.parse(source.text.encode("utf-8"), source.language)
    if not any(node.is_error for node in error_nodes(tree.root_node)):
        return []
    facts = EXTRACTOR.extract_facts(source.text, source.language, source.name)
    names = list(dict.fromkeys(ref.name for ref in facts.refs))[:3]
    chosen = [*names, *(name.swapcase() for name in names[:1]), *MENTION_NAMES]
    return [
        [
            source.language,
            source.name,
            name,
            list(EXTRACTOR.unparsed_mentions(source.text, source.language, name)),
        ]
        for name in chosen
    ]


def parsed_id(text: str) -> list[str]:
    try:
        parsed = parse_stable_id(text)
    except ValueError as error:
        return ["error", str(error)]
    return ["ok", parsed.prefix, parsed.path, parsed.qualname]


def deviations() -> list[object]:
    """Return the inputs with digits outside ASCII, and the outcome of the reference code."""
    target = locs.LocTarget(path="a.py", language="python", symbols=(), total_lines=10)
    lines: list[object] = [
        ["line", loc, [list(span) for span in locs.resolve_locs([loc], target).spans]]
        for loc in DEVIATION_LINES
    ]
    ordinals: list[object] = [
        ["ordinal", text, list(split_ordinal(text))] for text in DEVIATION_ORDINALS
    ]
    return lines + ordinals


def _text(block: str) -> str:
    return dedent(block).lstrip("\n")


# Characters the cases need that an editor shows as a space or as nothing at all.
IDEOGRAPHIC_SPACE = chr(0x3000)
BYTE_ORDER_MARK = chr(0xFEFF)
INVISIBLE = "no" + chr(0x200B) + "break" + chr(0xA0) + "space" + IDEOGRAPHIC_SPACE + "line"
LINE_SEPARATOR = chr(0x2028)
# Digits outside ASCII: Arabic-Indic three, fullwidth five and fullwidth two.
ARABIC_THREE = chr(0x663)
FULLWIDTH_FIVE = chr(0xFF15)
FULLWIDTH_TWO = chr(0xFF12)

# The files whose slices and locations the views fixture holds.
VIEW_SOURCES = (
    "repo_py/pricing.py",
    "repo_py/inventory.py",
    "shapes/nested.py",
    "shapes/nested.go",
    "repo_ts/pricing.ts",
    "repo_go/pricing.go",
    "tier1/pricing.cpp",
    "tier1/pricing.rs",
    "tier2/config.yaml",
    "extension/edges.py",
    "extension/nested.cpp",
    "extension/tables.toml",
    "extension/Second.cs",
    "variant/crlf/repo_py/pricing.py",
)

LINE_COUNT = ("", "a", "a\n", "\n", "\n\n", "a\nb", "a\nb\n", "x\r\ny\r\n", "a\n\n", "é\n中\n")

LOCS = [
    "class: Missing",
    "class: A.b",
    "function: nope",
    "function: Nope.method",
    "method: ",
    "line: 1",
    "line: 0",
    "line: -3",
    "line: -0",
    "line: +5",
    "line: 1_0",
    "line: 1__0",
    "line: _1",
    "line: 00012 trailing words",
    "line: 99999999999999999999999999999",
    "line: x",
    "line:",
    "line:    ",
    "line: " + "9" * (INT_MAX_STR_DIGITS + 1),
    "line: " + "1_" * 2200 + "1",
    "variable:",
    "variable: nope",
    "something else",
    "  function: padded  ",
    "class: First\nfunction: second\n\n  line: 2  ",
    "line: 3\x1cline: 4",
    "line:" + IDEOGRAPHIC_SPACE + "5",
]

STABLE_IDS = (
    "py:src/app/svc.py::Invoice.total",
    "py:src/app/svc.py::Invoice.total#2",
    "a:b::c::d",
    "a:b:c::d",
    "py:a.py::f::rationale@3",
    "py:a.py::f::rationale@3#2",
    "",
    "py",
    "py:a.py",
    ":a.py::f",
    "py:::f",
    "py: ::f",
    "py:a.py:: ",
    "py:a.py::" + IDEOGRAPHIC_SPACE,
    "py:a.py::\x1c",
    "py:a\n.py::f",
    "py:a.py::f\x7f",
    "py:a.py::f\t",
    " py:a.py::f",
    "py:a.py::é",
)

ORDINALS = (
    "f",
    "f#2",
    "f#0",
    "f#1",
    "f#000002",
    "f#123456",
    "f#1234567",
    "f#12\n",
    "f#12\n\n",
    "a#1#2",
    "a#1\n#2",
    "#3",
    "f#",
    "f#-1",
    "f#2 ",
    "C.m#3",
)

SIGNATURES = (
    "",
    "def f(a,\n      b):",
    "  leading and trailing  ",
    "tab\tand\x0bvertical\x0cfeed",
    "file\x1cgroup\x1drecord\x1eunit\x1fseparators",
    INVISIBLE + LINE_SEPARATOR + "end",
    "x" * 80,
    "x" * 81,
    "é" * 81,
    "中文" * 50,
)

UPPER_CASE = ("ABC", "A1_", "a", "_A", "A\n", "A\n\n", "ÀB", "A-B", "", "A", "1A")

BASES = ("Generic[T]", "enum.Enum", "metaclass=ABCMeta", " x . y ", "", "a.b[c.d]", "Protocol")

MENTION_NAMES = ("é", "", "TOTAL")


# Nesting depths: within the reference's recursion limit, past it, and past the indentation depth
# that the Python grammar parses without an error.
DEPTHS = (
    ("defs", 500),
    ("defs", 3000),
    ("classes", 400),
    ("classes", 500),
    ("c_declarators", 500),
    ("c_declarators", 3000),
)

DEVIATION_LINES = (
    "line: " + ARABIC_THREE,
    "line: " + FULLWIDTH_FIVE,
    "line: 1" + ARABIC_THREE,
)
DEVIATION_ORDINALS = ("f#" + ARABIC_THREE, "f#1" + ARABIC_THREE, "f#" + FULLWIDTH_TWO)

VARIANTS: tuple[tuple[str, tuple[str, ...], Callable[[str], str]], ...] = (
    (
        "crlf",
        ("repo_py/pricing.py", "tier1/pricing.c", "repo_go/pricing.go", "repo_ts/pricing.ts"),
        lambda text: text.replace("\n", "\r\n"),
    ),
    (
        "bom",
        ("repo_py/reports.py", "tier1/pricing.cpp", "repo_ts/types.ts", "tier2/config.json"),
        lambda text: BYTE_ORDER_MARK + text,
    ),
    ("tabs", ("repo_py/inventory.py", "tier1/pricing.rs"), tabbed),
)

PYTHON_EDGES = _text(
    r'''
    """Module docstring. NOTE: a string, not a comment, so no rationale."""
    from __future__ import annotations

    import os, sys as system
    import a.b as ab
    import a.b.c
    from . import sibling as sib
    from .. import *
    from ...pkg.mod import (first, second as two)
    from typing import TYPE_CHECKING, Protocol

    if TYPE_CHECKING:
        from collections.abc import Iterator  # NOTE: imported for typing only
        LIMIT = 3

    MAX_ITEMS = 10
    max_items = 3
    CONFIG = {
        "a": 1,
        "b": [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22],
    }
    type Pair = tuple[int, int]


    # TODO: drop the legacy path (ADR-12, rfc#9, RFC 7231)
    def outer(x, /, *args, y: int = MAX_ITEMS, **kwargs) -> list[int]:
        r"""Raw docstring with "quotes"."""
        # WHY: the walrus binds in the enclosing scope
        if (n := len(args)) > 1:
            return [n for _ in args]

        def inner():
            nonlocal x
            x = 1
            return ab.value + system.path + os.sep

        @staticmethod
        def decorated():
            return inner()

        class Local:
            def hidden(self):
                pass

        return inner


    async def fetch(url: str) -> bytes:
        """Fetch one URL."""
        global MAX_ITEMS
        async with session(url) as response:
            return await response.read()


    @dataclass
    class Point:
        """A point.

        HACK: a docstring marker is not a comment.
        """

        x: int = 0
        ys = [x for _ in range(3)]

        def norm(self) -> float:
            return (self.x ** 2) ** 0.5

        @property
        def double(self):
            def helper():
                return 2
            return helper() * self.x


    @dataclasses.dataclass(frozen=True)
    class Frozen(Base, metaclass=ABCMeta):
        'He said """hi""" here.'


    class Shape(Protocol):
        def area(self) -> float: ...

    '''
) + _text(
    r"""
    class Color(enum.Enum):
        '''Colors, with a docstring longer than two hundred characters so that the docstring form
        of the skeleton cuts it at its limit and adds the sentinel that marks a cut, which the
        reader then sees at the end of the one line.'''
        RED = 1


    class Color(str, IntEnum):  # NOTE: a second Color takes the ordinal #2
        pass


    def outer():  # NOTE: NOTE: two markers, one line
        return lambda q: [q for q in range(q)]


    squares = lambda v: {k: v for k in v}
    result = fetch(url=os.sep)
    print(len(result), type(result), squares, Iterator)
    """
)

RATIONALE_LINES_PY = _text(
    r"""
    def lines():
        # NOTE: plain
        # NOTE:no space after the colon
        # NOTE :  space before the colon
        # NOTE:
        # note: lower case is no marker
        # éNOTE: a word character before the marker
        # _TODO: an underscore is a word character
        # (WHY: punctuation before the marker
        # HACK:{tab}tab after the colon
        # ADR-12 and adr 7 and RFC#9 and rfc:: 10
        # XADR1 and ADR12x and ADR_3
        # RFC ٣٤ uses Arabic-Indic digits
        # ADR-٣x has a word character after the digits
        # TODO: with a citation ADR-0042
        # NOTE: {long}
        # WHY:   trailing spaces{trailing}
        # //NOTE: a nested line comment prefix
        # * TODO: a star prefix
        return 1
    """
).format(long="Ünïcödé counts one code point each, not bytes. " * 6, trailing="   ", tab="\t")

RATIONALE_LINES_C = _text(
    """
    int lines(void) {
        /* NOTE: one block on one line */
        /*
         * WHY: a star prefix inside a block
         * HACK: two markers in one block
         */
        // TODO: a line comment
        /* ADR-1 */ /* RFC-2 */
        int x = 1; /* NOTE: after code */ int y = 2;
        /** NOTE: a doc block -->*/
        return x + y;
    }
    """
)

TYPESCRIPT_EDGES = _text(
    """
    import Default, { Named as Alias, Other } from "./module";
    import * as ns from '../ns';
    import type { T } from "./types";
    import "./side-effect";
    export { Re } from "./re";

    // NOTE: arrow functions are declarations here
    export const App = () => { return 1; };
    const handler = function named(a: number): string { return ""; };
    const plain = 42;
    let [first, second] = pair();

    /* WHY: interfaces carry no body
       HACK: two markers in one block comment */
    export interface Shape { area(): number; }

    export abstract class Base {
      // NOTE: inside a class body
      private secret(): void {}
      protected guarded(): void {}
      public open(): number { return 1; }
      _underscore(): void {}
      static make = (x: number) => new Base();
    }

    namespace A { export function f() {} }
    namespace B { export function f() {} }

    function* gen() { yield 1; }
    export default function () {}
    /** TODO: a doc comment, RFC 7231 */
    function documented(): Promise<void> {
      // TODO: inside a function body (RFC 2119)
      /* NOTE: a block comment inside */
      return ns.run(Default, Alias, Other);
    }
    """
)

JAVASCRIPT_EDGES = _text(
    """
    import def, { a as b } from "./m.js";
    const fs = require("fs");
    <!-- NOTE: an HTML-like comment -->

    // TODO: a private method stays public in the table
    class Widget {
      _hidden() { return 1; }
      #really() { return 2; }
      static build() { return new Widget(); }
    }

    const load = async (path) => { return fs.readFileSync(path); };
    const gen = function* () { yield 1; };
    var legacy = function () {};
    module.exports = { load, Widget };
    """
)

TSX_EDGES = _text(
    """
    import React, { useState } from "react";

    // NOTE: a component is a function-valued binding
    export const Counter = ({ start }: { start: number }) => {
      const [count, setCount] = useState(start);
      return <button onClick={() => setCount(count + 1)}>{count}</button>;
    };

    export class Legacy extends React.Component {
      render() { return <div />; }
    }

    function helper(): JSX.Element { /* WHY: JSX here */ return <span>é</span>; }
    """
)

GO_EDGES = _text(
    """
    package main

    import (
    \t"fmt"
    \tstr "strings"
    \t_ "embed"
    \t. "math"
    )

    // NOTE: a generic receiver
    type Stack[T any] struct{ items []T }

    func (s *Stack[T]) Push(v T) { s.items = append(s.items, v) }

    func (s Stack[T]) Len() int { return len(s.items) }

    type Reader interface {
    \tRead(p []byte) (n int, err error)
    }

    type ID = string

    const Max = 10

    var global = 1

    /* HACK: a block comment
       TODO: a second marker */
    func main() {
    \t// WHY: inside a function body, RFC 1149
    \tf := func() { fmt.Println(str.ToUpper("x"), Pi) }
    \tf()
    }

    func (c *Config) Validate() error { return nil }

    func (c *Other) Validate() error { return nil }
    """
)

RUST_DOC_COMMENTS = _text(
    """
    //! Crate docs. NOTE: an inner doc comment.

    use std::collections::{HashMap, BTreeMap as Ordered, self};
    use crate::model::Item as Thing;
    use super::*;
    use self::inner::{deep::{Leaf}, Other};

    /// A doc comment with two underscores between them.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Point { x: i32, y: i32 }

    /** WHY: a block doc comment */
    pub(crate) enum Shape { Circle, Square }

    pub trait Area { fn area(&self) -> f64; }

    impl Area for i32 { fn area(&self) -> f64 { 0.0 } }

    impl<T> Wrapper<T> {
        pub async fn get(&self) -> Option<&T> { None }
        fn helper() {
            fn nested() {}
        }
    }

    const LIMIT: usize = 10;
    const UNTYPED = 3;
    pub type Map = HashMap<String, Ordered<u8, u8>>;

    mod inner {
        // TODO: items inside a module
        pub fn inside() -> u8 { 1 }
        #[cfg(test)]
        mod tests {
            #[test]
            fn works() {}
        }
    }

    macro_rules! noop { () => {}; }
    extern "C" { fn ext(x: i32) -> i32; }
    unsafe fn raw() {}
    """
)

C_EDGES = _text(
    """
    #ifndef EDGES_H
    #define EDGES_H

    #include <stdio.h>
    #include "local.h"
    #include  < spaced.h >

    // NOTE: a prototype inside an include guard
    double apply_tax(double amount, double rate);

    struct outer {
        struct inner { int a; } nested;
        int (*callback)(int);
    };

    typedef struct { int x; } anonymous_t;

    enum color { RED, GREEN };

    void (*handler)(int);

    int *make(void) { return 0; }

    int (*pick(void))(int) { return 0; }

    /* HACK: K&R style below */
    static int old(a) int a; { return a; }

    #endif
    """
)

CPP_NESTED = _text(
    """
    #include <vector>
    #include "pricing.hpp"

    namespace billing {
    namespace detail {

    // NOTE: a class nested two levels deep
    class Ledger {
    public:
        // HACK: inside a class body
        class Entry {
        public:
            struct Amount { long cents; };
            void post();
            bool operator==(const Entry& other) const;
        };
        Ledger();
        ~Ledger();
        void reset();
        int field_;
    };

    }  // namespace detail

    template <typename T>
    class Box {
        T value;
        T get() const { return value; }
    };

    template <typename T>
    T Box<T>::go() { return value; }

    void detail::Ledger::Entry::post() {}
    detail::Ledger::Ledger() {}
    detail::Ledger::~Ledger() {}
    bool detail::Ledger::Entry::operator==(const Entry& other) const { return true; }

    enum class Mode { Fast, Slow };
    void overloaded(int);
    void overloaded(double);
    }  // namespace billing
    """
)

JAVA_EDGES = _text(
    """
    package app;

    import static com.acme.Tax.RATE;
    import com.acme.*;
    import com.acme.money.Money;

    /** NOTE: a javadoc block */
    public class Edges {
        public static final double TAX_RATE = 0.2;
        private final double subtotal;
        static final int LIMIT = 3, OTHER = 4;

        public Edges(double subtotal) { this.subtotal = subtotal; }

        private double secret() { return subtotal; }

        protected class Inner {
            void run() {}
        }

        // TODO: records and enums
        record Pair(int a, int b) {
            Pair { if (a < 0) throw new IllegalArgumentException(); }
        }

        enum Mode { FAST, SLOW; void go() {} }

        interface Visitor { void visit(Edges e); }
    }
    """
)

RUBY_EDGES = _text(
    """
    require 'json'
    require_relative 'helper'
    require("net/http")

    =begin
    NOTE: a block comment
    =end

    module Billing
      # TODO: a class inside a module
      class Ledger
        def self.open(path)
          new(path)
        end

        def _hidden; end

        def total
          @total ||= 0
        end
      end
    end

    def top_level(a, b = 2) = a + b
    """
)

BASH_EDGES = _text(
    """
    #!/usr/bin/env bash
    . ./lib.sh
    source "$DIR/other.sh"
    source
    . '/etc/profile'

    # NOTE: two function spellings
    function greet {
      echo "hi"
    }

    _private() {
      # HACK: inside a body
      local x="$(greet)"
    }
    """
)

LUA_EDGES = _text(
    """
    local json = require("json")
    local http = require "socket.http"
    local deep = require('a.b.c')

    --[[ NOTE: a block comment
    WHY: two markers ]]
    local M = {}
    local CONSTANT = 42
    local lower = 1
    local UNSET
    local f = function(a, b) return a + b end

    M.version = "1.0"
    M.greet = function(name) return "hi " .. name end
    M.nested.deep = 3
    a, b = 1, 2
    _hidden = true

    -- TODO: a method declaration
    function M:method(x) return x end

    function M.plain()
      -- NOTE: inside a function
      --[[ WHY: a block comment inside ]]
      return 1
    end

    local function helper() end

    return M
    """
)

PHP_SECOND = _text(
    r"""
    <?php

    namespace App\Ledger;

    use App\Money\Currency as Cash;
    use function App\Tax\apply;
    use App\Shapes\{Circle, Square};

    # NOTE: a hash comment
    trait Loggable
    {
        protected function log(string $m): void {}
    }

    enum Suit: string
    {
        case Hearts = 'H';
        public function color(): string { return 'red'; }
    }

    // TODO: a static method
    final class Ledger
    {
        use Loggable;

        private static function make(): self { return new self(); }

        public function total(): int { /* WHY: inline */ return 0; }
    }

    function _helper() {}
    """
)

KOTLIN_SECOND = _text(
    """
    package app.ledger

    import app.money.Currency as Cash
    import app.shapes.*

    // NOTE: an object declaration
    object Registry {
        // NOTE: inside an object
        /* WHY: a block comment inside */
        private fun secret() = 1
        internal fun shared() = 2
        fun open(): Int { return 3 }
    }

    /* WHY: a block comment */
    data class Pair(val a: Int, val b: Int)

    sealed class Shape {
        class Circle(val r: Double) : Shape()
    }

    fun String.shout(): String = uppercase()

    suspend fun load(): Cash = TODO()
    """
)

SWIFT_SECOND = _text(
    """
    import UIKit.UIView

    // NOTE: struct, enum and actor share a node type
    struct Point { var x = 0 }

    enum Mode { case fast, slow }

    actor Counter {
        // TODO: inside an actor
        /* NOTE: a block comment inside */
        private var value = 0
        func increment() { value += 1 }
    }

    extension Point {
        fileprivate func hidden() {}
        init(both: Int) { self.x = both }
    }

    /* TODO: a protocol with a requirement */
    protocol Drawable {
        func draw()
    }

    func _helper() -> Int { 1 }
    """
)

SCALA_SECOND = _text(
    """
    package app.ledger

    import app.money.{Currency, Rate}
    import app.shapes._
    import app.tax.Tax

    // NOTE: an object
    object Registry {
      // NOTE: inside an object
      /* WHY: a block comment inside */
      private def secret: Int = 1
      def open(): Int = 3
    }

    /* WHY: a trait with an abstract member */
    trait Shape {
      def area: Double
    }

    case class Pair(a: Int, b: Int)

    enum Mode {
      case Fast, Slow
    }

    def topLevel(x: Int): Int = x + 1
    """
)

CSHARP_SECOND = _text(
    """
    using System;
    using static System.Math;
    using Alias = App.Money.Currency;

    namespace App.Ledger
    {
        /// <summary>NOTE: a doc comment</summary>
        public class Ledger
        {
            // NOTE: inside a class body
            /* TODO: a block comment inside */
            private int secret() => 1;
            internal int Shared() => 2;
            public async Task<int> LoadAsync() { return await Task.FromResult(3); }

            public class Entry { public void Post() {} }

            public int Total()
            {
                int Local(int x) => x + 1;
                return Local(1);
            }
        }

        /* TODO: other type declarations */
        public struct Point { public int X; }
        public record Pair(int A, int B);
        public enum Mode { Fast, Slow }
        public interface IShape { double Area(); }
    }
    """
)

JSON_SECOND = _text(
    """
    {
      "name": "ledger",
      "dotted.key": 1,
      "escaped \\"key\\"": 2,
      "nested": {
        // NOTE: a comment that the JSON grammar accepts
        "deep": {"leaf": true}, "list": [{"inner": 1}, 2]
      },
      "empty": {},
      "array": [1, 2, 3]
    }
    """
)

TOML_TABLES = _text(
    """
    # NOTE: table arrays and dotted keys
    title = "ledger"
    a.b.c = 1
    "quoted.key" = 2
    'literal key' = 3
    point = { x = 1, y = 2 }

    [owner]
    # TODO: inside a table, ADR-7
    name = "x"

    [a.b]
    d = 4

    [x."y.z"]
    w = 5

    [[products]]
    name = "Hammer"
    sku = 738594937

    [[products]]
    name = "Nail"

    [[products.variants]]
    size = "small"
    """
)

YAML_SECOND = _text(
    """
    # TODO: nested mappings and sequences
    service:
      name: ledger
      # NOTE: inside a mapping
      ports:
        - 8080
        - 8081
      routes:
        - path: /a
          handler: a
        - {path: /b, handler: b}
    "quoted key": 1
    anchors: &base
      x: 1
    derived:
      <<: *base
    ---
    second: document
    """
)

HCL_SECOND = _text(
    """
    # NOTE: a hash comment
    variable "region" {
      default = "us-east-1"
    }

    // WHY: a nested block
    resource "aws_instance" "web" {
      # NOTE: inside a block
      // TODO: a second comment syntax inside
      ami = "ami-123"
      lifecycle {
        create_before_destroy = true
      }
      tags = {
        Name = "web"
      }
    }

    /* TODO: locals */
    locals {
      prefix = "app"
      multi = <<EOT
    line
    EOT
    }
    """
)

SQL_SECOND = _text(
    """
    -- NOTE: a line comment
    CREATE TABLE IF NOT EXISTS ledger.entries (
        -- NOTE: inside a table
        id INTEGER PRIMARY KEY, /* WHY: a block comment inside */
        "amount" NUMERIC(10, 2) NOT NULL,
        note TEXT
    );

    /* WHY: a block comment */
    CREATE VIEW recent AS
    SELECT id, amount FROM ledger.entries WHERE id > 10;

    CREATE TABLE plain (x INT);
    """
)

UNICODE_PY = _text(
    """
    def café(naïve="é"):  # NOTE: ü before the code
        return naïve


    class Größe:
        '''Größe docstring.'''

        def 中文(self): return "中文"
    """
)

UNICODE_RS = _text(
    """
    fn größe() -> &'static str { /* NOTE: 中文 */ "é" }

    struct Café { naïve: u8 }

    impl Café { fn 名前(&self) -> u8 { self.naïve } }
    """
)

UNICODE_JS = _text(
    """
    const café = (naïve) => { return "é"; };
    function größe() { /* TODO: ü */ return 1; }
    class Café { 名前() { return 1; } }
    """
)

# Each file of the extension: its name and its source. The language follows from the suffix.
EXTENSION: tuple[tuple[str, str], ...] = (
    ("extension/edges.py", PYTHON_EDGES),
    ("extension/rationale_lines.py", RATIONALE_LINES_PY),
    ("extension/rationale_lines.c", RATIONALE_LINES_C),
    ("extension/edges.ts", TYPESCRIPT_EDGES),
    ("extension/edges.js", JAVASCRIPT_EDGES),
    ("extension/edges.tsx", TSX_EDGES),
    ("extension/edges.go", GO_EDGES),
    ("extension/doc_comments.rs", RUST_DOC_COMMENTS),
    ("extension/edges.h", C_EDGES),
    ("extension/nested.cpp", CPP_NESTED),
    ("extension/Edges.java", JAVA_EDGES),
    ("extension/edges.rb", RUBY_EDGES),
    ("extension/edges.sh", BASH_EDGES),
    ("extension/edges.lua", LUA_EDGES),
    ("extension/second.php", PHP_SECOND),
    ("extension/Second.kt", KOTLIN_SECOND),
    ("extension/Second.swift", SWIFT_SECOND),
    ("extension/Second.scala", SCALA_SECOND),
    ("extension/Second.cs", CSHARP_SECOND),
    ("extension/second.json", JSON_SECOND),
    ("extension/tables.toml", TOML_TABLES),
    ("extension/second.yaml", YAML_SECOND),
    ("extension/second.tf", HCL_SECOND),
    ("extension/second.sql", SQL_SECOND),
    ("extension/unicode.py", UNICODE_PY),
    ("extension/unicode.rs", UNICODE_RS),
    ("extension/unicode.js", UNICODE_JS),
    # Sources that do not parse, for the error spans and the unparsed mentions.
    ("extension/broken.py", "def f(:\n    total = 1\n    return total total\n"),
    ("extension/broken.js", "function (\n  total += Total;\n}}\n"),
    ("extension/broken.c", "int f( { total; totaltotal; }\n"),
    ("extension/broken.go", "func (s *T Push() { aaa := aaaa }\n"),
)


if __name__ == "__main__":
    sys.exit(main())
