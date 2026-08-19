"""Deterministic hallucination checks over a parsed patch.

The mechanical half of a hostile first-pass review, LLM-free and run *before*
validation, so that a candidate that imports a package nobody depends on or
re-implements a function the repository already has is named as such instead of
burning a test run to find out.

Seven checks ship here. Three are resolution-independent -- they need the
patch's parsed edits and the repository's already-extracted symbol and import
tables, and nothing more. Four read
:mod:`agentless_mcp.core.resolve`, through the resolver the caller hands over
on :class:`RepoFacts`; this module never opens a cache, a repository or a file
of its own.

``undeclared_imports``
    A top-level package the patch imports that is in neither the repository's
    declared dependencies nor :data:`sys.stdlib_module_names`. This is the
    slopsquatting check: a hallucinated package name looks exactly like a real
    one until something tries to install it.

``shadowing``
    A ``def`` or ``class`` the patch introduces at a module's top level whose
    name already lives there -- excluding the symbol the patch is *replacing*,
    which is what most edits legitimately are.

``near_duplicates``
    A function or method the patch introduces whose normalised token stream
    (the same machinery :mod:`agentless_mcp.core.normalize` keys the vote on)
    equals an existing symbol's. "You already have this at file:line." Exact
    key equality only; no fuzzy similarity, because a similarity threshold is
    a knob nobody can calibrate and every false positive spends a reader's
    attention.

``dangling_references``
    A name the patch *calls*, or names as a base class, that resolves to no
    definition anywhere in the repository, is not a builtin, is not a standard
    library module and is not bound by the patch itself. The hallucinated-helper
    check. Existing names within a small edit distance are offered as near
    misses, because the usual cause is a name half-remembered rather than
    invented.

``dangling_callers``
    A symbol the patch deletes or renames -- present in an edit's search text,
    absent from its replacement -- that files the patch does not touch still
    reference. The other half of the same question: not "does this exist" but
    "did you leave anything pointing at what you removed".

``arity``
    A call in new code whose callee resolves, at the ``same_file`` or
    ``imported`` tier, to exactly one plain function whose signature the
    extractor captured, and whose positional argument count cannot fit that
    signature. Advisory, and deliberately timid: varargs, keyword arguments,
    decorators, methods and every other source of bound-argument ambiguity are
    passed over in silence rather than guessed at.

``cycle_delta``
    Import cycles that exist after the patch and did not exist before it. Both
    graphs are built in memory, from the pre-patch file texts and from those
    same texts with the edits applied -- no worktree, no checkout and nothing
    written anywhere.

**Everything here is advisory.** No finding blocks anything, no function
returns a verdict, and the report has no ``ok`` field to be misread as one.
The tests decide whether a patch is right; this decides what a reviewer should
look at first.

**No check raises.** Degraded input -- a file whose text the caller did not
supply, a language with no dependency manifest, a fragment the grammar cannot
make sense of -- produces a
:data:`Severity.NOT_CHECKED` finding naming the reason. Silence is the one
outcome that would be a lie, because a caller cannot tell "checked and clean"
from "never ran".

Two boundaries worth naming. The edit's replacement text is a *fragment*, not
a file: it is dedented before parsing, and an introduced symbol is treated as
module-level only when its line began at column 1 in the original block, so a
method added to a class is never mistaken for a top-level definition. And a
distribution name is not an import name (``PyYAML`` provides ``yaml``), which
would make the import check noisy; the escape hatch is mechanical rather than
a hand-maintained alias table -- a top-level package already imported somewhere
in the repository is treated as available, because the evidence that it
installs is that the repository already runs.

Two more boundaries belong to the resolution-dependent half. **Only names in
call position and declared base classes are candidates for
``dangling_references``.** A bare identifier read is almost always a local, and
this module cannot see a function's scopes; a check that reported every local
variable as undefined would be a check nobody reads, and turning it off would
be the same outcome with worse manners. **The builtin, keyword and binder
tables are Python's**, so both name-level checks report ``not_checked`` for a
language whose tables are absent rather than judging it by Python's. The two
graph-shaped checks -- ``dangling_callers`` and ``cycle_delta`` -- read the
extractor's own reference and import rows and run for every language it parses.
"""

import builtins
import hashlib
import keyword
import re
import sys
import textwrap
from bisect import bisect_right
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, replace
from enum import Enum
from pathlib import Path
from typing import Any, Protocol

from agentless_mcp.core import resolve
from agentless_mcp.core.extractor import Ref, TreeSitterExtractor
from agentless_mcp.core.graph import resolve_import_target
from agentless_mcp.core.imports import ImportStatement
from agentless_mcp.core.normalize import normalized_stream
from agentless_mcp.core.patches import Edit, apply_edits
from agentless_mcp.core.refs import Definition, FileFacts
from agentless_mcp.core.symbols import ASTSymbol, SymbolKind, qualname
from agentless_mcp.util.errors import AtlasError
from agentless_mcp.util.fslimits import read_bounded

CHECK_UNDECLARED_IMPORTS = "undeclared_imports"
CHECK_SHADOWING = "shadowing"
CHECK_NEAR_DUPLICATES = "near_duplicates"
CHECK_DANGLING_REFERENCES = "dangling_references"
CHECK_DANGLING_CALLERS = "dangling_callers"
CHECK_ARITY = "arity"
CHECK_CYCLE_DELTA = "cycle_delta"

# Gaps that are not any one check's fault: a file in a language this build
# cannot parse leaves all three unable to say anything about it.
CHECK_COVERAGE = "coverage"

# Languages whose declared dependencies this module knows how to read. The
# check is structured per language so another one can be added by teaching
# `read_declared_dependencies` its manifest; a language absent from this set
# is reported "not checked", never quietly passed.
DEPENDENCY_LANGUAGES = frozenset({"python"})

# Languages whose builtin, keyword and binder vocabulary this module knows.
# The name-level checks judge a language only against its own tables; one
# absent from this set is reported "not checked", never judged by Python's.
RESOLUTION_LANGUAGES = frozenset({"python"})

# Names that are always defined and are nobody's missing helper.
_ALWAYS_BOUND: frozenset[str] = frozenset(
    set(dir(builtins)) | set(keyword.kwlist) | set(sys.stdlib_module_names) | {"self", "cls"}
)

# How far a name may be from an existing one and still be offered as the name
# the author meant. Two edits covers a transposition, a doubled letter and a
# dropped one; past that the "suggestion" is a different word.
MAX_EDIT_DISTANCE = 2

# How many near misses one dangling reference offers. Three is a hint; a
# longer list is a search result the reader has to redo the work of.
MAX_NEAR_MISSES = 3

# How many still-referencing sites a dangling-caller finding names before it
# elides. The count in the message is always exact.
MAX_CALLER_SITES = 5

# Below this many normalised tokens a function body says nothing about
# duplication: `return None`, `raise NotImplementedError` and a two-line
# delegation match dozens of existing symbols in any repository, and a check
# that reports them is a check nobody reads.
MIN_BODY_TOKENS = 12

# How many existing sites a near-duplicate finding names before it stops
# listing them. The count is always exact; only the enumeration is bounded.
MAX_DUPLICATE_SITES = 3

PYPROJECT_NAME = "pyproject.toml"
REQUIREMENTS_GLOB = "requirements*.txt"

# Failure modes a check may hit on hostile or merely odd input, each of which
# must degrade that check rather than the whole report. Named explicitly
# instead of catching Exception: an error class not in this list is a defect
# in this module and must surface as one.
DEGRADED_ERRORS: tuple[type[Exception], ...] = (
    AtlasError,
    ValueError,
    TypeError,
    KeyError,
    IndexError,
    AttributeError,
    UnicodeDecodeError,
    RecursionError,
    OSError,
)

