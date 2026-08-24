"""Deterministic hallucination checks over a parsed patch.

The mechanical half of a hostile first-pass review, LLM-free and run *before*
validation, so that a candidate that imports a package nobody depends on or
re-implements a function the repository already has is named as such instead of
burning a test run to find out.

Seven checks ship here. Three are resolution-independent -- they need the
patch's parsed edits and the repository's already-extracted symbol and import
tables, and nothing more. Four read
:mod:`agentless_mcp.core.resolve`, through the resolver the caller hands over
on :class:`RepoFacts`. This module opens nothing -- no cache, no repository,
no source file: every check reads the facts and text the caller supplies, and
the ``parse_*`` functions beside them take text. Reading the repository is
:mod:`agentless_mcp.application.lint_service`'s job, the dependency manifests
included: it reads them and hands the result over on :class:`RepoFacts`.

``undeclared_imports``
    A top-level package the patch imports that is in neither the repository's
    declared dependencies nor :data:`sys.stdlib_module_names`. This is the
    slopsquatting check: a hallucinated package name looks exactly like a real
    one until something tries to install it. Reading ``pyproject.toml`` needs
    ``tomllib`` arrived in 3.11; Python 3.10 uses the conditional ``tomli``
    dependency instead -- see :func:`_load_toml`.

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

**No check raises on input.** Degraded input -- a file whose text the caller
did not supply, a language with no dependency manifest, a fragment the grammar
cannot make sense of -- produces a
:data:`Severity.NOT_CHECKED` finding naming the reason. Silence is the one
outcome that would be a lie, because a caller cannot tell "checked and clean"
from "never ran". A defect *in this module* is the other thing that would be a
lie dressed as coverage, so it propagates: :data:`DEGRADED_ERRORS` names what
foreign data does and nothing else.

Two boundaries worth naming. The edit's replacement text is a *fragment*, not
a file: it is dedented before parsing, and an introduced symbol is treated as
module-level only when its line began at column 1 in the original block, so a
method added to a class is never mistaken for a top-level definition. And a
distribution name is not an import name (``PyYAML`` provides ``yaml``), which
would make the import check noisy. The escape hatch is mechanical rather than
a hand-maintained alias table: a top-level package already imported somewhere
in the repository is treated as available, because the evidence that it
installs is that the repository already runs, and a declared distribution
installed in *this* environment states the import names it provides. What a
distribution that is not installed here provides is unknown, and the check
says so beside its findings rather than dropping them.

Two more boundaries belong to the resolution-dependent half. **Only names in
call position and declared base classes are candidates for
``dangling_references``.** A bare identifier read is almost always a local,
and a check that reported every local variable as undefined would be a check
nobody reads. Which occurrences are local is not decided here: the extractor's
scope pass already labels every identifier, and this reads that label rather
than a second opinion. **The builtin and keyword tables are Python's**, so
both name-level checks report ``not_checked`` for a language whose tables are
absent rather than judging it by Python's. They are also the running
interpreter's, which the report says whenever that is not the interpreter the
repository targets. The two
graph-shaped checks -- ``dangling_callers`` and ``cycle_delta`` -- read the
extractor's own reference and import rows and run for every language it parses.
"""

import builtins
import hashlib
import importlib
import importlib.metadata
import keyword
import re
import sys
import textwrap
from bisect import bisect_right
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, replace
from enum import Enum
from functools import cached_property
from pathlib import Path
from typing import Any, Protocol, cast

from agentless_mcp.core import resolve
from agentless_mcp.core.extractor import IdentifierRole, Ref, TreeSitterExtractor
from agentless_mcp.core.graph import resolve_import_target
from agentless_mcp.core.imports import ImportStatement
from agentless_mcp.core.normalize import normalized_stream
from agentless_mcp.core.patches import Edit, apply_edits, resolve_elisions
from agentless_mcp.core.refs import Definition, FileFacts
from agentless_mcp.core.symbols import ASTSymbol, SymbolKind, qualname
from agentless_mcp.util.errors import AgentlessError

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
# the caller's manifest reader -- `lint_service.read_declared_dependencies`
# -- its manifest; a language absent from this set is reported "not
# checked", never quietly passed.
DEPENDENCY_LANGUAGES = frozenset({"python"})

# Languages whose builtin, keyword and binder vocabulary this module knows.
# The name-level checks judge a language only against its own tables; one
# absent from this set is reported "not checked", never judged by Python's.
RESOLUTION_LANGUAGES = frozenset({"python"})

