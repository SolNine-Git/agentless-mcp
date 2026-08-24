"""AST-equivalence keys and syntax verdicts for candidate patches.

Two questions this module answers about a change, both of them structural and
neither of them requiring a model.

**Are these two patches the same patch?** Agentless answered that with
``ast.unparse``, which is Python-only. Here a change is reduced to the file's
stream of leaf tokens before and after, comments dropped and whitespace
collapsed; the key is a sha256 over the unified diff between the two streams,
and a patch's key is a sha256 over its per-file keys in path order.
Reformatting, re-commenting and re-indenting all normalise away; a change to
what the code does moves the key. That is what lets the vote cluster
equivalent samples before counting them, which is the ranking failure TRAE
reported from naive majority voting.

*Comments* here means commentary. A build constraint, a shebang, a ``# type:``
annotation, a ``# noqa`` and a ``@ts-expect-error`` all live in a comment node
and are all read by something -- so they stay in the stream, per
:data:`_DIRECTIVE`. Dropping them made a candidate that flips ``//go:build
linux`` to ``windows`` hash identically to the candidate that changes nothing,
and the vote then credited its samples to the no-op's class.

Two design points, both established by measurement rather than assumption.

*Every* leaf is emitted, not only the named ones. A named-leaf stream carries
identifiers and literals and drops every operator, because tree-sitter spells
`+` and `-` as anonymous nodes -- so `left + right` and `left - right` hashed
identically, and an operator flip is precisely what a large share of one-line
bug fixes is. Measured across python, typescript and go before this was
changed.

Python additionally gets an explicit block marker, the per-language fallback
the plan anticipated. Every other tier-1 language closes a block with a token
that is in the stream (`}`, `end`, `fi`); Python closes one with a DEDENT that
tree-sitter keeps hidden, so dedenting a statement out of an `if` produced a
byte-identical stream. The node types live in
:data:`agentless_mcp.core.extractor.INDENT_BLOCK_NODE_TYPES`, beside the other
per-grammar node-type tables.

**Did this patch break the file?** Not "does the file parse" -- a repository
with a pre-existing parse error under this grammar would fail every candidate
including the right one -- but "does it parse *no worse than before*". The
verdict compares ERROR and MISSING node counts between the old and the new
content, so a file that already had two error nodes is judged on whether it
still has two.

A file whose language has no grammar here is not skipped. It falls back to a
whitespace-normalised comparison of the raw text, which catches a semantic
change and normalises away reformatting, and both answers say so in the value
rather than only in this docstring: the key carries
:data:`TEXT_ONLY_KEY_PREFIX` and the verdict carries ``checked=False``.
Labelling matters because the fallback is not a normalisation for a file
whose meaning depends on whitespace -- two YAML files with different nesting
collapse to one stream -- so a caller clustering candidates has to be able to
refuse the key.
"""

import difflib
import hashlib
import re
from collections.abc import Callable, Iterator, Mapping
from dataclasses import dataclass

from tree_sitter import Node, Parser

from agentless_mcp.core import grammars
from agentless_mcp.core.extractor import COMMENT_NODE_TYPES, INDENT_BLOCK_NODE_TYPES
from agentless_mcp.util.errors import LanguageUnavailable

# What a block boundary looks like in the normalised stream. Two control
# characters no source token is written with, so they do not collide with a
# leaf's own text. A string literal holding one of them raw is the exception,
# and it costs at most a wrong equivalence between two files that both do it.
BLOCK_OPEN = "\x01"
BLOCK_CLOSE = "\x02"

# The separator between a path and its per-file key when the patch key is
# built. Included so that the same edit made to two different files does not
# hash to one key -- the plan's "per-file keys concatenated in path-sorted
# order" fixes the order, and naming the path is what makes the order mean
# something.
_KEY_SEPARATOR = "\x00"

# The comment marker a directive can open with, in the languages this package
# parses: `#`, `//`, `/*` and Lua/SQL's `--`.
_COMMENT_MARKER = re.compile(r"^\s*(?:#|//|/\*|--)\s*")

