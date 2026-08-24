"""Skeletonizer: keep signatures and constants, strip bodies.

The A3 semantics from Agentless' ``compress_file.py``, re-done on tree-sitter
spans so it is not Python-only:

* module level keeps class definitions, function signatures (decorators and
  return annotations included) and assignments, so constants and class-level
  attributes survive;
* top-level import lines are KEPT, unlike Agentless which dropped them.
  Dropping imports throws away the dependency context that makes a skeleton
  useful for localization, and import lines are the cheapest tokens in a
  file;
* every function body is replaced by a ``...`` sentinel at the body's own
  indentation;
* comments are always stripped, and docstrings are stripped by default
  (``docstrings=True`` keeps them, truncated to 200 characters) -- token
  economy and prompt-injection surface in one decision.

Everything else is emitted verbatim from the source, so the output still
reads as code. Original line numbers survive: nothing is renumbered, and
the ``number_lines`` option renders them as ``N| `` prefixes.
"""

import re
from collections.abc import Iterator, Mapping
from dataclasses import dataclass, field

from tree_sitter import Node

from agentless_mcp.core import grammars
from agentless_mcp.core.extractor import BODY_BLOCK_NODE_TYPES, LANGUAGE_CONFIGS
from agentless_mcp.core.slices import line_prefix
from agentless_mcp.util.tokens import Chars4Counter, TokenCounter

SENTINEL = "..."
DOCSTRING_MAX_CHARS = 200

# Node types whose body is a block worth eliding. Keyed on what the node IS
# (a braced or indented statement block), not on the parent's name, so a
# language whose functions are spelled differently still hits the same rule.
# The table lives with the other node-type tables in `core.extractor`.
_BLOCK_TYPES = BODY_BLOCK_NODE_TYPES

# Function-like node types the LanguageConfig table does not already carry:
# the languages whose extraction is done by a dedicated handler, plus the
# member-function spellings the symbol table has no entry for.
_EXTRA_FUNCTION_NODE_TYPES: dict[str, frozenset[str]] = {
    "python": frozenset({"function_definition"}),
    "rust": frozenset({"function_item"}),
    "c": frozenset({"function_definition"}),
    "cpp": frozenset({"function_definition"}),
    "lua": frozenset({"function_declaration", "function_definition"}),
    "javascript": frozenset({"method_definition", "function_expression", "arrow_function"}),
    "typescript": frozenset({"method_definition", "function_expression", "arrow_function"}),
    "tsx": frozenset({"method_definition", "function_expression", "arrow_function"}),
    "go": frozenset({"func_literal"}),
    "java": frozenset({"method_declaration", "constructor_declaration"}),
    "ruby": frozenset({"method", "singleton_method"}),
}

_WHITESPACE_RUN = re.compile(r"\s+")

# A Python string prefix (`r`, `b`, `f`, `rb`, `Rb`, ...) sits between the
# start of the literal and its quote run. Bounded at three letters, which is
# one more than any prefix the language spells, so a bare identifier followed
# by a quote cannot be eaten whole.
_STRING_PREFIX = re.compile(r"^[A-Za-z]{0,3}(?=['\"])")


def function_node_types(language: str) -> frozenset[str]:
    """Return the node types treated as functions for ``language``."""
    config = LANGUAGE_CONFIGS.get(language)
    configured = frozenset(config.function_node_types) if config else frozenset()
    return configured | _EXTRA_FUNCTION_NODE_TYPES.get(language, frozenset())


@dataclass
class _Plan:
    """Which lines to drop and which to replace, keyed by 1-based line number.

    Three rules write here -- function bodies, comments and docstrings -- and
    one of them outranks the other two. ``owned`` is the set of lines an
    elided body has already claimed, and nothing may write inside it
    afterwards: "an elided body emits exactly one sentinel and no source" is
    the invariant the whole module exists for, and last-write-wins let a
    trailing comment overwrite the sentinel with the real body line (leaving a
    truncated function that reads as a complete one) and a nested function add
    a second, over-indented sentinel inside an already-elided body.
    """

    dropped: set[int] = field(default_factory=set)
    replaced: dict[int, str] = field(default_factory=dict)
    owned: set[int] = field(default_factory=set)

    def drop_range(self, first: int, last: int) -> None:
        """Drop every line in the inclusive range."""
        self.dropped.update(range(first, last + 1))

    def replace(self, line: int, text: str) -> None:
        """Replace one line, unless an elided body already owns it."""
        if line in self.owned:
            return
        self.dropped.discard(line)
        self.replaced[line] = text

    def owns(self, line: int) -> bool:
        """True when an elided body has already claimed this line."""
        return line in self.owned

    def elide(self, first: int, last: int, kept: Mapping[int, str]) -> None:
        """Replace one body's lines with ``kept`` and claim the range.

        One call rather than three, because dropping the range, writing the
        sentinel and taking ownership are one decision: a later rule that
        found the range dropped but unowned is exactly how the sentinel used
        to get overwritten.
        """
        self.dropped.update(range(first, last + 1))
        for line, replacement in kept.items():
            self.dropped.discard(line)
            self.replaced[line] = replacement
        self.owned.update(range(first, last + 1))