# Names that are always defined and are nobody's missing helper.
#
# Built from the interpreter this process runs on, which is not necessarily the
# one the repository targets: `ExceptionGroup` is a builtin from 3.11 and
# `tomllib` a stdlib module from 3.11. There is no table of another version's
# vocabulary to consult, so the disagreement is reported rather than guessed
# at -- see `_interpreter_gaps`.
#
# `core.extractor._PYTHON_ALWAYS_BOUND` answers a different question with a
# smaller set. A bare `json` with no import is a NameError, so the reference
# classifier there leaves `sys.stdlib_module_names` out on purpose. Here the
# name arrives from a patch that may have imported it.
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
#
# Every member is something *foreign data* does, not something this module's
# own code does wrong. `TypeError`, `KeyError`, `IndexError` and
# `AttributeError` were members and are deliberately not: those are the four
# classes a None dereference, an off-by-one or a renamed field raises, and
# catching them turned a crash into a `not checked` line with no traceback
# while the other six checks reported a healthy-looking patch around it.
# `UnicodeDecodeError` is a `ValueError` and is covered by it; a manifest or a
# fragment that is not text arrives that way.
DEGRADED_ERRORS: tuple[type[Exception], ...] = (
    AgentlessError,
    ValueError,
    RecursionError,
    OSError,
)

_DISTRIBUTION_SEPARATORS = re.compile(r"[-_.]+")
_REQUIREMENT_NAME = re.compile(r"^([A-Za-z0-9][A-Za-z0-9._-]*)")

# The lowest interpreter a `requires-python` specifier admits. Only the `>=`
# clause is read: that is what the key states in practice, and a full PEP 440
# specifier parser here would be a second implementation of somebody else's
# rule for one advisory sentence.
_PYTHON_FLOOR = re.compile(r">=\s*(\d+)\.(\d+)")


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

    ``warnings`` is what went wrong *reading the repository* rather than what
    is wrong with the patch -- a dependency manifest that did not parse is the
    case that matters, because it is also the case that silences a whole
    check. A caller that renders findings and drops these reports a clean
    patch against a repository it could not read.
    """

    findings: tuple[Finding, ...]
    warnings: tuple[str, ...] = ()

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this report."""
        return {
            "findings": [finding.as_dict() for finding in self.findings],
            "warnings": list(self.warnings),
        }


@dataclass(frozen=True)
class ManifestParse:
    """One dependency manifest reduced to what the import check needs.

    ``parsed`` is the field this type exists for. A document the parser could
    not read declares nothing *knowable*, which is a different answer from a
    document that declares nothing -- and a caller handed only ``packages``
    and ``warnings`` cannot tell them apart, which is how an unreadable
    manifest came to make every third-party import look hallucinated.

    ``requires_python`` is the raw ``[project] requires-python`` specifier, or
    empty when the manifest states none. It travels with the packages because
    the same two checks that read the declared set also read the interpreter's
    builtin and standard-library tables, and those describe the interpreter
    this process runs on rather than the one the repository targets.
    """

    packages: frozenset[str]
    warnings: tuple[str, ...]
    parsed: bool
    requires_python: str = ""


@dataclass(frozen=True)
class DeclaredDependencies:
    """What a repository says it depends on, and where that was read from.

    ``packages`` holds PEP-503-normalised distribution names.
    ``sources`` is empty when no manifest was found at all, which is a
    different situation from a manifest that declares nothing: the first means
    the check cannot run, the second means every third-party import is
    genuinely undeclared.

    ``requires_python`` is the interpreter floor the manifest states, empty
    when it states none. See :func:`_interpreter_gaps` for what reads it.
    """

    packages: frozenset[str]
    sources: tuple[str, ...]
    warnings: tuple[str, ...]
    requires_python: str = ""

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
    against, and no check may open anything.

    ``bodies`` is derived rather than supplied, and cached because it is
    derived from the whole repository while a lint run judges one candidate.
    A caller linting several candidates against one repository builds these
    facts once, so the index is built once too.
    """

    files: Mapping[str, FileFacts]
    texts: Mapping[str, str]
    dependencies: DeclaredDependencies
    resolver: resolve.Resolver

    @cached_property
    def bodies(self) -> "Mapping[str, tuple[_Site, ...]]":
        """Every readable function body in the repository, indexed by its key."""
        return _existing_bodies(self)


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
    replaced_qualnames: frozenset[str]
    imports: tuple[ImportStatement, ...]
    refs: tuple[Ref, ...]
    calls: tuple[_CallSite, ...]

    def file_line(self, fragment_line: int) -> int:
        """Map a line in the replacement block to a line in the file."""
        if self.base_line <= 0 or fragment_line <= 0:
            return 0
        return self.base_line + fragment_line - 1


@dataclass(frozen=True)
class _PatchNames:
    """What the whole patch defines and removes, rather than one edit of it.

    Splitting one change across two SEARCH/REPLACE blocks in the same file is
    the ordinary shape of model output, and it is the input these checks exist
    to read. Judged one fragment at a time against the pre-patch repository,
    the second block's use of the first block's definition reads as a
    hallucination, and a definition the patch moves reads as both a removal
    and a redefinition.

    ``introduced`` pools every edit's new names, because a name any edit
    defines is a name the patch defines. ``removed_by_path`` pools per file
    instead: a definition moved from one module to another really does leave
    the first module's importers pointing at nothing.
    """

    introduced: frozenset[str]
    removed_by_path: Mapping[str, frozenset[str]]
    replaced_by_path: Mapping[str, frozenset[str]]

    def removed_in(self, path: str) -> frozenset[str]:
        """The names this patch takes out of one file and does not put back."""
        return self.removed_by_path.get(path, frozenset())

    def replaced_in(self, path: str) -> frozenset[str]:
        """The names this patch's search text held for one file."""
        return self.replaced_by_path.get(path, frozenset())


