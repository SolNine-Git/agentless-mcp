"""Symbol domain values: ASTSymbol, SymbolKind and the stable-id vocabulary.

Pure value objects with no external dependencies. ASTSymbol and SymbolKind are
copied from the mcp-local extractor's domain layer so the two stay
behaviourally identical; the only change is the SymbolKind base class, because
this package supports Python 3.10 and ``enum.StrEnum`` arrived in 3.11.

Stable ids live here too, because a stable id is the identity of a symbol and
this is the module that owns what a symbol is. The form is
``<langprefix>:<relpath>::<qualname>`` -- ``py:src/app/svc.py::Invoice.total``
-- chosen so that it reads as a name rather than an opaque handle, survives a
re-index (nothing in it is a row id or a line number), and round-trips: every
id printed by a map or a skeleton parses back into the file and qualified name
that ``expand_symbols`` needs.

An id has to be *unique* for any of that to hold, and a qualified name alone
does not guarantee it. Two C++ overloads, two Ruby methods reopened in one
file, two same-name functions in sibling TypeScript namespaces: each pair
spells one qualified name in one file, so each pair would spell one id, and
`expand_symbols` would hand back the first of them twice. Where the grammar
carries the missing context the extractor uses it -- a Go method's receiver
becomes its parent, so ``ServerInfo.Validate`` and ``AWSConf.Validate`` are
distinct names rather than one name twice. Where it does not,
:func:`disambiguate` is the backstop: within one file, the second and later
symbols sharing a qualified name carry a ``#2``, ``#3`` ordinal in source
order, and :func:`split_ordinal` takes it back off. The ordinal lives on the
*id*, not on :func:`qualname`, so the name a reader sees stays the name the
source spells.
"""

import re
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, replace
from enum import Enum
from types import MappingProxyType

# A signature is an index row, not a rendering of the declaration. Every
# renderer that shows one -- the map, find_symbol cards, expand headers --
# puts it on a single line, so a multi-line `def` that arrives verbatim from
# the source breaks the one-symbol-per-line format and spends budget doing it.
# The invariant lives on the value object rather than in each of the eleven
# construction sites, because a handler added later inherits it for free.
#
# Skeletons are unaffected: `core.skeleton` renders real source text from
# tree-sitter spans and never reads this field.
SIGNATURE_MAX_CHARS = 80

_WHITESPACE_RUN = re.compile(r"\s+")


@dataclass(frozen=True, slots=True)
class Rationale:
    """One design-rationale comment attached to its enclosing symbol."""

    kind: str
    text: str
    line_number: int
    citations: tuple[str, ...] = ()
    duplicate_index: int = 0


class SymbolKind(str, Enum):
    """Classification of extracted AST symbols.

    ``str, Enum`` rather than ``StrEnum`` for the 3.10 floor. The mixin gives
    the same equality and JSON behaviour, but ``format()`` of a plain mixin
    enum differs across 3.10 and 3.11+, and ``str()`` renders the class name
    on every version rather than the value ``StrEnum`` gives, so both are
    pinned here to the member value -- what StrEnum guarantees. Code that
    needs the wire form should still say ``.value``; this override exists so
    an f-string cannot silently emit ``SymbolKind.CLASS`` on one interpreter
    and ``class`` on another.
    """

    CLASS = "class"
    FUNCTION = "function"
    METHOD = "method"
    CONSTANT = "constant"
    TYPE_ALIAS = "type_alias"
    PROTOCOL = "protocol"
    DATACLASS = "dataclass"
    ENUM = "enum"
    DECORATOR = "decorator"

    def __str__(self) -> str:
        """Return the member value, matching ``enum.StrEnum`` semantics."""
        return self.value

    def __format__(self, format_spec: str) -> str:
        """Format the member value, matching ``enum.StrEnum`` semantics."""
        return format(self.value, format_spec)


