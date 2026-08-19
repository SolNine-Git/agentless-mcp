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
"""

import re
from dataclasses import dataclass
from enum import Enum

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


class SymbolKind(str, Enum):
    """Classification of extracted AST symbols.

    ``str, Enum`` rather than ``StrEnum`` for the 3.10 floor. The mixin gives
    the same equality and JSON behaviour, but ``str()`` and ``format()`` of a
    plain mixin enum differ across 3.10/3.11/3.12, so ``__str__`` is pinned
    here to the member value -- exactly what StrEnum guarantees. Code that
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
LANGUAGE_PREFIXES: dict[str, str] = {
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
}

_QUALNAME_SEPARATOR = "::"


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


def stable_id(language: str, path: str, qualified_name: str) -> str:
    """Build a stable id from its three parts."""
    return f"{language_prefix(language)}:{path}{_QUALNAME_SEPARATOR}{qualified_name}"


def symbol_stable_id(symbol: ASTSymbol) -> str:
    """Build the stable id of ``symbol`` from the file that produced it.

    ``module_path`` is the repository-relative path the scan passed in, so the
    id is portable across checkouts of the same repository.
    """
    return stable_id(symbol.language, symbol.module_path, qualname(symbol))


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