def _patch_names(fragments: Sequence[_Fragment]) -> _PatchNames:
    """Pool what every fragment defines and replaces into one patch-wide view."""
    added: dict[str, set[str]] = {}
    replaced: dict[str, set[str]] = {}
    for fragment in fragments:
        path = fragment.edit.path
        added.setdefault(path, set()).update(symbol.name for symbol in fragment.introduced)
        replaced.setdefault(path, set()).update(fragment.replaced_names)
    return _PatchNames(
        introduced=frozenset(name for names in added.values() for name in names),
        removed_by_path={
            path: frozenset(names - added.get(path, set())) for path, names in replaced.items()
        },
        replaced_by_path={path: frozenset(names) for path, names in replaced.items()},
    )


# ---------------------------------------------------------------------------
# Declared dependencies
# ---------------------------------------------------------------------------


class _TomlParser(Protocol):
    """The shared ``loads`` surface of tomllib and tomli."""

    def loads(self, text: str) -> dict[str, Any]:
        """Parse one TOML document."""


if sys.version_info >= (3, 11):
    _toml = cast(_TomlParser, importlib.import_module("tomllib"))
else:
    import tomli

    _toml = cast(_TomlParser, tomli)


def _load_toml(text: str) -> dict[str, Any]:
    """Parse TOML with the stdlib parser or the Python 3.10 fallback.

    Never reports "no parser": ``_toml`` is bound unconditionally above, and
    ``tomli`` is a hard conditional dependency, so an interpreter with neither
    fails at import rather than reaching here.
    """
    return _toml.loads(text)


_PROJECT = "project"
_REQUIRES_PYTHON = "requires-python"
_OPTIONAL = "optional-dependencies"
_DEPENDENCIES = "dependencies"
_DEPENDENCY_GROUPS = "dependency-groups"


def parse_pyproject_dependencies(text: str) -> ManifestParse:
    """Return the normalised distribution names a pyproject document declares.

    ``[project] dependencies``, every list under
    ``[project.optional-dependencies]``, and every list under PEP 735
    ``[dependency-groups]``. The groups are included deliberately: a patch to
    a test file importing a dev-only package is declared, and leaving them out
    would make the check fire on exactly the code it should not.

    A document that does not parse comes back ``parsed=False`` with the reason
    in ``warnings``, never as an empty declaration.
    """
    warnings: list[str] = []
    try:
        document = _load_toml(text)
    except DEGRADED_ERRORS as exc:
        return ManifestParse(
            packages=frozenset(),
            warnings=(f"{PYPROJECT_NAME} did not parse: {type(exc).__name__}: {exc}",),
            parsed=False,
        )

    specifications: list[str] = []
    floor = ""
    project = document.get(_PROJECT)
    if isinstance(project, dict):
        specifications.extend(_string_list(project.get(_DEPENDENCIES), warnings, "dependencies"))
        specifications.extend(_string_lists(project.get(_OPTIONAL), warnings, _OPTIONAL))
        stated = project.get(_REQUIRES_PYTHON)
        if isinstance(stated, str):
            floor = stated
        elif stated is not None:
            warnings.append(f"{_REQUIRES_PYTHON} is not a string; ignored")
    specifications.extend(
        _string_lists(document.get(_DEPENDENCY_GROUPS), warnings, _DEPENDENCY_GROUPS)
    )

    names = {name for name in (requirement_name(item) for item in specifications) if name}
    return ManifestParse(
        packages=frozenset(names),
        warnings=tuple(warnings),
        parsed=True,
        requires_python=floor,
    )


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