_DISTRIBUTION_SEPARATORS = re.compile(r"[-_.]+")
_REQUIREMENT_NAME = re.compile(r"^([A-Za-z0-9][A-Za-z0-9._-]*)")


class Severity(str, Enum):
    """How much attention a finding is asking for.

    ``NOT_CHECKED`` is a statement about coverage rather than about the patch:
    it says a check did not run and why. It is a member of this enum, not a
    separate channel, so that a caller rendering findings cannot render the
    findings and drop the gaps.

    ``str, Enum`` rather than ``StrEnum`` for the 3.10 floor, matching
    :class:`agentless_mcp.core.symbols.SymbolKind`.
    """

    ADVISORY = "advisory"
    WARNING = "warning"
    NOT_CHECKED = "not_checked"

    def __str__(self) -> str:
        """Return the member value, matching ``enum.StrEnum`` semantics."""
        return self.value


@dataclass(frozen=True)
class Finding:
    """One thing a reviewer should look at, and where to look.

    ``line`` is 1-based and 0 means "this file, line unknown" -- which happens
    when an edit's search text does not occur exactly once in the pre-patch
    file, so the block cannot be anchored. ``path`` is empty for a finding
    about the repository as a whole rather than about one file.
    """

    check: str
    severity: Severity
    message: str
    path: str
    line: int
    evidence: str

    @property
    def location(self) -> str:
        """Render the ``file:line`` this finding points at."""
        if not self.path:
            return "(repository)"
        if self.line <= 0:
            return self.path
        return f"{self.path}:{self.line}"

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this finding."""
        return {
            "check": self.check,
            "severity": self.severity.value,
            "message": self.message,
            "path": self.path,
            "line": self.line,
            "location": self.location,
            "evidence": self.evidence,
        }


@dataclass(frozen=True)
class LintReport:
    """Every finding one patch produced, in a fixed order.

    Deliberately without a boolean: this report never says whether to proceed.
    """

    findings: tuple[Finding, ...]

    def of_severity(self, severity: Severity) -> tuple[Finding, ...]:
        """The findings at one severity, in report order."""
        return tuple(finding for finding in self.findings if finding.severity is severity)

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this report."""
        return {"findings": [finding.as_dict() for finding in self.findings]}


@dataclass(frozen=True)
class DeclaredDependencies:
    """What a repository says it depends on, and where that was read from.

    ``packages`` holds PEP-503-normalised distribution names.
    ``sources`` is empty when no manifest was found at all, which is a
    different situation from a manifest that declares nothing: the first means
    the check cannot run, the second means every third-party import is
    genuinely undeclared.
    """

    packages: frozenset[str]
    sources: tuple[str, ...]
    warnings: tuple[str, ...]

    @property
    def known(self) -> bool:
        """True when at least one dependency manifest was read."""
        return bool(self.sources)


@dataclass(frozen=True)
class RepoFacts:
    """The pre-patch repository the checks compare a patch against.

    ``files`` is the symbol/import table a scan already produced and ``texts``
    the source those facts came from, both keyed by repository-relative path.
    They are separate because a caller may hold facts for a whole repository
    and text for only part of it; the near-duplicate check says how much of
    the repository it could actually see rather than pretending to have read
    all of it.

    ``resolver`` is the Phase 6 resolver over those same files, built by the
    caller and passed in. It arrives as a value rather than being constructed
    here for the same reason the rest of this does: a check that built its own
    view of the repository could disagree with the one the caller is reporting
    against, and this module must not be able to open anything.
    """

    files: Mapping[str, FileFacts]
    texts: Mapping[str, str]
    dependencies: DeclaredDependencies
    resolver: resolve.Resolver


class FragmentSource(Protocol):
    """Where a fragment's symbols and imports are parsed.

    Structurally satisfied by the package's existing per-file fact sources and
    by the extractor itself. A protocol rather than a concrete type because
    this module must not know whether a caller has a tag cache open.
    """

    def symbols_for(self, text: str, language: str, path: str) -> list[ASTSymbol]:
        """Return the symbols ``text`` defines."""
        ...

    def imports_for(self, text: str, language: str, path: str) -> list[ImportStatement]:
        """Return the imports ``text`` declares."""
        ...

    def refs_for(self, text: str, language: str, path: str) -> list[Ref]:
        """Return the identifier occurrences in ``text``."""
        ...


@dataclass(frozen=True)
class _CallSite:
    """One call the replacement block makes, as text positions.

    ``offset`` points at the opening parenthesis, which is where the argument
    scan starts; ``line`` is 1-based within the dedented fragment. Both are
    carried because the two checks that read call sites want different things
    -- one wants somewhere to point a reader, the other wants somewhere to
    start parsing.
    """

    name: str
    line: int
    offset: int


@dataclass(frozen=True)
class _Fragment:
    """One edit reduced to what the checks read.

    ``search_span`` and ``base_line`` are the anchor of the block in the
    pre-patch file, 0 when the search text is absent or occurs more than once.
    ``replace_columns`` keeps the *original* block's lines so that an
    introduced symbol's indentation survives the dedent that made the fragment
    parseable.
    """

    edit: Edit
    language: str
    replace_text: str
    replace_columns: tuple[str, ...]
    base_line: int
    search_span: tuple[int, int]
    introduced: tuple[ASTSymbol, ...]
    replaced_names: frozenset[str]
    imports: tuple[ImportStatement, ...]
    refs: tuple[Ref, ...]
    calls: tuple[_CallSite, ...]

    def file_line(self, fragment_line: int) -> int:
        """Map a line in the replacement block to a line in the file."""
        if self.base_line <= 0 or fragment_line <= 0:
            return 0
        return self.base_line + fragment_line - 1


# ---------------------------------------------------------------------------
# Declared dependencies
# ---------------------------------------------------------------------------

if sys.version_info >= (3, 11):
    import tomllib

    def _load_toml(text: str) -> dict[str, Any]:
        """Parse TOML with the standard library parser."""
        return tomllib.loads(text)

else:

    def _load_toml(text: str) -> dict[str, Any]:
        """Parse the subset of TOML this check needs.

        ``tomllib`` arrived in 3.11 and this package's floor is 3.10, so the
        older interpreter gets a scanner rather than a dependency: the plan
        for this repository put the whole configuration surface on stdlib
        json precisely to avoid taking one, and adding ``tomli`` for a
        best-effort advisory would invert that.

        What it understands is exactly the shape a dependency declaration
        has: an array of strings assigned to a key inside ``[project]``,
        ``[project.optional-dependencies]`` or ``[dependency-groups]``,
        possibly spanning lines. What it does not understand -- inline
        tables, multi-line basic strings, a ``#`` inside a requirement
        string -- either yields nothing for that key or, at worst, an extra
        name in the declared set, which can only make this check quieter
        and never make it accuse a real dependency.
        """
        return _scan_toml(text)


_TABLE_HEADER = re.compile(r"^\[\[?([^\]]+)\]\]?\s*$")
_ARRAY_ASSIGNMENT = re.compile(r'^\s*(?:"([^"]+)"|([A-Za-z0-9_.-]+))\s*=\s*\[(.*)$')
_QUOTED_STRING = re.compile(r'"([^"]*)"|\'([^\']*)\'')

