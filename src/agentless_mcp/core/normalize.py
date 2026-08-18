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
change and normalises away reformatting, and the verdict says so instead of
claiming a clean parse nobody performed.
"""

import difflib
import hashlib
from collections.abc import Callable, Iterator, Mapping
from dataclasses import dataclass

from tree_sitter import Node, Parser

from agentless_mcp.core import grammars
from agentless_mcp.core.extractor import COMMENT_NODE_TYPES, INDENT_BLOCK_NODE_TYPES
from agentless_mcp.util.errors import LanguageUnavailable

# What a block boundary looks like in the normalised stream. Two characters no
# source token can be, so they cannot collide with a leaf's own text.
BLOCK_OPEN = "\x01"
BLOCK_CLOSE = "\x02"

# The separator between a path and its per-file key when the patch key is
# built. Included so that the same edit made to two different files does not
# hash to one key -- the plan's "per-file keys concatenated in path-sorted
# order" fixes the order, and naming the path is what makes the order mean
# something.
_KEY_SEPARATOR = "\x00"

LanguageOf = Callable[[str], str | None]


@dataclass(frozen=True)
class SyntaxVerdict:
    """Whether an edited file parses no worse than it did before.

    ``ok`` is a *delta*, not an absolute: ``new_errors <= old_errors``. The
    counts are carried so a caller can tell "clean before, clean after" from
    "broken before, equally broken after", which are the same verdict and very
    different situations.
    """

    language: str
    old_errors: int
    new_errors: int
    ok: bool
    detail: str = ""

    def as_dict(self) -> dict[str, object]:
        """Return the JSON form of this verdict."""
        return {
            "language": self.language,
            "old_errors": self.old_errors,
            "new_errors": self.new_errors,
            "ok": self.ok,
            "detail": self.detail,
        }


def equivalence_key(changes: Mapping[str, tuple[str, str]], language_of: LanguageOf) -> str:
    """Return the AST-equivalence key for a set of ``path -> (old, new)`` changes.

    Two patches share a key when they make the same structural change to the
    same files, however differently they were written.
    """
    digest = hashlib.sha256()
    for path in sorted(changes):
        old, new = changes[path]
        digest.update(path.encode("utf-8"))
        digest.update(_KEY_SEPARATOR.encode("utf-8"))
        digest.update(file_key(old, new, language_of(path)).encode("utf-8"))
        digest.update(b"\n")
    return digest.hexdigest()


def file_key(old: str, new: str, language: str | None) -> str:
    """Return the equivalence key for one file's change.

    Identical content before and after yields an empty diff and therefore one
    fixed key, which is how a comment-only or whitespace-only edit becomes
    indistinguishable from no edit at all.
    """
    before = normalized_stream(old, language)
    after = normalized_stream(new, language)
    diff = "\n".join(difflib.unified_diff(before.splitlines(), after.splitlines(), lineterm=""))
    return hashlib.sha256(diff.encode("utf-8")).hexdigest()


def normalized_stream(text: str, language: str | None) -> str:
    """Return the normalised token stream of ``text``.

    Every leaf in source order -- operators and keywords included, comments
    excluded -- with an explicit marker around the block types whose
    delimiters the grammar hides, joined by single spaces. Falls back to
    whitespace-collapsed raw text when the language has no usable grammar:
    documented degradation rather than a silent claim of AST equivalence.
    """
    parser = _parser_for(language)
    if parser is None:
        return " ".join(text.split())

    tree = parser.parse(text.encode("utf-8"))
    blocks = frozenset(INDENT_BLOCK_NODE_TYPES.get(language or "", ()))
    return " ".join(_tokens(tree.root_node, blocks))


def syntax_delta(old: str, new: str, language: str | None) -> SyntaxVerdict:
    """Compare the parse-error count of ``new`` against ``old``.

    ``ok`` when the edit introduced no new ERROR or MISSING nodes.
    """
    parser = _parser_for(language)
    if parser is None:
        return SyntaxVerdict(
            language=language or "unknown",
            old_errors=0,
            new_errors=0,
            ok=True,
            detail="no grammar for this file: syntax was not checked",
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
            continue

        if item.child_count == 0:
            text = item.text.decode("utf-8", errors="replace") if item.text else ""
            if text:
                yield text
            continue

        if item.type in blocks:
            yield BLOCK_OPEN
            stack.append(BLOCK_CLOSE)

        stack.extend(reversed(item.children))


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