def python_floor(specifier: str) -> tuple[int, int] | None:
    """Return the lowest interpreter ``requires-python`` admits, or None.

    None means the specifier states no floor this can read, which is the same
    answer as no specifier at all: nothing to compare the running interpreter
    against.
    """
    match = _PYTHON_FLOOR.search(specifier)
    if match is None:
        return None
    return int(match.group(1)), int(match.group(2))


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
    caller may diff one against another. Whatever the caller could not read
    about the repository travels with them, on
    :attr:`LintReport.warnings`.
    """
    fragments, notes = _fragments(edits, facts, source)
    patch = _patch_names(fragments)
    findings = list(notes)
    findings.extend(_interpreter_gaps(fragments, facts))
    findings.extend(
        _guarded(CHECK_UNDECLARED_IMPORTS, lambda: _undeclared_imports(fragments, facts))
    )
    findings.extend(_guarded(CHECK_SHADOWING, lambda: _shadowing(fragments, facts, patch)))
    findings.extend(_guarded(CHECK_NEAR_DUPLICATES, lambda: _near_duplicates(fragments, facts)))
    findings.extend(
        _guarded(CHECK_DANGLING_REFERENCES, lambda: _dangling_references(fragments, facts, patch))
    )
    findings.extend(
        _guarded(CHECK_DANGLING_CALLERS, lambda: _dangling_callers(fragments, facts, patch))
    )
    findings.extend(_guarded(CHECK_ARITY, lambda: _arity(fragments, facts)))
    findings.extend(_guarded(CHECK_CYCLE_DELTA, lambda: _cycle_delta(fragments, facts, source)))
    return LintReport(
        findings=tuple(sorted(findings, key=_order)),
        warnings=facts.dependencies.warnings,
    )


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


def _interpreter_gaps(fragments: Sequence[_Fragment], facts: RepoFacts) -> list[Finding]:
    """Report a version-dependent table that came from the wrong interpreter.

    ``dir(builtins)`` and :data:`sys.stdlib_module_names` describe the
    interpreter this process runs on, not the one the repository targets.
    ``tomllib`` and ``ExceptionGroup`` both arrived in 3.11, so a newer
    interpreter passes a name the declared floor does not have, and an older
    one accuses a name the repository may legitimately use.

    Nothing here resolves another version's vocabulary, because the running
    interpreter does not carry one. Stating the disagreement is the honest
    answer; reading the tables and saying nothing is the guard keyed on a
    proxy that produced both directions of the error.

    Reported once, under :data:`CHECK_COVERAGE`, because it is neither
    check's fault and both read the same tables.
    """
    if not any(fragment.language in RESOLUTION_LANGUAGES for fragment in fragments):
        return []
    declared = facts.dependencies.requires_python
    floor = python_floor(declared)
    if floor is None or floor == sys.version_info[:2]:
        return []
    running = f"{sys.version_info.major}.{sys.version_info.minor}"
    return [
        _gap(
            CHECK_COVERAGE,
            "",
            f"partially checked: undeclared_imports and dangling_references read Python "
            f"{running}'s builtin and standard-library tables, and this repository requires "
            f"{declared}",
            f"requires-python {declared}; tables read from Python {running}",
        )
    ]


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
    search, replace = _resolved(edit, facts.texts.get(edit.path, ""))
    replace_text = textwrap.dedent(replace)
    search_text = textwrap.dedent(search)
    span = _locate(facts.texts.get(edit.path, ""), search)
    introduced = tuple(source.symbols_for(replace_text, language, edit.path))
    references = tuple(source.refs_for(replace_text, language, edit.path))
    replaced = tuple(source.symbols_for(search_text, language, edit.path))
    return _Fragment(
        edit=edit,
        language=language,
        replace_text=replace_text,
        replace_columns=tuple(replace.split("\n")),
        base_line=span[0],
        search_span=span,
        introduced=introduced,
        replaced_names=frozenset(symbol.name for symbol in replaced),
        replaced_qualnames=frozenset(qualname(symbol) for symbol in replaced),
        imports=tuple(source.imports_for(replace_text, language, edit.path)),
        refs=references,
        calls=_call_sites(replace_text, references, introduced),
    )


def _resolved(edit: Edit, text: str) -> tuple[str, str]:
    """Return this edit's two sides with any ``...`` elision expanded.

    The applier expands elisions before it matches. A linter matching the
    block as written anchored nowhere, so every finding about an elided edit
    came back pointing at a file with no line. The rule has one owner --
    :func:`agentless_mcp.core.patches.resolve_elisions` -- and this is the
    second caller its docstring names.

    The scope it wants is the whole file, which is what the applier passes
    when the caller named no line ranges. An elision this cannot expand comes
    back as written and anchors nowhere: the same answer as before, reached
    without a second copy of the rule.
    """
    padded = "\n" + text + "\n"
    search, replace, reason = resolve_elisions(
        edit.search, edit.replace, padded, ((0, len(padded)),)
    )
    if reason:
        return edit.search, edit.replace
    return search, replace


def _locate(text: str, search: str) -> tuple[int, int]:
    """Return the 1-based line span ``search`` occupies in ``text``.

    ``(0, 0)`` when the search text is absent or occurs more than once, which
    is the same whole-line, ambiguity-refusing rule
    :mod:`agentless_mcp.core.patches` applies -- an anchor that could be one of
    two places is not an anchor. The elision rule is applied by
    :func:`_resolved` before this runs, so what arrives here is the text the
    applier will match.
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
        findings.append(_no_manifest_gap(facts.dependencies))
        return findings

    # Read once per candidate: it walks every distribution on `sys.path`, and
    # both the mapping and the gap below are answers about the same reading.
    provided = _provided_modules()
    available = _available_packages(facts, provided)
    known_paths = sorted(facts.files)
    seen: set[tuple[str, str]] = set()
    accused: list[Finding] = []
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
            accused.append(_undeclared_finding(fragment, statement, top))

    if accused:
        findings.extend(_unmapped_gap(facts.dependencies, provided))
    findings.extend(accused)
    return findings