_PROJECT = "project"
_OPTIONAL = "optional-dependencies"
_DEPENDENCIES = "dependencies"
_DEPENDENCY_GROUPS = "dependency-groups"


def _scan_toml(text: str) -> dict[str, Any]:
    """Scan the dependency-bearing tables of a pyproject document."""
    tables: dict[str, dict[str, list[str]]] = {}
    table = ""
    pending_key = ""
    pending: list[str] = []
    depth = 0

    for raw in text.splitlines():
        if depth > 0:
            depth += raw.count("[") - raw.count("]")
            pending.append(raw)
            if depth <= 0:
                tables.setdefault(table, {})[pending_key] = _quoted_strings("\n".join(pending))
                pending = []
            continue

        line = raw.split("#", 1)[0].rstrip() if raw.lstrip().startswith("#") else raw.rstrip()
        header = _TABLE_HEADER.match(line.strip())
        if header:
            table = header.group(1).strip()
            continue

        assignment = _ARRAY_ASSIGNMENT.match(line)
        if assignment:
            pending_key = assignment.group(1) or assignment.group(2)
            rest = assignment.group(3)
            depth = 1 + rest.count("[") - rest.count("]")
            pending = [rest]
            if depth <= 0:
                tables.setdefault(table, {})[pending_key] = _quoted_strings(rest)
                pending = []

    return _shape(tables)


def _quoted_strings(text: str) -> list[str]:
    """Return every single- or double-quoted string in ``text``, in order."""
    return [double or single for double, single in _QUOTED_STRING.findall(text)]


def _shape(tables: Mapping[str, Mapping[str, list[str]]]) -> dict[str, Any]:
    """Reshape scanned tables into the document layout ``tomllib`` returns."""
    project: dict[str, Any] = {}
    if _DEPENDENCIES in tables.get(_PROJECT, {}):
        project[_DEPENDENCIES] = list(tables[_PROJECT][_DEPENDENCIES])
    optional = tables.get(f"{_PROJECT}.{_OPTIONAL}")
    if optional:
        project[_OPTIONAL] = {key: list(value) for key, value in optional.items()}

    document: dict[str, Any] = {}
    if project:
        document[_PROJECT] = project
    groups = tables.get(_DEPENDENCY_GROUPS)
    if groups:
        document[_DEPENDENCY_GROUPS] = {key: list(value) for key, value in groups.items()}
    return document


def read_declared_dependencies(root: Path) -> DeclaredDependencies:
    """Read ``root``'s dependency manifests.

    ``pyproject.toml`` and every ``requirements*.txt`` at the repository root,
    read through the same bounded reader every other file access in this
    package uses. A manifest that cannot be parsed produces a warning and
    contributes nothing, rather than being treated as an empty one -- an empty
    declared set would make every third-party import in the patch look
    hallucinated.
    """
    packages: set[str] = set()
    sources: list[str] = []
    warnings: list[str] = []

    pyproject = root / PYPROJECT_NAME
    if pyproject.is_file():
        read = read_bounded(pyproject)
        if read.text is None:
            warnings.append(f"{PYPROJECT_NAME} not read: {read.skipped}")
        else:
            found, problems = parse_pyproject_dependencies(read.text)
            packages.update(found)
            warnings.extend(problems)
            sources.append(PYPROJECT_NAME)

    for requirements in sorted(root.glob(REQUIREMENTS_GLOB)):
        if not requirements.is_file():
            continue
        read = read_bounded(requirements)
        if read.text is None:
            warnings.append(f"{requirements.name} not read: {read.skipped}")
            continue
        packages.update(parse_requirements(read.text))
        sources.append(requirements.name)

    return DeclaredDependencies(
        packages=frozenset(packages),
        sources=tuple(sources),
        warnings=tuple(warnings),
    )


def parse_pyproject_dependencies(text: str) -> tuple[frozenset[str], tuple[str, ...]]:
    """Return the normalised distribution names a pyproject document declares.

    ``[project] dependencies``, every list under
    ``[project.optional-dependencies]``, and every list under PEP 735
    ``[dependency-groups]``. The groups are included deliberately: a patch to
    a test file importing a dev-only package is declared, and leaving them out
    would make the check fire on exactly the code it should not.
    """
    warnings: list[str] = []
    try:
        document = _load_toml(text)
    except DEGRADED_ERRORS as exc:
        return frozenset(), (f"{PYPROJECT_NAME} did not parse: {type(exc).__name__}",)

    specifications: list[str] = []
    project = document.get(_PROJECT)
    if isinstance(project, dict):
        specifications.extend(_string_list(project.get(_DEPENDENCIES), warnings, "dependencies"))
        specifications.extend(_string_lists(project.get(_OPTIONAL), warnings, _OPTIONAL))
    specifications.extend(
        _string_lists(document.get(_DEPENDENCY_GROUPS), warnings, _DEPENDENCY_GROUPS)
    )

    names = {name for name in (requirement_name(item) for item in specifications) if name}
    return frozenset(names), tuple(warnings)


def _string_list(value: object, warnings: list[str], where: str) -> list[str]:
    """Take a list of strings from foreign data, reporting any other shape."""
    if value is None:
        return []
    if not isinstance(value, list):
        warnings.append(f"{where} is not a list; ignored")
        return []
    strings = [item for item in value if isinstance(item, str)]
    if len(strings) != len(value):
        warnings.append(f"{where} holds {len(value) - len(strings)} non-string entries; ignored")
    return strings


def _string_lists(value: object, warnings: list[str], where: str) -> list[str]:
    """Flatten a table of string lists, reporting any other shape."""
    if value is None:
        return []
    if not isinstance(value, dict):
        warnings.append(f"{where} is not a table; ignored")
        return []
    flattened: list[str] = []
    for key in sorted(value):
        flattened.extend(_string_list(value[key], warnings, f"{where}.{key}"))
    return flattened


def parse_requirements(text: str) -> frozenset[str]:
    """Return the normalised distribution names a requirements file declares.

    Option lines (``-r``, ``--index-url``, ``-e``) are skipped rather than
    followed: following a ``-r`` would mean reading a path out of repository
    content, and this check is not worth that.
    """
    names: set[str] = set()
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line or line.startswith("-"):
            continue
        name = requirement_name(line)
        if name:
            names.add(name)
    return frozenset(names)


def requirement_name(specification: str) -> str:
    """Return the normalised distribution name a requirement string names.

    Everything after an extras bracket, a version specifier, an environment
    marker or a direct-reference ``@`` is the requirement's *constraints*, not
    its identity.
    """
    head = specification.split(";", 1)[0].split("@", 1)[0].strip()
    match = _REQUIREMENT_NAME.match(head)
    if not match:
        return ""
    return normalize_distribution(match.group(1))


def normalize_distribution(name: str) -> str:
    """Normalise a distribution or module name per PEP 503.

    This is also what maps an import name onto a distribution name in the
    common case: ``tree_sitter`` and ``tree-sitter`` both normalise to
    ``tree-sitter``.
    """
    return _DISTRIBUTION_SEPARATORS.sub("-", name).lower()


# ---------------------------------------------------------------------------
# The checks
# ---------------------------------------------------------------------------


