"""The repository scan and the name index every view is built from.

The extractor answers "what does this file define" and "where is each name
used"; this module is what turns those per-file answers into a repository. A
reference is deliberately a ``(file, name, line)`` triple plus one binding
fact the parse can see -- whether an enclosing parameter list binds the name
locally: resolving a name to the declaration it really binds to needs type
information this tool does not have and will not pretend to have.

Fan-in is therefore fuzzy by construction: the references to a symbol are the
sites that spell its name outside its own definition span. That over-reports
across unrelated files that happen to share a short name -- which is exactly
what the log damping in :mod:`agentless_mcp.core.graph` and the stoplist knob
there exist to absorb -- and it never silently under-reports, which is the
failure mode that matters for a blast-radius question.

:func:`scan_repo` is the one traversal every service shares: walk the
repository once, read each supported file once, and hand back symbols,
imports and references together. Files that cannot be read, are too large, or
whose grammar is not warmed are reported in ``skipped`` with the reason --
never dropped into an answer that then looks complete.
"""

from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path

from agentless_mcp.core.cache import FileSource, effective_source
from agentless_mcp.core.extractor import Ref, TreeSitterExtractor
from agentless_mcp.core.imports import ImportStatement
from agentless_mcp.core.projectconfig import CONFIG_FILENAME
from agentless_mcp.core.symbols import (
    ASTSymbol,
    id_qualname,
    parse_stable_id,
    split_ordinal,
)
from agentless_mcp.core.treewalk import walk_repo
from agentless_mcp.util.errors import LanguageUnavailable
from agentless_mcp.util.fslimits import DEFAULT_MAX_FILE_BYTES, read_bounded


@dataclass(frozen=True)
class FileFacts:
    """Everything one parsed file contributes to the read surface."""

    path: str
    language: str
    line_count: int
    symbols: tuple[ASTSymbol, ...]
    imports: tuple[ImportStatement, ...]
    refs: tuple[Ref, ...]


@dataclass(frozen=True)
class SkippedFile:
    """A file the scan saw but did not parse, and why."""

    path: str
    reason: str


@dataclass(frozen=True)
class RepoScan:
    """One traversal of a repository: what parsed, and what did not."""

    root: Path
    files: tuple[FileFacts, ...]
    skipped: tuple[SkippedFile, ...]

    def by_path(self) -> dict[str, FileFacts]:
        """Index the parsed files by their repository-relative path."""
        return {facts.path: facts for facts in self.files}


@dataclass(frozen=True)
class Definition:
    """A symbol together with the file that defines it."""

    path: str
    symbol: ASTSymbol


@dataclass(frozen=True)
class RefIndex:
    """Name-keyed definitions and reference sites across a whole scan."""

    definitions: Mapping[str, tuple[Definition, ...]]
    sites: Mapping[str, tuple[Ref, ...]]
    files_referencing: Mapping[str, int]

    def defining_paths(self, name: str) -> tuple[str, ...]:
        """The distinct files defining ``name``, in path order."""
        return tuple(sorted({definition.path for definition in self.definitions.get(name, ())}))


def scan_repo(
    root: Path,
    extractor: TreeSitterExtractor,
    *,
    max_file_bytes: int = DEFAULT_MAX_FILE_BYTES,
    source: FileSource | None = None,
) -> RepoScan:
    """Walk ``root`` once and parse every supported file it holds.

    ``source`` is where each file's facts come from: a tag cache when the call
    opened one and the file's digest still matches, on-demand extraction
    otherwise. Symbols, imports and references all travel that seam, so a
    fresh index removes all three parses rather than one of them.
    """
    facts_source = effective_source(source, extractor)
    files: list[FileFacts] = []
    skipped: list[SkippedFile] = []

    for repo_file in walk_repo(root):
        if repo_file.path == CONFIG_FILENAME:
            continue

        language = TreeSitterExtractor.SUPPORTED_EXTENSIONS.get(Path(repo_file.path).suffix)
        if language is None:
            continue

        read = read_bounded(root / repo_file.path, max_bytes=max_file_bytes)
        if read.text is None:
            skipped.append(SkippedFile(path=repo_file.path, reason=read.skipped or "unreadable"))
            continue

        facts = _parse_one(read.text, language, repo_file.path, facts_source)
        if isinstance(facts, SkippedFile):
            skipped.append(facts)
        else:
            files.append(facts)

    return RepoScan(root=root, files=tuple(files), skipped=tuple(skipped))