def skeletonize(
    source: str,
    language: str,
    *,
    docstrings: bool = False,
    number_lines: bool = False,
) -> str:
    """Return the skeleton of ``source``.

    Raises ``LanguageUnavailable`` (from :mod:`agentless_mcp.core.grammars`)
    when the grammar is not warmed: a skeleton built without a parser would
    be a plausible-looking lie.
    """
    parser = grammars.get_parser(language)
    data = source.encode("utf-8")
    tree = parser.parse(data)
    lines = source.split("\n")

    plan = _Plan()
    functions = function_node_types(language)
    for node in _walk(tree.root_node):
        if node.type in functions:
            _plan_function(node, lines, plan, keep_docstring=docstrings)
        elif "comment" in node.type:
            _plan_comment(node, lines, plan)
        elif _is_leading_docstring(node):
            _plan_docstring(node, lines, plan, keep=docstrings)

    return _render(lines, plan, number_lines=number_lines)


def compression_ratio(
    source: str,
    skeleton: str,
    counter: TokenCounter | None = None,
) -> float:
    """Return source tokens divided by skeleton tokens, floored at one token."""
    counted = counter if counter is not None else Chars4Counter()
    return counted.count(source) / max(1, counted.count(skeleton))


def _char_column(line: str, byte_column: int) -> int:
    """Convert a tree-sitter byte column into an index into the decoded line.

    Tree-sitter counts a column in UTF-8 bytes; every column this module uses
    indexes a Python ``str``. The two agree only while the line is ASCII, and
    slicing a ``str`` past its end never raises, so one accented character
    earlier on the line silently moves every later cut -- leaking source past
    a sentinel, or leaving half a comment marker behind. Converting here is
    the one place the two index spaces meet.

    The decode is strict: a node boundary that does not land on a codepoint
    boundary would mean the parser reported a span the source does not have.
    """
    if line.isascii():
        return byte_column
    return len(line.encode("utf-8")[:byte_column].decode("utf-8"))


def _walk(root: Node) -> Iterator[Node]:
    """Yield every node in the tree, parents before children."""
    stack = [root]
    while stack:
        node = stack.pop()
        yield node
        stack.extend(reversed(node.children))


def _body_of(node: Node) -> Node | None:
    """Return the node's block body, or None when it has none worth eliding."""
    body = node.child_by_field_name("body")
    if body is not None and body.type in _BLOCK_TYPES:
        return body
    if body is not None:
        return None
    for child in reversed(node.children):
        if child.type in _BLOCK_TYPES:
            return child
    return None


def _plan_function(node: Node, lines: list[str], plan: _Plan, *, keep_docstring: bool) -> None:
    """Replace a function body with the sentinel, keeping its signature."""
    body = _body_of(node)
    if body is None:
        return

    body_first = body.start_point[0] + 1
    body_last = body.end_point[0] + 1
    if plan.owns(body_first):
        # A nested function inside a body that is already elided. The
        # enclosing sentinel stands for this one too; a second sentinel at the
        # inner body's own indentation would sit inside a body the reader
        # cannot see, and would not re-parse.
        return

    # The line the body opens on, which is the signature's own line only when
    # the body starts there. Naming it for what it holds is what stops the
    # Allman branch below reading as dead code.
    body_line = lines[body_first - 1]
    body_column = _char_column(body_line, body.start_point[1])
    opens_brace = body_line[body_column : body_column + 1] == "{"

    docstring = _body_docstring(body) if keep_docstring else None

    if body_column > len(body_line) - len(body_line.lstrip()):
        # The body opens on the signature's own line: keep the prefix and put
        # the sentinel where the body was.
        prefix = body_line[:body_column].rstrip()
        sentinel = f"{prefix} {{ {SENTINEL} }}" if opens_brace else f"{prefix} {SENTINEL}"
        plan.elide(body_first, body_last, {body_first: sentinel})
        return

    indent = " " * body_column
    if opens_brace:
        # Allman style: the brace opens its own line. Replacing the whole body
        # with a bare sentinel left the signature followed by `...` and no
        # block at all, which does not re-parse as C, C++, Java or Go. The
        # braces are emitted around the sentinel instead.
        plan.elide(body_first, body_last, {body_first: f"{indent}{{ {SENTINEL} }}"})
        return

    if docstring is not None:
        doc_first = docstring.start_point[0] + 1
        doc_last = docstring.end_point[0] + 1
        kept = {doc_first: indent + _one_line_docstring(docstring, lines)}
        if body_last > doc_last:
            kept[body_last] = indent + SENTINEL
        plan.elide(body_first, body_last, kept)
        return

    plan.elide(body_first, body_last, {body_first: indent + SENTINEL})