def lint_patch(
    edits: Sequence[Edit],
    facts: RepoFacts,
    source: FragmentSource,
) -> LintReport:
    """Run every check over one patch's parsed edits.

    Findings come back in a fixed order -- check, then path, then line, then
    text -- so two runs over the same patch produce the same report and a
    caller may diff one against another.
    """
    fragments, notes = _fragments(edits, facts, source)
    findings = list(notes)
    findings.extend(
        _guarded(CHECK_UNDECLARED_IMPORTS, lambda: _undeclared_imports(fragments, facts))
    )
    findings.extend(_guarded(CHECK_SHADOWING, lambda: _shadowing(fragments, facts)))
    findings.extend(_guarded(CHECK_NEAR_DUPLICATES, lambda: _near_duplicates(fragments, facts)))
    findings.extend(
        _guarded(CHECK_DANGLING_REFERENCES, lambda: _dangling_references(fragments, facts))
    )
    findings.extend(_guarded(CHECK_DANGLING_CALLERS, lambda: _dangling_callers(fragments, facts)))
    findings.extend(_guarded(CHECK_ARITY, lambda: _arity(fragments, facts)))
    findings.extend(_guarded(CHECK_CYCLE_DELTA, lambda: _cycle_delta(fragments, facts, source)))
    return LintReport(findings=tuple(sorted(findings, key=_order)))


def _order(finding: Finding) -> tuple[str, str, int, str, str]:
    """The total order findings are reported in."""
    return (finding.check, finding.path, finding.line, finding.message, finding.evidence)


def _guarded(check: str, run: Callable[[], list[Finding]]) -> list[Finding]:
    """Run one check, turning a degraded input into a reported gap."""
    try:
        return run()
    except DEGRADED_ERRORS as exc:
        return [
            _gap(
                check,
                "",
                f"not checked: the check could not run over this patch ({type(exc).__name__})",
                str(exc),
            )
        ]


def _gap(check: str, path: str, message: str, evidence: str) -> Finding:
    """Build a NOT_CHECKED finding: a statement about coverage, not the patch."""
    return Finding(
        check=check,
        severity=Severity.NOT_CHECKED,
        message=message,
        path=path,
        line=0,
        evidence=evidence,
    )


def _fragments(
    edits: Sequence[Edit],
    facts: RepoFacts,
    source: FragmentSource,
) -> tuple[tuple[_Fragment, ...], list[Finding]]:
    """Parse every edit into the fragment the checks read, reporting gaps."""
    fragments: list[_Fragment] = []
    unsupported: dict[str, int] = {}
    notes: list[Finding] = []

    for edit in edits:
        language = TreeSitterExtractor.SUPPORTED_EXTENSIONS.get(Path(edit.path).suffix)
        if language is None:
            suffix = Path(edit.path).suffix or "(no extension)"
            unsupported[suffix] = unsupported.get(suffix, 0) + 1
            continue
        try:
            fragments.append(_fragment(edit, language, facts, source))
        except DEGRADED_ERRORS as exc:
            notes.append(
                _gap(
                    CHECK_COVERAGE,
                    edit.path,
                    "not checked: this edit's replacement block could not be parsed",
                    f"{type(exc).__name__}: {exc}",
                )
            )

    notes.extend(
        _gap(
            CHECK_COVERAGE,
            "",
            f"not checked: {count} edit(s) to {suffix} files, which this build has no grammar for",
            f"suffix {suffix}",
        )
        for suffix, count in sorted(unsupported.items())
    )
    return tuple(fragments), notes


def _fragment(
    edit: Edit,
    language: str,
    facts: RepoFacts,
    source: FragmentSource,
) -> _Fragment:
    """Parse one edit's two sides into the fragment the checks read."""
    replace_text = textwrap.dedent(edit.replace)
    search_text = textwrap.dedent(edit.search)
    span = _locate(facts.texts.get(edit.path, ""), edit.search)
    introduced = tuple(source.symbols_for(replace_text, language, edit.path))
    references = tuple(source.refs_for(replace_text, language, edit.path))
    return _Fragment(
        edit=edit,
        language=language,
        replace_text=replace_text,
        replace_columns=tuple(edit.replace.split("\n")),
        base_line=span[0],
        search_span=span,
        introduced=introduced,
        replaced_names=frozenset(
            symbol.name for symbol in source.symbols_for(search_text, language, edit.path)
        ),
        imports=tuple(source.imports_for(replace_text, language, edit.path)),
        refs=references,
        calls=_call_sites(replace_text, references, introduced),
    )


def _locate(text: str, search: str) -> tuple[int, int]:
    """Return the 1-based line span ``search`` occupies in ``text``.

    ``(0, 0)`` when the search text is absent or occurs more than once, which
    is the same whole-line, ambiguity-refusing rule
    :mod:`agentless_mcp.core.patches` applies -- an anchor that could be one of
    two places is not an anchor.
    """
    if not text or not search:
        return (0, 0)
    padded = "\n" + text + "\n"
    needle = "\n" + search + "\n"
    first = padded.find(needle)
    if first < 0 or padded.find(needle, first + 1) >= 0:
        return (0, 0)
    start = padded.count("\n", 0, first) + 1
    return (start, start + search.count("\n"))


def _is_module_level(fragment: _Fragment, symbol: ASTSymbol) -> bool:
    """True when this symbol's declaration began at column 1 in the block.

    The fragment was dedented so it would parse, which moves a method's
    ``def`` to column 1 and would make it look like a module-level function.
    The original block's own indentation is what actually answers the
    question, and ``textwrap.dedent`` preserves line numbering, so the two
    line up.
    """
    line = symbol.line_number
    if not 1 <= line <= len(fragment.replace_columns):
        return False
    text = fragment.replace_columns[line - 1]
    return bool(text) and not text[:1].isspace()


def _undeclared_imports(fragments: Sequence[_Fragment], facts: RepoFacts) -> list[Finding]:
    """Report top-level packages the patch imports that nothing declares."""
    findings: list[Finding] = []
    languages = {fragment.language for fragment in fragments}
    findings.extend(
        _gap(
            CHECK_UNDECLARED_IMPORTS,
            "",
            f"not checked: no declared-dependency manifest is known for {language}",
            f"language {language}",
        )
        for language in sorted(languages - DEPENDENCY_LANGUAGES)
    )

    checkable = [fragment for fragment in fragments if fragment.language in DEPENDENCY_LANGUAGES]
    if not checkable:
        return findings
    if not facts.dependencies.known:
        findings.append(
            _gap(
                CHECK_UNDECLARED_IMPORTS,
                "",
                "not checked: the repository has no pyproject.toml or requirements file",
                "no dependency manifest found",
            )
        )
        return findings

    available = _available_packages(facts)
    known_paths = sorted(facts.files)
    seen: set[tuple[str, str]] = set()
    for fragment in checkable:
        for statement in fragment.imports:
            top = _top_level_package(statement)
            if not top or normalize_distribution(top) in available:
                continue
            if resolve_import_target(fragment.edit.path, statement, known_paths) is not None:
                continue
            if (fragment.edit.path, top) in seen:
                continue
            seen.add((fragment.edit.path, top))
            findings.append(_undeclared_finding(fragment, statement, top))
    return findings


def _undeclared_finding(fragment: _Fragment, statement: ImportStatement, top: str) -> Finding:
    """Build the finding for one undeclared top-level package."""
    return Finding(
        check=CHECK_UNDECLARED_IMPORTS,
        severity=Severity.WARNING,
        message=(
            f"the patch imports {top!r}, which is neither a declared dependency, "
            "nor part of the standard library, nor imported anywhere else in this repository"
        ),
        path=fragment.edit.path,
        line=fragment.file_line(statement.line_number),
        evidence=f"import of module {statement.module!r}",
    )