# Comment bodies that are not commentary. Each of these is read by something
# -- the compiler, the interpreter, the type checker, a lint gate -- so two
# files differing only in one of them are not the same file, and clustering
# them together credits a candidate's votes to a class it does not belong to.
# Matched after the opening marker is stripped, anchored, and each ends at a
# boundary a prose comment will not reproduce: `# pragmatic` is prose,
# `# pragma: no cover` is a directive.
_DIRECTIVE = re.compile(
    r"^(?:"
    r"!"  # a shebang selects the interpreter
    r"|go:"  # //go:build, //go:embed, //go:generate
    r"|\+build\b"  # the pre-1.17 Go build constraint
    r"|-\*-"  # the PEP 263 coding cookie
    r"|type:"  # PEP 484 type comments
    r"|noqa\b"  # a suppressed diagnostic is still a decision
    r"|pragma:"  # coverage and compiler pragmas
    r"|@ts-"  # @ts-expect-error, @ts-ignore, @ts-nocheck
    r"|eslint-"  # eslint-disable and friends
    # The linter and formatter families, each of which this repository's own
    # stack reads: `# ruff: noqa` is the file-level form and does not begin
    # with `noqa`, and `fmt: off` switches formatting for a region.
    r"|ruff:"
    r"|pylint:"
    r"|mypy:"
    r"|flake8:"
    r"|coverage:"
    r"|fmt:"
    r"|yapf\b"
    r"|isort:"
    r"|prettier-"
    r")"
)

LanguageOf = Callable[[str], str | None]

# What a key built without a grammar is labelled with. A bare 64-character
# hex digest means an AST key; this prefix means the stream behind it is
# whitespace-collapsed raw text, which is a weaker claim: `a:\n  b: 1` and
# `a:\n    b: 1` share it, and so do `a: 1\nb: 2` and `a: 1 b: 2`. Carried in
# the value because that value is what reaches `core.vote` and patchlint's
# near-duplicate check, and clustering two files on a text-only key credits
# one candidate's samples to another candidate's class.
TEXT_ONLY_KEY_PREFIX = "text:"


@dataclass(frozen=True)
class SyntaxVerdict:
    """Whether an edited file parses no worse than it did before.

    ``ok`` is a *delta*, not an absolute: ``new_errors <= old_errors``. The
    counts are carried so a caller can tell "clean before, clean after" from
    "broken before, equally broken after", which are the same verdict and very
    different situations.

    ``checked`` says whether a parser ran at all. Without it "checked and
    clean" and "not checked, because this file has no grammar" were the same
    ``ok=True`` at the boundary, and only the free-text ``detail``
    distinguished them -- so a caller that wanted to gate on a real parse
    could not. ``ok`` keeps its meaning for the callers already reading it:
    the edit introduced nothing new that a parser could see.
    """

    language: str
    old_errors: int
    new_errors: int
    ok: bool
    detail: str = ""
    checked: bool = True

    def as_dict(self) -> dict[str, object]:
        """Return the JSON form of this verdict.

        ``checked`` is on the wire because ``ok`` alone cannot answer the
        question a caller gating on a real parse is asking: a file with no
        grammar comes back ``ok=True, checked=False``, which asserts nothing.
        Reading ``ok`` without it reads "nothing looked at this" as "this is
        clean".
        """
        return {
            "language": self.language,
            "old_errors": self.old_errors,
            "new_errors": self.new_errors,
            "ok": self.ok,
            "checked": self.checked,
            "detail": self.detail,
        }


def equivalence_key(changes: Mapping[str, tuple[str, str]], language_of: LanguageOf) -> str:
    """Return the AST-equivalence key for a set of ``path -> (old, new)`` changes.

    Two patches share a key when they make the same structural change to the
    same files, however differently they were written.

    The key carries :data:`TEXT_ONLY_KEY_PREFIX` when any one file in the set
    had no grammar, because the whole key is then only as strong as its
    weakest file.
    """
    digest = hashlib.sha256()
    degraded = False
    for path in sorted(changes):
        old, new = changes[path]
        key = file_key(old, new, language_of(path))
        degraded = degraded or key.startswith(TEXT_ONLY_KEY_PREFIX)
        digest.update(path.encode("utf-8"))
        digest.update(_KEY_SEPARATOR.encode("utf-8"))
        digest.update(key.encode("utf-8"))
        digest.update(b"\n")
    return _label(digest.hexdigest(), grammar=not degraded)


def file_key(old: str, new: str, language: str | None) -> str:
    """Return the equivalence key for one file's change.

    Identical content before and after yields an empty diff and therefore one
    fixed key, which is how a comment-only or whitespace-only edit becomes
    indistinguishable from no edit at all.

    A key built without a grammar carries :data:`TEXT_ONLY_KEY_PREFIX`. The
    fallback stream is whitespace-collapsed raw text, so for any file whose
    meaning depends on whitespace -- YAML being the everyday case -- two
    genuinely different files share a key. Labelling the value is what lets a
    caller that clusters candidates refuse to cluster on this one; without it
    the fallback was documented only in this module's own docstring and the
    hash reaching the vote was indistinguishable from a real AST key.
    """
    before, grammar = _stream_and_grammar(old, language)
    after, _ = _stream_and_grammar(new, language)
    diff = "\n".join(difflib.unified_diff(before.splitlines(), after.splitlines(), lineterm=""))
    return _label(hashlib.sha256(diff.encode("utf-8")).hexdigest(), grammar=grammar)