def _plan_comment(node: Node, lines: list[str], plan: _Plan) -> None:
    """Drop a comment, keeping whatever code shares its first or last line.

    A `#` comment runs to the end of its line, so only the text before it can
    be code. A `/* ... */` comment does not: it can sit between two pieces of
    an expression, on one line or across several. Keeping only the head
    rewrote `const X = 1 /* note */ + 2` into `const X = 1`, which still reads
    as a complete constant with a different value.
    """
    first = node.start_point[0] + 1
    last = node.end_point[0] + 1
    first_line = lines[first - 1]
    last_line = lines[last - 1]
    head = first_line[: _char_column(first_line, node.start_point[1])]
    tail = last_line[_char_column(last_line, node.end_point[1]) :]

    if not head.strip() and not tail.strip():
        plan.drop_range(first, last)
        return

    plan.drop_range(first + 1, last)
    # A comment opening its own line keeps that line's indentation; one with
    # code before it keeps the code and loses the whitespace it left behind.
    kept = head.rstrip() + tail if head.strip() else head + tail.lstrip()
    plan.replace(first, kept.rstrip())


def _plan_docstring(node: Node, lines: list[str], plan: _Plan, *, keep: bool) -> None:
    """Drop a module- or class-level docstring, or truncate it when kept."""
    first = node.start_point[0] + 1
    last = node.end_point[0] + 1
    if not keep:
        plan.drop_range(first, last)
        return

    indent = " " * _char_column(lines[first - 1], node.start_point[1])
    plan.drop_range(first, last)
    plan.replace(first, indent + _one_line_docstring(node, lines))


def _is_string_statement(node: Node) -> bool:
    """True for a bare string statement, wrapper node or not.

    tree-sitter-python 1.14.3 emits docstrings as a bare ``string`` child of
    the enclosing body; older revisions wrap them in an
    ``expression_statement``. Both shapes are recognised so the skeletonizer
    does not start leaking docstrings the day a grammar is bumped.
    """
    if node.type == "string":
        return True
    return (
        node.type == "expression_statement"
        and bool(node.children)
        and node.children[0].type == "string"
    )


def _is_leading_docstring(node: Node) -> bool:
    """True for a string statement that opens a module or a class body."""
    if not _is_string_statement(node):
        return False
    parent = node.parent
    if parent is None:
        return False
    if parent.type not in ("module", "block", "class_body", "program"):
        return False
    if parent.type == "block" and (parent.parent is None or "class" not in parent.parent.type):
        # Function bodies are handled by the function rule; only class bodies
        # and modules reach here.
        return False
    named = [child for child in parent.children if child.is_named]
    return bool(named) and named[0] == node


def _body_docstring(body: Node) -> Node | None:
    """Return the docstring statement opening a function body, if any."""
    named = [child for child in body.children if child.is_named]
    if not named:
        return None
    first = named[0]
    return first if _is_string_statement(first) else None


def _one_line_docstring(node: Node, lines: list[str]) -> str:
    """Render a docstring node as one truncated line, quotes preserved."""
    text = _node_text(node, lines).strip()
    # The quote run is matched against the literal with its string prefix
    # removed: `r"""docs."""` matched against the whole literal starts with
    # `r`, not with `"""`, so the unstripped literal used to be wrapped in a
    # second pair of quotes and the result did not parse. The prefix is put
    # back, because dropping it changes what the literal means.
    match = _STRING_PREFIX.match(text)
    prefix = match.group(0) if match else ""
    inner = text[len(prefix) :]
    quote = '"""' if '"""' in inner else ("'''" if "'''" in inner else inner[:1])
    if quote and inner.startswith(quote) and inner.endswith(quote) and len(inner) >= 2 * len(quote):
        inner = inner[len(quote) : -len(quote)]
    flattened = _WHITESPACE_RUN.sub(" ", inner).strip()
    if len(flattened) > DOCSTRING_MAX_CHARS:
        flattened = flattened[: DOCSTRING_MAX_CHARS - 3] + SENTINEL
    return f"{prefix}{quote}{flattened}{quote}"


def _node_text(node: Node, lines: list[str]) -> str:
    """Return a node's source text, reconstructed from the split lines."""
    first, first_byte = node.start_point
    last, last_byte = node.end_point
    first_column = _char_column(lines[first], first_byte)
    last_column = _char_column(lines[last], last_byte)
    if first == last:
        return lines[first][first_column:last_column]
    parts = [lines[first][first_column:]]
    parts.extend(lines[first + 1 : last])
    parts.append(lines[last][:last_column])
    return "\n".join(parts)


def _render(lines: list[str], plan: _Plan, *, number_lines: bool) -> str:
    """Emit the surviving lines, collapsing the blank runs removal leaves."""
    rendered: list[str] = []
    previous_blank = True
    # How long `rendered` was after the last line that carried content. Read
    # back from the rendered text instead, the trim never fired under
    # `number_lines`: a blank line is emitted as `2| `, and no amount of
    # stripping makes a line prefix blank once it carries a digit.
    content_end = 0
    for number, original in enumerate(lines, start=1):
        if number in plan.dropped:
            continue
        text = plan.replaced.get(number, original)
        if not text.strip():
            if previous_blank:
                continue
            previous_blank = True
            rendered.append(line_prefix(number) if number_lines else "")
            continue
        previous_blank = False
        rendered.append(f"{line_prefix(number)}{text}" if number_lines else text)
        content_end = len(rendered)

    del rendered[content_end:]

    return "\n".join(rendered) + "\n" if rendered else ""