def _no_manifest_gap(dependencies: DeclaredDependencies) -> Finding:
    """Say why the declared set is unknown, distinguishing absent from unreadable.

    A repository with no manifest and a repository whose manifest did not
    parse both silence this check, and a reader given the same sentence for
    both would go looking for a file that is right there.
    """
    if dependencies.warnings:
        return _gap(
            CHECK_UNDECLARED_IMPORTS,
            "",
            "not checked: no dependency manifest here could be read",
            "; ".join(dependencies.warnings),
        )
    return _gap(
        CHECK_UNDECLARED_IMPORTS,
        "",
        "not checked: the repository has no pyproject.toml or requirements file",
        "no dependency manifest found",
    )


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


def _available_packages(facts: RepoFacts, provided: Mapping[str, list[str]]) -> frozenset[str]:
    """Every package name the repository has evidence of being able to import.

    Three sources, none of them a hand-maintained alias table. The declared
    distributions. Every top-level package already imported somewhere in the
    repository, which is the evidence that it installs, because the repository
    runs. And, for a declared distribution installed in the environment this
    process runs in, the import names its metadata says it provides -- which
    is what tells this that ``PyYAML`` provides ``yaml`` before any file in
    the repository has imported it.
    """
    imported = {
        normalize_distribution(top)
        for file_facts in facts.files.values()
        for statement in file_facts.imports
        for top in (_top_level_package(statement),)
        if top
    }
    declared = {
        normalize_distribution(module)
        for module, distributions in provided.items()
        if any(
            normalize_distribution(name) in facts.dependencies.packages for name in distributions
        )
    }
    return frozenset(facts.dependencies.packages | imported | declared)


def _provided_modules() -> Mapping[str, list[str]]:
    """Return this environment's import-name to distribution-name mapping.

    A distribution's import name is metadata, not a naming convention, so this
    is the only mechanical way to learn that ``opencv-python`` provides
    ``cv2``. It describes the environment this process runs in rather than the
    repository's, so what it cannot answer is stated rather than assumed --
    see :func:`_unmapped_gap`.
    """
    return importlib.metadata.packages_distributions()