@dataclass(frozen=True, slots=True)
class ASTSymbol:
    """A single symbol extracted from source code AST."""

    name: str
    kind: SymbolKind
    module_path: str
    line_number: int
    end_line_number: int | None
    signature: str
    docstring: str
    parent_class: str
    decorators: tuple[str, ...]
    bases: tuple[str, ...]
    language: str
    is_public: bool
    is_async: bool
    rationales: tuple[Rationale, ...] = ()
    # How many earlier symbols in the same file already carry this qualified
    # name. Zero for all but a collision, and assigned by
    # :func:`disambiguate` rather than by any extraction handler, so a
    # language added later inherits unique ids without knowing they exist.
    duplicate_index: int = 0

    def __post_init__(self) -> None:
        """Normalise the signature to one line of at most the cap.

        Enforced here so no extraction path can skip it. ``object.__setattr__``
        is the documented way to finish initialising a frozen dataclass; the
        value is immutable to everything downstream, which is the property the
        frozen decorator is protecting.
        """
        normalised = normalise_signature(self.signature)
        if normalised != self.signature:
            object.__setattr__(self, "signature", normalised)


# Short, conventional prefixes: the id is read by a human as often as it is
# parsed by a machine, and `py:` costs five characters less than `python:` on
# every line of every map.
LANGUAGE_PREFIXES: Mapping[str, str] = MappingProxyType(
    {
        "python": "py",
        "javascript": "js",
        "typescript": "ts",
        "tsx": "tsx",
        "go": "go",
        "java": "java",
        "ruby": "rb",
        "rust": "rs",
        "c": "c",
        "cpp": "cpp",
        "lua": "lua",
        "bash": "sh",
        "php": "php",
        "kotlin": "kt",
        "swift": "swift",
        "scala": "scala",
        "csharp": "cs",
        "json": "json",
        "toml": "toml",
        "yaml": "yaml",
        "hcl": "hcl",
        "sql": "sql",
    }
)

_QUALNAME_SEPARATOR = "::"

# The collision ordinal an id carries when a qualified name is not unique
# inside its file. `#` is not a name character in any grammar in the table, so
# a real qualified name can never be mistaken for one that carries an ordinal.
_ORDINAL_MARKER = "#"
_ORDINAL_SUFFIX = re.compile(r"#(\d+)$")


def normalise_signature(signature: str) -> str:
    """Collapse a signature onto one line and cap its length.

    Truncation is marked with an ellipsis so a reader can tell a cut signature
    from a short one; a caller that needs the real declaration asks for the
    body with ``expand_symbols``, which is what the escalation call is for.
    """
    flattened = _WHITESPACE_RUN.sub(" ", signature).strip()
    if len(flattened) > SIGNATURE_MAX_CHARS:
        return flattened[: SIGNATURE_MAX_CHARS - 3] + "..."
    return flattened


@dataclass(frozen=True, slots=True)
class StableId:
    """A parsed stable id: language prefix, repo-relative path, qualified name."""

    prefix: str
    path: str
    qualname: str

    def __str__(self) -> str:
        """Render the id back to its wire form."""
        return f"{self.prefix}:{self.path}{_QUALNAME_SEPARATOR}{self.qualname}"


def language_prefix(language: str) -> str:
    """Return the id prefix for ``language``, the language name if unlisted."""
    return LANGUAGE_PREFIXES.get(language, language)


def qualname(symbol: ASTSymbol) -> str:
    """Return ``Class.method`` for a method, the bare name otherwise."""
    if symbol.parent_class:
        return f"{symbol.parent_class}.{symbol.name}"
    return symbol.name


def id_qualname(symbol: ASTSymbol) -> str:
    """Return the qualified name an id addresses ``symbol`` by.

    :func:`qualname` for everything unique in its file, and that name plus a
    ``#2``/``#3`` ordinal for the second and later symbols that share it. The
    two functions are deliberately different: a card shows the name the source
    spells, an id has to name one symbol.
    """
    return _apply_ordinal(qualname(symbol), symbol.duplicate_index)


def _apply_ordinal(base: str, duplicate_index: int) -> str:
    """Add the collision ordinal to a qualified name, if it needs one."""
    if duplicate_index <= 0:
        return base
    return f"{base}{_ORDINAL_MARKER}{duplicate_index + 1}"