def _top_level_package(statement: ImportStatement) -> str:
    """Return the distributed package an import names, or '' when there is none.

    Relative imports, path-shaped module strings and anything the standard
    library provides are all "there is none": none of them can be a
    hallucinated package.
    """
    module = statement.module.strip()
    if statement.is_relative or not module or module[0] in "./":
        return ""
    top = module.split(".", 1)[0].split("/", 1)[0]
    if not top or top in sys.stdlib_module_names:
        return ""
    return top


def _available_packages(facts: RepoFacts) -> frozenset[str]:
    """Every package name the repository has evidence of being able to import.

    The declared distributions, plus every top-level package already imported
    somewhere in the repository. The second half is what absorbs the
    distribution-name/import-name mismatch (``PyYAML`` provides ``yaml``)
    without a hand-maintained alias table that would drift.
    """
    imported = {
        normalize_distribution(top)
        for file_facts in facts.files.values()
        for statement in file_facts.imports
        for top in (_top_level_package(statement),)
        if top
    }
    return frozenset(facts.dependencies.packages | imported)


def _shadowing(fragments: Sequence[_Fragment], facts: RepoFacts) -> list[Finding]:
    """Report module-level definitions the patch adds over an existing name."""
    findings: list[Finding] = []
    missing = sorted({fragment.edit.path for fragment in fragments} - set(facts.files))
    findings.extend(
        _gap(
            CHECK_SHADOWING,
            path,
            "not checked: no symbol table was supplied for this file",
            "file absent from the scan",
        )
        for path in missing
    )

    for fragment in fragments:
        existing = _module_level_symbols(facts.files.get(fragment.edit.path))
        for symbol in fragment.introduced:
            if symbol.name in fragment.replaced_names or not _is_module_level(fragment, symbol):
                continue
            previous = existing.get(symbol.name)
            if previous is None:
                continue
            findings.append(_shadowing_finding(fragment, symbol, previous))
    return findings


def _shadowing_finding(fragment: _Fragment, symbol: ASTSymbol, previous: ASTSymbol) -> Finding:
    """Build the finding for one module-level name defined twice."""
    return Finding(
        check=CHECK_SHADOWING,
        severity=Severity.WARNING,
        message=(
            f"the patch adds a module-level {symbol.kind} named {symbol.name!r} to a file "
            f"that already defines that name; the later definition wins at import time"
        ),
        path=fragment.edit.path,
        line=fragment.file_line(symbol.line_number),
        evidence=f"existing {previous.kind} at {fragment.edit.path}:{previous.line_number}",
    )


def _module_level_symbols(file_facts: FileFacts | None) -> dict[str, ASTSymbol]:
    """Index a file's top-level symbols by name, first definition winning."""
    if file_facts is None:
        return {}
    indexed: dict[str, ASTSymbol] = {}
    for symbol in file_facts.symbols:
        if not symbol.parent_class:
            indexed.setdefault(symbol.name, symbol)
    return indexed


@dataclass(frozen=True)
class _Site:
    """One existing definition a normalised body key points back at."""

    path: str
    line: int
    name: str


def _near_duplicates(fragments: Sequence[_Fragment], facts: RepoFacts) -> list[Finding]:
    """Report introduced functions whose body already exists in the repository."""
    findings: list[Finding] = []
    unread = sorted(set(facts.files) - set(facts.texts))
    if unread:
        findings.append(
            _gap(
                CHECK_NEAR_DUPLICATES,
                "",
                f"partially checked: {len(unread)} of {len(facts.files)} scanned files had no "
                "text supplied, so their bodies were not compared",
                f"first unread: {unread[0]}",
            )
        )

    index = _existing_bodies(facts)
    for fragment in fragments:
        lines = fragment.replace_text.split("\n")
        for symbol in fragment.introduced:
            if symbol.kind not in _BODY_KINDS:
                continue
            key = _body_key(lines, symbol, fragment.language)
            if key is None:
                continue
            sites = [
                site for site in index.get(key, ()) if not _is_the_replaced_site(fragment, site)
            ]
            if sites:
                findings.append(_duplicate_finding(fragment, symbol, sites))
    return findings


_BODY_KINDS = frozenset({SymbolKind.FUNCTION, SymbolKind.METHOD})


def _is_the_replaced_site(fragment: _Fragment, site: _Site) -> bool:
    """True when this existing definition is the one the patch is replacing."""
    if site.path != fragment.edit.path:
        return False
    start, end = fragment.search_span
    if start > 0 and start <= site.line <= end:
        return True
    return site.name.split(".")[-1] in fragment.replaced_names


def _duplicate_finding(fragment: _Fragment, symbol: ASTSymbol, sites: Sequence[_Site]) -> Finding:
    """Build the finding for one introduced body that already exists."""
    listed = ", ".join(
        f"{site.path}:{site.line} ({site.name})" for site in sites[:MAX_DUPLICATE_SITES]
    )
    more = (
        "" if len(sites) <= MAX_DUPLICATE_SITES else f" and {len(sites) - MAX_DUPLICATE_SITES} more"
    )
    return Finding(
        check=CHECK_NEAR_DUPLICATES,
        severity=Severity.ADVISORY,
        message=(
            f"the body the patch gives {symbol.name!r} normalises to the same token stream as "
            f"{len(sites)} existing definition(s); you may already have this"
        ),
        path=fragment.edit.path,
        line=fragment.file_line(symbol.line_number),
        evidence=f"already at {listed}{more}",
    )


def _existing_bodies(facts: RepoFacts) -> dict[str, tuple[_Site, ...]]:
    """Index every readable function body in the repository by its key."""
    index: dict[str, list[_Site]] = {}
    for path in sorted(facts.files):
        text = facts.texts.get(path)
        file_facts = facts.files[path]
        if text is None:
            continue
        lines = text.split("\n")
        for symbol in file_facts.symbols:
            if symbol.kind not in _BODY_KINDS:
                continue
            key = _body_key(lines, symbol, file_facts.language)
            if key is None:
                continue
            index.setdefault(key, []).append(
                _Site(path=path, line=symbol.line_number, name=qualname(symbol))
            )
    return {
        key: tuple(sorted(sites, key=lambda site: (site.path, site.line)))
        for key, sites in index.items()
    }


def _body_key(lines: Sequence[str], symbol: ASTSymbol, language: str) -> str | None:
    """Return the equivalence key of one symbol's body, or None when it says nothing.

    The declaration line is excluded, so a copied function renamed on the way
    in still matches. Bodies shorter than :data:`MIN_BODY_TOKENS` normalised
    tokens are not keyed at all: below that length equality is a coincidence,
    not a duplicate.
    """
    end = symbol.end_line_number
    if end is None or end <= symbol.line_number or symbol.line_number >= len(lines):
        return None
    body = textwrap.dedent("\n".join(lines[symbol.line_number : end]))
    stream = normalized_stream(body, language)
    if len(stream.split()) < MIN_BODY_TOKENS:
        return None
    return hashlib.sha256(stream.encode("utf-8")).hexdigest()


# ---------------------------------------------------------------------------
# Call sites: the one place a "this patch calls X" fact is produced
# ---------------------------------------------------------------------------

_CALL = re.compile(r"(?<![\w.])([A-Za-z_][A-Za-z0-9_]*)\s*\(")

_QUOTES = "\"'"
_TRIPLE_QUOTES = frozenset({'"""', "'''"})
_OPENING = "([{"
_CLOSING = ")]}"
_COMMENT = "#"

# The depth recorded for a character that is inside a string literal or a
# comment. Negative so that no structural comparison can ever match it.
_INSIDE_STRING = -1