def _unmapped_gap(
    dependencies: DeclaredDependencies,
    provided: Mapping[str, list[str]],
) -> list[Finding]:
    """Say which declared distributions this environment could not map.

    An installed distribution states the import names it provides, so
    ``PyYAML`` is known to provide ``yaml``. One that is not installed here
    states nothing, and an import name it would have supplied looks exactly
    like one nothing supplies.

    Reported once beside the findings rather than in place of them. Turning
    every accusation into a gap would disable the whole check whenever a
    single declared extra is missing from the environment ``lint`` runs in,
    which is the ordinary case; a reader who sees the names below can weigh
    them against the names here.
    """
    installed = {normalize_distribution(name) for names in provided.values() for name in names}
    unmapped = sorted(dependencies.packages - installed)
    if not unmapped:
        return []
    listed = ", ".join(unmapped[:MAX_DUPLICATE_SITES])
    elided = len(unmapped) - MAX_DUPLICATE_SITES
    more = "" if elided <= 0 else f" and {elided} more"
    return [
        _gap(
            CHECK_UNDECLARED_IMPORTS,
            "",
            f"partially checked: {len(unmapped)} declared distribution(s) are not installed "
            "in this environment, so the import names they provide are unknown and an import "
            "below may be one of them",
            f"not installed here: {listed}{more}",
        )
    ]


def _shadowing(
    fragments: Sequence[_Fragment],
    facts: RepoFacts,
    patch: _PatchNames,
) -> list[Finding]:
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
        # Any edit to this file may hold the definition being replaced: a
        # patch that moves a function deletes it in one block and writes it in
        # another, and the pre-patch symbol table still holds the deleted one.
        replaced = patch.replaced_in(fragment.edit.path)
        for symbol in fragment.introduced:
            if symbol.name in replaced or not _is_module_level(fragment, symbol):
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

    index = facts.bodies
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
    """True when this existing definition is the one the patch is replacing.

    Matched on the qualified name, not its tail. ``ThisClass.run`` and
    ``OtherClass.run`` are two definitions, and comparing the tail alone let a
    patch that rewrites one hide a genuine duplicate of the other.
    """
    if site.path != fragment.edit.path:
        return False
    start, end = fragment.search_span
    if start > 0 and start <= site.line <= end:
        return True
    return site.name in fragment.replaced_qualnames


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

    Only occurrences the grammar called *references* count. The extractor has
    already resolved scope, and it labels a call through a local variable
    ``local`` and its binder ``binding``. Reading every row instead made
    ``for index, (key, value) in ...`` followed by ``value(key)`` report
    ``value`` as a helper nothing defines -- a confident warning about an
    ordinary loop.
    """
    coded = {(ref.name, ref.line) for ref in references if ref.is_reference}
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

    triple = text[start : start + 3]
    marker = triple if triple in _TRIPLE_QUOTES else text[start]
    if _opens_fstring(text, start):
        return _fstring_end(text, start, marker)

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


# The letters a string literal's prefix may hold. Only `f` changes how the
# literal is scanned; the rest are here so that `rb"..."` is recognised as one
# prefix rather than as an identifier ending in `b`.
_STRING_PREFIXES = "rRbBuUfF"


def _opens_fstring(text: str, start: int) -> bool:
    """True when the literal opening at ``start`` carries an ``f`` prefix."""
    index = start - 1
    prefix = ""
    while index >= 0 and text[index] in _STRING_PREFIXES:
        prefix = text[index] + prefix
        index -= 1
    if index >= 0 and (text[index].isalnum() or text[index] == "_"):
        return False
    return "f" in prefix.lower()


def _fstring_end(text: str, start: int, marker: str) -> int:
    """Return the index just past the f-string opening at ``start``.

    PEP 701 lets a replacement field hold a string quoted the same way as the
    f-string around it: ``f"{d["key"]}"`` is valid from Python 3.12. Reading
    that inner quote as the terminator hands the rest of the line back with
    its string and code polarity inverted, and the argument count that comes
    out then disagrees with ``ast``.

    So the brace depth is tracked. Inside a replacement field a quote opens a
    literal of its own, scanned by :func:`_literal_end`; only a quote at depth
    zero closes the f-string.
    """
    depth = 0
    index = start + len(marker)
    while index < len(text):
        char = text[index]
        if char == "\\":
            index += 2
            continue
        if depth == 0 and text.startswith(marker, index):
            return index + len(marker)
        if char in {"{", "}"}:
            # A doubled brace is an escaped literal one, but only in the
            # literal portion. Inside a replacement field `}}` is the nested
            # format spec closing and then the field closing -- `f"{a:{w}}"`
            # -- and reading that pair as an escape left the depth above zero,
            # so the closing quote opened a literal instead of ending one and
            # the rest of the fragment was scanned as string. Every later call
            # then reported no bracket span, and `_arity` went silently blind.
            if depth == 0 and text[index + 1 : index + 2] == char:
                index += 2
                continue
            depth = depth + 1 if char == "{" else max(0, depth - 1)
            index += 1
            continue
        if depth > 0 and char in _QUOTES:
            index = _literal_end(text, index)
            continue
        if char == "\n" and marker not in _TRIPLE_QUOTES:
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
    """Split ``text`` on the commas that are not inside a bracket or a string.

    One construct separates arguments with a comma that no bracket encloses:
    ``lambda``, whose parameter list is closed by ``:`` rather than by ``)``.
    Its commas are at depth zero and are split like separators, so this is not
    the function that can read one. :func:`_holds_top_level_lambda` is what the
    callers ask before trusting the result.
    """
    depths = _structure(text)
    parts: list[str] = []
    start = 0
    for index, char in enumerate(text):
        if char == "," and depths[index] == 0:
            parts.append(text[start:index])
            start = index + 1
    parts.append(text[start:])
    return parts


_LAMBDA = "lambda"


def _holds_top_level_lambda(text: str) -> bool:
    """True when ``text`` holds a ``lambda`` no bracket and no string encloses.

    Only a top-level one matters. A lambda nested inside a bracket keeps its
    commas at that bracket's depth, where :func:`_split_top_level` already
    leaves them alone; a lambda at depth zero puts its parameter separators
    exactly where the argument separators are, and nothing downstream can tell
    the two apart.

    The answer to that is not to parse it. ``lambda`` joins the star and the
    keyword argument on the list of things that make the count on the page
    disagree with the count the callee sees, and the callers answer all three
    the same way: do not judge this one. Reading it as a split reported
    ``apply_to(lambda a, b: a + b)`` -- one argument -- as two, and read
    ``def sorter(key=lambda a, b: a)`` as taking a required parameter it does
    not have.
    """
    depths = _structure(text)
    at = text.find(_LAMBDA)
    while at != -1:
        end = at + len(_LAMBDA)
        before = text[at - 1] if at else ""
        after = text[end] if end < len(text) else ""
        if (
            depths[at] == 0
            and not (before.isalnum() or before == "_")
            and not (after.isalnum() or after == "_")
        ):
            return True
        at = text.find(_LAMBDA, at + 1)
    return False


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
    argument, a keyword argument, a top-level ``lambda`` or a trailing comma
    all mean the argument count on the page is not the argument count the
    callee will see.
    """
    inner = _inner_span(text, offset)
    if inner is None:
        return None
    if _holds_top_level_lambda(inner):
        return None

    arguments = [part.strip() for part in _split_top_level(inner)]
    if arguments == [""]:
        return []
    if any(not argument or argument.startswith("*") for argument in arguments):
        return None
    if any(_names_keyword_argument(argument) for argument in arguments):
        return None
    return arguments