def split_ordinal(qualified_name: str) -> tuple[str, int]:
    """Split a qualified name into its base and its collision ordinal.

    The inverse of :func:`_apply_ordinal`, and the one place a consumer of an
    id is allowed to know the ordinal spelling. A name with no ordinal comes
    back unchanged with index 0, so callers can apply it unconditionally.
    """
    match = _ORDINAL_SUFFIX.search(qualified_name)
    if match is None:
        return qualified_name, 0
    return qualified_name[: match.start()], max(0, int(match.group(1)) - 1)


def disambiguate(symbols: Sequence[ASTSymbol]) -> list[ASTSymbol]:
    """Give every symbol extracted from one file a distinct qualified name.

    The backstop under stable-id uniqueness. Receiver types, impl blocks and
    class bodies supply the missing context wherever a grammar exposes it, but
    overloads, reopened classes and namespaced siblings still collide, and no
    per-language table can promise otherwise for a language added tomorrow.
    So the rule is applied once, over whatever an extraction handler produced:
    the first symbol with a qualified name keeps it, later ones carry an
    ordinal.

    Order is by line, then by extraction order, so the ordinals follow the
    source rather than the traversal -- inserting a method above another one
    moves the ordinals the same way a reader would expect, and a handler that
    changes its traversal order does not silently renumber a repository's ids.
    The returned list preserves the input's order; only the ordinals are new.

    Counted on the qualified name, which is what the id spells. Counting on
    ``(parent_class, name)`` -- the pair the qualified name is *built from* --
    is not the same test, because the pair keeps the dot's position and the
    qualified name does not: a key ``b.c`` under parent ``a`` and a key ``c``
    under parent ``a.b`` are two different pairs and one qualified name.
    Reproduced on four YAML keys, which minted three ids and renumbered
    neither, so two symbols in one file answered to the same id and a caller
    holding it reached whichever the lookup found first.
    """
    counts: dict[str, int] = {}
    indices: dict[int, int] = {}
    for position, symbol in sorted(
        enumerate(symbols), key=lambda pair: (pair[1].line_number, pair[0])
    ):
        key = qualname(symbol)
        seen = counts.get(key, 0)
        counts[key] = seen + 1
        if seen:
            indices[position] = seen

    return [
        replace(symbol, duplicate_index=indices[position]) if position in indices else symbol
        for position, symbol in enumerate(symbols)
    ]


def stable_id(language: str, path: str, qualified_name: str) -> str:
    """Build a stable id from its three parts."""
    return f"{language_prefix(language)}:{path}{_QUALNAME_SEPARATOR}{qualified_name}"


def symbol_stable_id(symbol: ASTSymbol) -> str:
    """Build the stable id of ``symbol`` from the file that produced it.

    ``module_path`` is the repository-relative path the scan passed in, so the
    id is portable across checkouts of the same repository.
    """
    return stable_id(symbol.language, symbol.module_path, id_qualname(symbol))


def rationale_stable_id(symbol: ASTSymbol, rationale: Rationale) -> str:
    """Build a stable id for one rationale node below ``symbol``."""
    suffix = f"{rationale.line_number}"
    if rationale.duplicate_index:
        suffix += f"#{rationale.duplicate_index + 1}"
    return f"{symbol_stable_id(symbol)}::rationale@{suffix}"


def parse_stable_id(text: str) -> StableId:
    """Parse a stable id, raising ValueError on anything that is not one.

    Malformed ids raise rather than resolving to a plausible-looking wrong
    symbol: an id is machine-generated, so a broken one is a bug upstream, not
    user input to be guessed at.
    """
    prefix, colon, remainder = text.partition(":")
    path, separator, qualified_name = remainder.partition(_QUALNAME_SEPARATOR)
    if not (colon and separator and prefix and path and qualified_name):
        message = (
            f"not a stable id: {text!r}; expected <langprefix>:<relpath>::<qualname>, "
            "for example py:src/app/svc.py::Invoice.total"
        )
        raise ValueError(message)
    return StableId(prefix=prefix, path=path, qualname=qualified_name)