_ASSIGNED = re.compile(r"^\s*([A-Za-z_]\w*(?:\s*,\s*[A-Za-z_]\w*)*)\s*(?::[^=]+)?=(?!=)")
_LOOP_TARGET = re.compile(r"\bfor\s+([A-Za-z_]\w*(?:\s*,\s*[A-Za-z_]\w*)*)\s+in\b")
_ALIAS_TARGET = re.compile(r"\bas\s+([A-Za-z_]\w*)")
_WALRUS_TARGET = re.compile(r"([A-Za-z_]\w*)\s*:=")

_ARITY_TIERS = frozenset({resolve.Tier.SAME_FILE, resolve.Tier.IMPORTED})

_VERB_CALLS = "calls"
_VERB_INHERITS = "inherits from"


@dataclass(frozen=True)
class _Usage:
    """One place the patch names something it did not define: name and verb."""

    name: str
    line: int
    verb: str


@dataclass(frozen=True)
class _Shape:
    """What a captured signature says about how many arguments it accepts."""

    required: int
    total: int


def _call_sites(
    text: str,
    references: Sequence[Ref],
    introduced: Sequence[ASTSymbol],
) -> tuple[_CallSite, ...]:
    """Return the calls ``text`` makes, cross-checked against the parsed refs.

    The pattern finds ``name(``; the reference table decides whether that text
    was code. A name inside a string literal or a comment is not an identifier
    the grammar reported, so it never becomes a call site -- which is what
    keeps a docstring showing example usage out of every check downstream. A
    declaration is excluded on the same ``(name, line)`` rule
    :func:`agentless_mcp.core.resolve.build_graph` uses: ``def quote(`` names
    the function, it does not call it.
    """
    coded = {(ref.name, ref.line) for ref in references}
    declared = {(symbol.name, symbol.line_number) for symbol in introduced}
    starts = _line_starts(text)

    sites: list[_CallSite] = []
    for match in _CALL.finditer(text):
        name = match.group(1)
        line = _line_of(starts, match.start(1))
        if (name, line) not in coded or (name, line) in declared:
            continue
        sites.append(_CallSite(name=name, line=line, offset=match.end() - 1))
    return tuple(sites)


def _line_starts(text: str) -> tuple[int, ...]:
    """Return the offset each 1-based line begins at."""
    offsets = [0]
    for index, char in enumerate(text):
        if char == "\n":
            offsets.append(index + 1)
    return tuple(offsets)


def _line_of(starts: Sequence[int], offset: int) -> int:
    """Return the 1-based line number one offset falls on."""
    line = bisect_right(starts, offset)
    return max(1, line)


def _structure(text: str) -> list[int]:
    """Return the bracket depth of every character, strings and comments apart.

    An opening bracket carries the depth *inside* it and a closing bracket the
    depth it closes, so the two ends of one pair share a number. Everything
    inside a string literal or a comment is :data:`_INSIDE_STRING`, which is
    what stops a bracket in a docstring from unbalancing the count.
    """
    depths = [0] * len(text)
    depth = 0
    index = 0

    while index < len(text):
        char = text[index]
        if char in _QUOTES or char == _COMMENT:
            end = _literal_end(text, index)
            for position in range(index, end):
                depths[position] = _INSIDE_STRING
            index = end
            continue
        if char in _OPENING:
            depth += 1
            depths[index] = depth
        elif char in _CLOSING:
            depths[index] = depth
            depth -= 1
        else:
            depths[index] = depth
        index += 1

    return depths


def _literal_end(text: str, start: int) -> int:
    """Return the index just past the string or comment beginning at ``start``."""
    if text[start] == _COMMENT:
        end = text.find("\n", start)
        return len(text) if end < 0 else end

    marker = text[start : start + 3]
    if marker in _TRIPLE_QUOTES:
        found = text.find(marker, start + 3)
        return len(text) if found < 0 else found + 3

    quote = text[start]
    index = start + 1
    while index < len(text):
        char = text[index]
        if char == "\\":
            index += 2
            continue
        if char in (quote, "\n"):
            return index + 1
        index += 1
    return len(text)


def _inner_span(text: str, offset: int) -> str | None:
    """Return the text between the bracket at ``offset`` and its partner."""
    depths = _structure(text)
    if offset >= len(depths) or depths[offset] == _INSIDE_STRING:
        return None
    wanted = depths[offset]
    for index in range(offset + 1, len(text)):
        if text[index] in _CLOSING and depths[index] == wanted:
            return text[offset + 1 : index]
    return None


def _split_top_level(text: str) -> list[str]:
    """Split ``text`` on the commas that are not inside a bracket or a string."""
    depths = _structure(text)
    parts: list[str] = []
    start = 0
    for index, char in enumerate(text):
        if char == "," and depths[index] == 0:
            parts.append(text[start:index])
            start = index + 1
    parts.append(text[start:])
    return parts


def _names_keyword_argument(text: str) -> bool:
    """True when ``text`` is a ``name=value`` argument rather than a positional one.

    Comparison operators are excluded by looking at the character on each side,
    so ``a == b`` and ``a >= b`` are arguments and ``a=b`` is not.
    """
    depths = _structure(text)
    for index, char in enumerate(text):
        if char != "=" or depths[index] != 0:
            continue
        before = text[index - 1] if index else ""
        after = text[index + 1] if index + 1 < len(text) else ""
        if before not in "=!<>" and after != "=":
            return True
    return False


def _positional_arguments(text: str, offset: int) -> list[str] | None:
    """Return one call's positional arguments, or None when it is not readable.

    None is "do not judge this call": an unbalanced fragment, an unpacked
    argument, a keyword argument or a trailing comma all mean the argument
    count on the page is not the argument count the callee will see.
    """
    inner = _inner_span(text, offset)
    if inner is None:
        return None

    arguments = [part.strip() for part in _split_top_level(inner)]
    if arguments == [""]:
        return []
    if any(not argument or argument.startswith("*") for argument in arguments):
        return None
    if any(_names_keyword_argument(argument) for argument in arguments):
        return None
    return arguments


def _parameter_shape(signature: str) -> _Shape | None:
    """Return what ``signature`` accepts positionally, or None when unreadable.

    The parameter list is the first balanced bracket pair, not the tail of the
    string: a captured signature carries its return annotation
    (``def quote(sku) -> Decimal``), and requiring the text to end in a
    parenthesis would silently exempt every annotated function in a typed
    repository -- which is to say most of the ones worth checking.

    None on anything that would make the comparison a guess: a signature the
    extractor truncated, one this module cannot find a parameter list in, and
    any list carrying a star -- ``*args``, ``**kwargs`` and the bare ``*`` that
    makes what follows keyword-only are three different rules and none of them
    is worth encoding for an advisory.
    """
    flattened = signature.strip()
    if flattened.endswith("..."):
        return None

    opening = flattened.find("(")
    if opening < 0:
        return None

    inner = _inner_span(flattened, opening)
    if inner is None:
        return None
    if not inner.strip():
        return _Shape(required=0, total=0)

    parameters = [part.strip() for part in _split_top_level(inner)]
    if any(not parameter or parameter.startswith("*") for parameter in parameters):
        return None
    return _Shape(
        required=sum(1 for parameter in parameters if not _names_keyword_argument(parameter)),
        total=len(parameters),
    )


# ---------------------------------------------------------------------------
# dangling_references
# ---------------------------------------------------------------------------


def _language_gaps(check: str, fragments: Sequence[_Fragment], what: str) -> list[Finding]:
    """Report the languages this check has no vocabulary for."""
    languages = {fragment.language for fragment in fragments}
    return [
        _gap(
            check,
            "",
            f"not checked: no {what} is known for {language}",
            f"language {language}",
        )
        for language in sorted(languages - RESOLUTION_LANGUAGES)
    ]