def normalized_stream(text: str, language: str | None) -> str:
    """Return the normalised token stream of ``text``.

    Every leaf in source order -- operators and keywords included, comments
    excluded -- with an explicit marker around the block types whose
    delimiters the grammar hides, joined by single spaces. Falls back to
    whitespace-collapsed raw text when the language has no usable grammar:
    documented degradation rather than a silent claim of AST equivalence.
    """
    return _stream_and_grammar(text, language)[0]


def _stream_and_grammar(text: str, language: str | None) -> tuple[str, bool]:
    """Return the normalised stream and whether a grammar produced it."""
    parser = _parser_for(language)
    if parser is None:
        return " ".join(text.split()), False

    tree = parser.parse(text.encode("utf-8"))
    blocks = frozenset(INDENT_BLOCK_NODE_TYPES.get(language or "", ()))
    return " ".join(_tokens(tree.root_node, blocks)), True


def _label(digest: str, *, grammar: bool) -> str:
    """Prefix a digest that no grammar stands behind, leaving a real one bare."""
    return digest if grammar else TEXT_ONLY_KEY_PREFIX + digest


def syntax_delta(old: str, new: str, language: str | None) -> SyntaxVerdict:
    """Compare the parse-error count of ``new`` against ``old``.

    ``ok`` when the edit introduced no new ERROR or MISSING nodes. Read it
    with ``checked``: a file whose language has no grammar comes back
    ``ok=True, checked=False``, which asserts nothing about the parse.
    """
    parser = _parser_for(language)
    if parser is None:
        return SyntaxVerdict(
            language=language or "unknown",
            old_errors=0,
            new_errors=0,
            ok=True,
            detail="no grammar for this file: syntax was not checked",
            checked=False,
        )

    old_errors = _error_count(parser.parse(old.encode("utf-8")).root_node)
    new_errors = _error_count(parser.parse(new.encode("utf-8")).root_node)
    ok = new_errors <= old_errors
    detail = ""
    if not ok:
        detail = f"edit introduced {new_errors - old_errors} new parse errors"
    elif old_errors:
        detail = f"file already had {old_errors} parse errors before the edit"

    return SyntaxVerdict(
        language=language or "unknown",
        old_errors=old_errors,
        new_errors=new_errors,
        ok=ok,
        detail=detail,
    )


def _parser_for(language: str | None) -> Parser | None:
    """Return a parser for ``language``, or None when there is no usable one."""
    if not language:
        return None
    try:
        return grammars.get_parser(language)
    except LanguageUnavailable:
        return None


def _tokens(root: Node, blocks: frozenset[str]) -> Iterator[str]:
    """Yield the normalised token stream of one parse tree.

    Iterative rather than recursive: a deeply nested expression in an analysed
    repository must not be able to exhaust this process's stack.
    """
    # Each frame is either a node to visit or a close marker to emit.
    stack: list[Node | str] = [root]
    while stack:
        item = stack.pop()
        if isinstance(item, str):
            yield item
            continue

        if item.type in COMMENT_NODE_TYPES:
            directive = _directive_text(item)
            if directive:
                yield directive
            continue

        if item.child_count == 0:
            # Strict: the tree was parsed from this text's own UTF-8 bytes and
            # node spans fall on codepoint boundaries, so a decode error here
            # would mean the parser lied and should not be papered over.
            text = item.text.decode("utf-8") if item.text else ""
            if text:
                yield text
            continue

        if item.type in blocks:
            yield BLOCK_OPEN
            stack.append(BLOCK_CLOSE)

        stack.extend(reversed(item.children))


def _directive_text(node: Node) -> str:
    """Return one comment's normalised text when it is a directive, else ''.

    Prose is dropped, which is what makes a re-commenting equivalent to no
    change. A directive is kept with its whitespace collapsed, so reflowing
    one is still equivalent and rewriting one is not.
    """
    if node.text is None:
        return ""
    text = " ".join(node.text.decode("utf-8").split())
    body = _COMMENT_MARKER.sub("", text, count=1)
    if not _DIRECTIVE.match(body):
        return ""
    return text


def _error_count(root: Node) -> int:
    """Count ERROR and MISSING nodes, without descending into an ERROR subtree.

    An ERROR node stands for one place the grammar lost the thread; the
    fragments underneath it are consequences of that one failure, not separate
    ones. Counting them separately would make the baseline delta depend on how
    much text followed the mistake.
    """
    count = 0
    stack: list[Node] = [root]
    while stack:
        node = stack.pop()
        if not node.has_error and not node.is_missing:
            continue
        if node.is_missing or node.is_error:
            count += 1
            continue
        stack.extend(node.children)
    return count