def _parameter_list(signature: str) -> str | None:
    """Return the text inside a signature's parameter brackets, or None.

    The parameter list is the first balanced bracket pair, not the tail of the
    string: a captured signature carries its return annotation
    (``def quote(sku) -> Decimal``), and requiring the text to end in a
    parenthesis would silently exempt every annotated function in a typed
    repository -- which is to say most of the ones worth checking.

    None when there is no list to read: a signature the extractor truncated,
    and one holding no bracket pair this can balance.
    """
    flattened = signature.strip()
    if flattened.endswith("..."):
        return None
    opening = flattened.find("(")
    if opening < 0:
        return None
    return _inner_span(flattened, opening)


def _parameter_shape(signature: str) -> _Shape | None:
    """Return what ``signature`` accepts positionally, or None when unreadable.

    None on anything that would make the comparison a guess: a signature
    :func:`_parameter_list` could not read, one whose default value is a
    ``lambda`` -- whose own commas are not parameter separators -- and any
    list carrying a star: ``*args``, ``**kwargs`` and the bare ``*`` that
    makes what follows keyword-only are three different rules and none of them
    is worth encoding for an advisory.

    The positional-only ``/`` is the fourth marker in that list and is dropped
    rather than refused: unlike the stars it changes nothing about how many
    positional arguments fit, only about how they may be spelled. Counting it
    as a parameter reported ``target(1, 2, 3)`` against ``def target(a, b, /,
    c)`` as one argument short.
    """
    inner = _parameter_list(signature)
    if inner is None:
        return None
    if not inner.strip():
        return _Shape(required=0, total=0)
    if _holds_top_level_lambda(inner):
        return None

    listed = [part.strip() for part in _split_top_level(inner)]
    parameters = [part for part in listed if part != _POSITIONAL_ONLY]
    if any(not parameter or parameter.startswith("*") for parameter in parameters):
        return None
    return _Shape(
        required=sum(1 for parameter in parameters if not _names_keyword_argument(parameter)),
        total=len(parameters),
    )