def _dangling_references(fragments: Sequence[_Fragment], facts: RepoFacts) -> list[Finding]:
    """Report names the patch uses that resolve to nothing at all."""
    findings = _language_gaps(CHECK_DANGLING_REFERENCES, fragments, "builtin and keyword table")
    known = _defined_names(facts)

    for fragment in fragments:
        if fragment.language not in RESOLUTION_LANGUAGES:
            continue
        bound = _bound_names(fragment)
        seen: set[str] = set()
        for usage in _usages(fragment):
            if usage.name in seen or usage.name in _ALWAYS_BOUND or usage.name in bound:
                continue
            if facts.resolver.resolve(usage.name, fragment.edit.path) is not None:
                continue
            seen.add(usage.name)
            findings.append(_dangling_reference_finding(fragment, usage, known))
    return findings


def _usages(fragment: _Fragment) -> list[_Usage]:
    """Return every name the fragment uses in a position this check judges."""
    usages = [_Usage(name=site.name, line=site.line, verb=_VERB_CALLS) for site in fragment.calls]
    for symbol in fragment.introduced:
        for base in symbol.bases:
            name = resolve.base_name(base)
            if name:
                usages.append(_Usage(name=name, line=symbol.line_number, verb=_VERB_INHERITS))
    return usages


def _bound_names(fragment: _Fragment) -> frozenset[str]:
    """Return every name the fragment binds for itself.

    Definitions it introduces, their parameters, what it imports, and the
    targets of its assignments, loops, ``as`` clauses and walrus operators.
    Deliberately generous: a name wrongly counted as bound costs one missed
    finding, and a name wrongly counted as free costs a reader a false one.
    """
    bound: set[str] = set()
    for symbol in fragment.introduced:
        bound.add(symbol.name)
        if symbol.kind in _BODY_KINDS:
            # Only a callable's parentheses hold parameters. A class's hold its
            # bases, which are uses of names rather than bindings of them --
            # and are exactly what the base-class half of this check looks at.
            bound.update(_signature_names(symbol.signature))
    for statement in fragment.imports:
        bound.update(statement.names)
        top = statement.module.strip().split(".", 1)[0].split("/", 1)[0]
        if top:
            bound.add(top)
    for line in fragment.replace_text.split("\n"):
        bound.update(_bound_in_line(line))
    return frozenset(bound)


def _bound_in_line(line: str) -> set[str]:
    """Return the names one line binds by assignment, loop, alias or walrus."""
    names: set[str] = set()
    for pattern in (_ASSIGNED, _LOOP_TARGET):
        for match in pattern.finditer(line):
            names.update(part.strip() for part in match.group(1).split(","))
    for pattern in (_ALIAS_TARGET, _WALRUS_TARGET):
        names.update(match.group(1) for match in pattern.finditer(line))
    return {name for name in names if name}


def _signature_names(signature: str) -> set[str]:
    """Return the parameter names a captured signature declares."""
    opening = signature.find("(")
    if opening < 0:
        return set()
    inner = _inner_span(signature, opening)
    if inner is None:
        return set()

    names: set[str] = set()
    for part in _split_top_level(inner):
        head = part.split("=", 1)[0].split(":", 1)[0].strip().lstrip("*").strip()
        if head.isidentifier():
            names.add(head)
    return names


def _defined_names(facts: RepoFacts) -> tuple[str, ...]:
    """Every symbol name the repository defines, in one deterministic order."""
    return tuple(
        sorted(
            {symbol.name for file_facts in facts.files.values() for symbol in file_facts.symbols}
        )
    )


def near_misses(
    name: str,
    known: Sequence[str],
    *,
    limit: int = MAX_NEAR_MISSES,
    ceiling: int = MAX_EDIT_DISTANCE,
) -> tuple[str, ...]:
    """Return the existing names closest to ``name``, nearest first.

    A name differing only in case or underscores is offered first, at distance
    zero, because ``reorder_list`` for ``reorderList`` is the same mistake as a
    typo and a reader wants it at the top. Ties break on the name itself, so
    the suggestion list is a function of the repository and not of its walk
    order.
    """
    folded = _folded(name)
    scored: list[tuple[int, str]] = []
    for candidate in known:
        if candidate == name:
            continue
        if _folded(candidate) == folded:
            scored.append((0, candidate))
            continue
        distance = _edit_distance(name, candidate, ceiling)
        if distance <= ceiling:
            scored.append((distance, candidate))
    return tuple(candidate for _, candidate in sorted(scored)[:limit])


def _folded(name: str) -> str:
    """Return the form two names share when they differ only in style."""
    return name.replace("_", "").lower()


def _edit_distance(left: str, right: str, ceiling: int) -> int:
    """Return the Levenshtein distance, abandoning it once it passes ``ceiling``.

    The early exit is what makes this affordable against every symbol name in
    a repository: a row whose cheapest cell already exceeds the ceiling cannot
    lead anywhere under it, so the remaining rows are never computed.
    """
    if abs(len(left) - len(right)) > ceiling:
        return ceiling + 1

    previous = list(range(len(right) + 1))
    for row, left_char in enumerate(left, start=1):
        current = [row]
        for column, right_char in enumerate(right, start=1):
            current.append(
                min(
                    previous[column] + 1,
                    current[column - 1] + 1,
                    previous[column - 1] + (left_char != right_char),
                )
            )
        if min(current) > ceiling:
            return ceiling + 1
        previous = current
    return previous[-1]


def _dangling_reference_finding(
    fragment: _Fragment,
    usage: _Usage,
    known: Sequence[str],
) -> Finding:
    """Build the finding for one name that resolves nowhere."""
    misses = near_misses(usage.name, known)
    suggestion = f"; did you mean {', '.join(misses)}?" if misses else ""
    return Finding(
        check=CHECK_DANGLING_REFERENCES,
        severity=Severity.WARNING,
        message=(
            f"the patch {usage.verb} {usage.name!r}, which nothing in this repository defines, "
            "the patch itself does not define, and which is neither a builtin nor a standard "
            f"library module{suggestion}"
        ),
        path=fragment.edit.path,
        line=fragment.file_line(usage.line),
        evidence=(
            f"nearest existing names: {', '.join(misses)}"
            if misses
            else "no existing name is within two edits of it"
        ),
    )


# ---------------------------------------------------------------------------
# dangling_callers
# ---------------------------------------------------------------------------


def _dangling_callers(fragments: Sequence[_Fragment], facts: RepoFacts) -> list[Finding]:
    """Report symbols the patch removes that untouched files still reference."""
    findings = _unscanned_gaps(CHECK_DANGLING_CALLERS, fragments, facts)
    touched = {fragment.edit.path for fragment in fragments}
    untouched = tuple(sorted(path for path in facts.files if path not in touched))

    reported: set[str] = set()
    for fragment in fragments:
        removed = fragment.replaced_names - {symbol.name for symbol in fragment.introduced}
        for name in sorted(removed):
            if name in reported or _defined_in(name, facts, untouched):
                continue
            sites = _reference_sites(name, facts, untouched)
            if not sites:
                continue
            reported.add(name)
            findings.append(_dangling_caller_finding(fragment, name, sites))
    return findings