def build_ref_index(scan: RepoScan) -> RefIndex:
    """Index a scan by name: where each name is defined and where it is used.

    Non-reference occurrences stay in ``sites``: a literal lookup lists every
    place a name is spelled, while consumers that turn a site into a
    *relationship* -- the map's edge weights and the resolver's tiers -- must
    honour the identifier role.
    """
    definitions: dict[str, list[Definition]] = {}
    sites: dict[str, list[Ref]] = {}
    referencing: dict[str, set[str]] = {}

    for facts in scan.files:
        for symbol in facts.symbols:
            definitions.setdefault(symbol.name, []).append(
                Definition(path=facts.path, symbol=symbol)
            )
        for ref in facts.refs:
            sites.setdefault(ref.name, []).append(ref)
            if ref.is_reference:
                referencing.setdefault(ref.name, set()).add(ref.path)

    return RefIndex(
        definitions={name: tuple(values) for name, values in definitions.items()},
        sites={name: tuple(values) for name, values in sites.items()},
        files_referencing={name: len(paths) for name, paths in referencing.items()},
    )


def references_to(index: RefIndex, definition: Definition) -> tuple[Ref, ...]:
    """Return the sites that spell a definition's name from outside its body.

    The definition's own span is excluded, which also removes the declaration
    line itself: the identifier in ``class Invoice`` is a reference site like
    any other, and counting it would make every symbol look used once.
    """
    symbol = definition.symbol
    start = symbol.line_number
    end = span_end(symbol)

    return tuple(
        ref
        for ref in index.sites.get(symbol.name, ())
        if not (ref.path == definition.path and start <= ref.line <= end)
    )


def definitions_for(index: RefIndex, target: str) -> tuple[Definition, ...]:
    """Resolve a lookup target -- a stable id or a bare name -- to definitions.

    One home for what "the symbol the caller meant" means, because fan-in,
    explanation and path all take the same kind of argument and must agree
    about it. A stable id narrows to the definition in its own file and falls
    back to every definition of that name when the file no longer defines it,
    which is what makes an id from a previous generation degrade to a name
    lookup instead of to an empty answer.
    """
    try:
        parsed = parse_stable_id(target)
    except ValueError:
        base, _ = split_ordinal(target)
        name = base.rpartition(".")[2] or base
        return tuple(index.definitions.get(name, ()))

    base, _ = split_ordinal(parsed.qualname)
    name = base.rpartition(".")[2] or base
    scoped = tuple(
        definition
        for definition in index.definitions.get(name, ())
        if definition.path == parsed.path and id_qualname(definition.symbol) == parsed.qualname
    )
    return scoped or tuple(index.definitions.get(name, ()))


def span_end(symbol: ASTSymbol) -> int:
    """Return the last line a symbol covers, its own when the parse gave no end."""
    return symbol.end_line_number if symbol.end_line_number is not None else symbol.line_number


def enclosing_symbol(facts: FileFacts, line: int) -> ASTSymbol | None:
    """Return the innermost symbol whose span contains ``line``.

    Innermost by start line: a method inside a class contains fewer lines and
    starts later, so the deepest containing span is always the one that starts
    last, and two spans starting on the same line go to the one the file
    declared first. Attribution for a reference site is that symbol's
    qualified name.
    """
    containing = [
        symbol for symbol in facts.symbols if symbol.line_number <= line <= span_end(symbol)
    ]
    if not containing:
        return None
    return max(containing, key=lambda symbol: symbol.line_number)


def line_owners(facts: FileFacts) -> dict[int, ASTSymbol]:
    """Map every covered line to the symbol :func:`enclosing_symbol` would name.

    The same rule as the point query, and deliberately next to it so the two
    cannot drift: this is the bulk form, for a caller resolving tens of
    thousands of references against a few hundred spans, where asking line by
    line would rescan every symbol each time.
    """
    owners: dict[int, ASTSymbol] = {}
    for symbol in facts.symbols:
        for line in range(symbol.line_number, span_end(symbol) + 1):
            current = owners.get(line)
            if current is None or current.line_number < symbol.line_number:
                owners[line] = symbol
    return owners


def _parse_one(
    text: str,
    language: str,
    path: str,
    source: FileSource,
) -> FileFacts | SkippedFile:
    """Take one file's three fact sets, degrading that file alone when it cannot be."""
    try:
        defined = source.symbols_for(text, language, path)
        imports = source.imports_for(text, language, path)
        refs = source.refs_for(text, language, path)
    except LanguageUnavailable as exc:
        return SkippedFile(path=path, reason=str(exc))

    return FileFacts(
        path=path,
        language=language,
        line_count=len(text.split("\n")),
        symbols=tuple(defined),
        imports=tuple(imports),
        refs=tuple(refs),
    )