# The marker that ends a signature's positional-only parameters. A parameter
# list may not hold a bare `/` for any other reason.
_POSITIONAL_ONLY = "/"


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


def _dangling_references(
    fragments: Sequence[_Fragment],
    facts: RepoFacts,
    patch: _PatchNames,
) -> list[Finding]:
    """Report names the patch uses that resolve to nothing at all."""
    findings = _language_gaps(CHECK_DANGLING_REFERENCES, fragments, "builtin and keyword table")
    known = _defined_names(facts)

    for fragment in fragments:
        if fragment.language not in RESOLUTION_LANGUAGES:
            continue
        bound = _bound_names(fragment, patch)
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


def _bound_names(fragment: _Fragment, patch: _PatchNames) -> frozenset[str]:
    """Return every name this patch binds before the fragment uses it.

    Four sources. Every definition the *whole patch* introduces, because a
    helper one edit adds is a helper the next edit may call. This fragment's
    own signatures and imports. And every occurrence the extractor labelled a
    binding or a declaration, which is where an assignment, an unpacked loop
    target, a ``with ... as`` clause, a walrus and a nested ``def`` all come
    from -- the grammar has already resolved those, and a second answer here
    written in regular expressions disagreed with it on ordinary Python.

    Deliberately generous: a name wrongly counted as bound costs one missed
    finding, and a name wrongly counted as free costs a reader a false one.
    """
    bound: set[str] = set(patch.introduced)
    for symbol in fragment.introduced:
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
    bound.update(ref.name for ref in fragment.refs if ref.role in _BINDING_ROLES)
    return frozenset(bound)


# The occurrence roles that put a name in scope. `BINDING` covers assignment,
# unpacking, ``as`` clauses, walrus and parameters; `DECLARATION` covers a
# ``def`` or ``class``, including a nested one the symbol table does not carry;
# `IMPORT` covers what an import statement names.
_BINDING_ROLES = frozenset(
    {IdentifierRole.BINDING, IdentifierRole.DECLARATION, IdentifierRole.IMPORT}
)


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


def near_misses(name: str, known: Sequence[str]) -> tuple[str, ...]:
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
        distance = _edit_distance(name, candidate, MAX_EDIT_DISTANCE)
        if distance <= MAX_EDIT_DISTANCE:
            scored.append((distance, candidate))
    return tuple(candidate for _, candidate in sorted(scored)[:MAX_NEAR_MISSES])


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


def _dangling_callers(
    fragments: Sequence[_Fragment],
    facts: RepoFacts,
    patch: _PatchNames,
) -> list[Finding]:
    """Report symbols the patch removes that untouched files still reference."""
    findings = _unscanned_gaps(CHECK_DANGLING_CALLERS, fragments, facts)
    touched = {fragment.edit.path for fragment in fragments}
    untouched = tuple(sorted(path for path in facts.files if path not in touched))

    reported: set[str] = set()
    for fragment in fragments:
        # A name another edit to the same file writes back is not removed.
        # Per file rather than patch-wide: a definition moved to a different
        # module still leaves this module's importers pointing at nothing.
        removed = fragment.replaced_names & patch.removed_in(fragment.edit.path)
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
    """Return the ``file:line`` of every reference to ``name`` in ``paths``.

    Locally-bound uses are not references to the removed symbol. A parameter
    or local variable that happens to share the spelling is what
    ``Ref.role`` exists to mark, and counting one here reports a
    caller this patch did not break -- the expensive direction for a check
    whose whole job is to say a removal is unsafe.
    """
    return tuple(
        f"{path}:{ref.line}"
        for path in paths
        for ref in facts.files[path].refs
        if ref.name == name and ref.is_reference
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
    """Report calls in new code that cannot fit the signature they resolve to.

    A file the scan did not supply a symbol table for is a stated gap, not a
    silent pass. Resolution at the ``same_file`` and ``imported`` tiers reads
    that file's own imports, so without it this check is disabled rather than
    clean -- and every new file a patch creates is in exactly that position.
    """
    findings = _language_gaps(CHECK_ARITY, fragments, "signature grammar")
    findings.extend(_unscanned_gaps(CHECK_ARITY, fragments, facts))
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