def _unscanned_gaps(check: str, fragments: Sequence[_Fragment], facts: RepoFacts) -> list[Finding]:
    """Report the edited files no symbol table was supplied for."""
    missing = sorted({fragment.edit.path for fragment in fragments} - set(facts.files))
    return [
        _gap(
            check,
            path,
            "not checked: no symbol table was supplied for this file",
            "file absent from the scan",
        )
        for path in missing
    ]


def _defined_in(name: str, facts: RepoFacts, paths: Sequence[str]) -> bool:
    """True when any of ``paths`` still defines ``name``."""
    return any(symbol.name == name for path in paths for symbol in facts.files[path].symbols)


def _reference_sites(name: str, facts: RepoFacts, paths: Sequence[str]) -> tuple[str, ...]:
    """Return the ``file:line`` of every reference to ``name`` in ``paths``."""
    return tuple(
        f"{path}:{ref.line}" for path in paths for ref in facts.files[path].refs if ref.name == name
    )


def _dangling_caller_finding(fragment: _Fragment, name: str, sites: Sequence[str]) -> Finding:
    """Build the finding for one removed symbol that is still referenced."""
    listed = ", ".join(sites[:MAX_CALLER_SITES])
    more = "" if len(sites) <= MAX_CALLER_SITES else f" and {len(sites) - MAX_CALLER_SITES} more"
    return Finding(
        check=CHECK_DANGLING_CALLERS,
        severity=Severity.WARNING,
        message=(
            f"the patch removes or renames {name!r}, which {len(sites)} reference(s) in files "
            "this patch does not touch still name"
        ),
        path=fragment.edit.path,
        line=fragment.base_line,
        evidence=f"still referenced at {listed}{more}",
    )


# ---------------------------------------------------------------------------
# arity
# ---------------------------------------------------------------------------


def _arity(fragments: Sequence[_Fragment], facts: RepoFacts) -> list[Finding]:
    """Report calls in new code that cannot fit the signature they resolve to."""
    findings = _language_gaps(CHECK_ARITY, fragments, "signature grammar")
    for fragment in fragments:
        if fragment.language not in RESOLUTION_LANGUAGES:
            continue
        introduced = {symbol.name for symbol in fragment.introduced}
        for site in fragment.calls:
            finding = _arity_finding(fragment, site, facts, introduced)
            if finding is not None:
                findings.append(finding)
    return findings


def _arity_finding(
    fragment: _Fragment,
    site: _CallSite,
    facts: RepoFacts,
    introduced: frozenset[str] | set[str],
) -> Finding | None:
    """Judge one call site, or return None -- which is most of the time.

    Every gate here is a silent pass rather than a report: the callee has to
    resolve to exactly one plain function on strong evidence, the call has to
    read as purely positional, and the signature has to say unambiguously how
    many arguments it takes. Anything else and this check has nothing to say.
    """
    if site.name in introduced or site.name in _ALWAYS_BOUND:
        return None

    arguments = _positional_arguments(fragment.replace_text, site.offset)
    if arguments is None:
        return None

    definition = _plain_function(site.name, fragment.edit.path, facts)
    if definition is None:
        return None

    shape = _parameter_shape(definition.symbol.signature)
    if shape is None or shape.required <= len(arguments) <= shape.total:
        return None
    return _arity_report(fragment, site, definition, shape, len(arguments))


def _plain_function(name: str, path: str, facts: RepoFacts) -> Definition | None:
    """Resolve ``name`` to the one function an arity claim could be made about.

    Strong evidence only -- a same-file or imported binding, a single
    candidate, an undecorated ``function`` rather than a method or a class.
    A method's bound first argument and a decorator's rewritten signature are
    both reasons the count on the page is not the count that matters, and
    neither is knowable from a symbol table.
    """
    resolution = facts.resolver.resolve(name, path)
    if resolution is None or resolution.tier not in _ARITY_TIERS:
        return None
    if len(resolution.candidates) != 1:
        return None

    definition = resolution.candidates[0]
    if definition.symbol.kind is not SymbolKind.FUNCTION or definition.symbol.decorators:
        return None
    return definition


def _arity_report(
    fragment: _Fragment,
    site: _CallSite,
    definition: Definition,
    shape: _Shape,
    given: int,
) -> Finding:
    """Build the finding for one call whose argument count cannot fit."""
    accepts = (
        f"{shape.required}"
        if shape.required == shape.total
        else f"{shape.required} to {shape.total}"
    )
    return Finding(
        check=CHECK_ARITY,
        severity=Severity.ADVISORY,
        message=(
            f"the patch calls {site.name!r} with {given} positional argument(s); the definition "
            f"it resolves to takes {accepts}"
        ),
        path=fragment.edit.path,
        line=fragment.file_line(site.line),
        evidence=(
            f"{definition.path}:{definition.symbol.line_number} {definition.symbol.signature}"
        ),
    )


# ---------------------------------------------------------------------------
# cycle_delta
# ---------------------------------------------------------------------------


def _cycle_delta(
    fragments: Sequence[_Fragment],
    facts: RepoFacts,
    source: FragmentSource,
) -> list[Finding]:
    """Report import cycles the patch creates that did not exist before it.

    Both graphs are assembled in memory from the same file facts: the second
    one differs only in the imports of the files the patch edits, re-extracted
    from the patched text. Nothing is written, no worktree is materialised and
    the repository under analysis is not touched.
    """
    edited = sorted({fragment.edit.path for fragment in fragments})
    if not edited:
        return []

    missing = [path for path in edited if path not in facts.texts]
    if missing:
        return [
            _gap(
                CHECK_CYCLE_DELTA,
                "",
                f"not checked: no text was supplied for {len(missing)} edited file(s), so the "
                "post-patch import graph could not be built",
                f"first missing: {missing[0]}",
            )
        ]

    applied = apply_edits(
        [fragment.edit for fragment in fragments],
        {path: facts.texts[path] for path in edited},
    )
    if not applied.ok:
        return [
            _gap(
                CHECK_CYCLE_DELTA,
                "",
                "not checked: at least one edit did not apply, so there is no post-patch tree "
                "to compare against",
                f"{len(applied.failures)} edit(s) did not apply",
            )
        ]

    before = resolve.import_cycles(resolve.import_graph(list(facts.files.values())))
    after = resolve.import_cycles(
        resolve.import_graph(_patched_files(facts, applied.new_contents, source))
    )

    existing = {frozenset(cycle.files) for cycle in before}
    return [_cycle_finding(cycle) for cycle in after if frozenset(cycle.files) not in existing]


def _patched_files(
    facts: RepoFacts,
    contents: Mapping[str, str],
    source: FragmentSource,
) -> list[FileFacts]:
    """Return the repository's files with the patched ones re-parsed for imports.

    Only the import table is refreshed. That is the whole input to
    :func:`agentless_mcp.core.resolve.import_graph`, and re-extracting symbols
    and references for a cycle question would spend the parse this check
    exists to avoid.
    """
    patched: list[FileFacts] = []
    for path in sorted(facts.files):
        file_facts = facts.files[path]
        text = contents.get(path)
        if text is None:
            patched.append(file_facts)
            continue
        imports = source.imports_for(text, file_facts.language, path)
        patched.append(replace(file_facts, imports=tuple(imports)))
    return patched


def _cycle_finding(cycle: resolve.Cycle) -> Finding:
    """Build the finding for one import cycle the patch introduces."""
    return Finding(
        check=CHECK_CYCLE_DELTA,
        severity=Severity.WARNING,
        message=(
            f"the patch introduces an import cycle across {len(cycle.files)} files that did not "
            "exist before it"
        ),
        path=cycle.files[0],
        line=0,
        evidence=cycle.chain,
    )
