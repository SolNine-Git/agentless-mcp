"""The deterministic hallucination checks over a parsed patch."""

import sys
from pathlib import Path

import pytest

from agentless_mcp.application.lint_service import read_declared_dependencies
from agentless_mcp.core import patchlint
from agentless_mcp.core.cache import OnDemandSource
from agentless_mcp.core.patches import Edit, parse_blocks
from agentless_mcp.core.patchlint import (
    CHECK_ARITY,
    CHECK_COVERAGE,
    CHECK_CYCLE_DELTA,
    CHECK_DANGLING_CALLERS,
    CHECK_DANGLING_REFERENCES,
    CHECK_NEAR_DUPLICATES,
    CHECK_SHADOWING,
    CHECK_UNDECLARED_IMPORTS,
    MAX_CALLER_SITES,
    DeclaredDependencies,
    RepoFacts,
    Severity,
    _guarded,
    _literal_end,
    _parameter_shape,
    _positional_arguments,
    _Shape,
    lint_patch,
    near_misses,
    normalize_distribution,
    parse_pyproject_dependencies,
    parse_requirements,
    python_floor,
    requirement_name,
)
from agentless_mcp.core.refs import RepoScan, build_ref_index, scan_repo
from agentless_mcp.core.resolve import build_resolver
from agentless_mcp.util.errors import OperationFailed

PYPROJECT = """\
[project]
name = "fixture"
version = "0.1.0"
dependencies = [
    # the unit pin, kept as one dependency with two names
    "tree-sitter>=0.25,<0.27",
    "requests ; python_version >= '3.10'",
]

[project.optional-dependencies]
mcp = [
    "fastmcp>=2.14",
]

[dependency-groups]
dev = ["pytest>=9.1.1"]
"""

# The same manifest mid-edit: one `]` short, which is what a developer running
# `lint` while editing `pyproject.toml` hands this module.
BROKEN_PYPROJECT = """\
[project]
name = "fixture"
dependencies = [
    "requests",
"""

# Not TOML at all: an unterminated table header followed by a line that is not
# an assignment. The parser has to refuse it, because a document nobody can
# read reported as a document that declares nothing makes every third-party
# import in the patch look hallucinated.
NOT_TOML = "[project\nbroken\n"

# A non-array value for the one key this check reads. `tomllib` hands the
# wrong shape through, so the caller is the one that has to say so.
NON_ARRAY_DEPENDENCIES = '[project]\ndependencies = "requests"\n'

# Reading a manifest uses `tomllib` on 3.11+ and the declared `tomli` fallback
# on 3.10. The marker name remains for the parser-focused cases, but those
# cases now run on every supported interpreter.
requires_tomllib = pytest.mark.skipif(
    False,
    reason="the supported interpreter has a TOML parser",
)

APP = """\
CONSTANT = 1


def greet(name):
    return "hello " + name


def farewell(name):
    return "bye " + name


class Box:
    def method(self):
        return 1
"""

UTIL_MATH = """\
def compute(values):
    total = 0
    for value in values:
        total = total + value * 2
    return total
"""

HELPERS = """\
def helper():
    return 1
"""

# A file that imports a package the manifest does not declare. Its presence is
# the mechanical evidence that the package installs here, which is what keeps
# the import check from accusing every distribution whose import name differs
# from its distribution name.
LEGACY = """\
import yaml


def load(text):
    return yaml.safe_load(text)
"""

# A file that imports and calls `helper`. Its references are what a patch
# removing `helper` leaves pointing at nothing.
CALLER = """\
from helpers import helper


def run():
    return helper()
"""

FIXTURE_FILES = {
    "pyproject.toml": PYPROJECT,
    "app.py": APP,
    "util_math.py": UTIL_MATH,
    "helpers.py": HELPERS,
    "legacy.py": LEGACY,
    "caller.py": CALLER,
}

# The body `util_math.compute` already has, about to be pasted in under
# another name. Long enough to clear the minimum-token floor.
DUPLICATE_BODY = """\
def tally(values):
    total = 0
    for value in values:
        total = total + value * 2
    return total"""


@pytest.fixture
def repo(tmp_path):
    """A small repository with a manifest, some modules and one seeded body."""
    root = tmp_path / "fixture"
    root.mkdir()
    for name, content in FIXTURE_FILES.items():
        (root / name).write_text(content, encoding="utf-8")
    return root


@pytest.fixture
def facts(repo, extractor):
    """The scanned, read repository the checks compare a patch against."""

    def build(root=None):
        target = root if root is not None else repo
        scan = scan_repo(target, extractor)
        files = scan.by_path()
        return RepoFacts(
            files=files,
            texts={path: (target / path).read_text(encoding="utf-8") for path in files},
            dependencies=read_declared_dependencies(target),
            resolver=build_resolver(scan, build_ref_index(scan)),
        )

    return build


@pytest.fixture
def source(extractor):
    """The fragment parser the checks use: no cache, no filesystem."""
    return OnDemandSource(extractor)


def edit(path, search, replace, index=0):
    """One parsed SEARCH/REPLACE block."""
    return Edit(index=index, path=path, search=search, replace=replace)


def checks(report, name):
    """The findings one check produced."""
    return [finding for finding in report.findings if finding.check == name]


def accusations(report, name):
    """What one check said about the patch, with its coverage gaps left out.

    The import check states what this environment could not map beside what it
    found, and which distributions are installed here is not something a test
    may depend on. Asserting on the accusations keeps these hermetic.
    """
    return [
        finding for finding in checks(report, name) if finding.severity is not Severity.NOT_CHECKED
    ]


class TestDependencyManifests:
    def test_normalization_follows_pep_503(self):
        assert normalize_distribution("Tree_Sitter.Core") == "tree-sitter-core"

    def test_a_bare_name_is_its_own_requirement(self):
        assert requirement_name("requests") == "requests"

    def test_version_specifiers_are_not_part_of_the_name(self):
        assert requirement_name("tree-sitter>=0.25,<0.27") == "tree-sitter"

    def test_extras_are_not_part_of_the_name(self):
        assert requirement_name("uvicorn[standard]>=0.30") == "uvicorn"

    def test_environment_markers_are_not_part_of_the_name(self):
        assert requirement_name('tomli ; python_version < "3.11"') == "tomli"

    def test_a_direct_reference_is_not_part_of_the_name(self):
        assert requirement_name("mypkg @ git+https://example.invalid/mypkg") == "mypkg"

    def test_an_unparseable_requirement_yields_no_name(self):
        assert requirement_name("!!!") == ""

    @requires_tomllib
    def test_every_declaration_table_is_read(self):
        parse = parse_pyproject_dependencies(PYPROJECT)

        assert parse.packages == {"tree-sitter", "requests", "fastmcp", "pytest"}
        assert parse.warnings == ()
        assert parse.parsed is True

    def test_a_malformed_manifest_declares_nothing_and_says_so(self):
        parse = parse_pyproject_dependencies(NOT_TOML)

        assert parse.packages == frozenset()
        assert len(parse.warnings) == 1
        assert parse.parsed is False

    @requires_tomllib
    def test_a_non_list_dependencies_key_is_reported_not_guessed(self):
        parse = parse_pyproject_dependencies(NON_ARRAY_DEPENDENCIES)

        assert parse.packages == frozenset()
        assert parse.warnings == ("dependencies is not a list; ignored",)
        assert parse.parsed is True

    def test_requirements_files_are_read(self):
        text = "-r other.txt\n--index-url https://example.invalid\n\n# comment\nrequests==2.0\n"

        assert parse_requirements(text) == {"requests"}

    def test_a_repository_without_a_manifest_is_not_a_repository_with_none(self, tmp_path):
        declared = read_declared_dependencies(tmp_path)

        assert declared.known is False
        assert declared.packages == frozenset()

    @requires_tomllib
    def test_the_manifest_that_was_read_is_named(self, repo):
        assert read_declared_dependencies(repo).sources == ("pyproject.toml",)

    @requires_tomllib
    def test_requirements_files_join_the_declared_set(self, repo):
        (repo / "requirements-dev.txt").write_text("coverage==7.0\n", encoding="utf-8")

        declared = read_declared_dependencies(repo)

        assert "coverage" in declared.packages
        assert declared.sources == ("pyproject.toml", "requirements-dev.txt")

    @requires_tomllib
    def test_a_manifest_that_did_not_parse_is_not_a_manifest_that_declares_nothing(self, repo):
        (repo / "pyproject.toml").write_text(BROKEN_PYPROJECT, encoding="utf-8")

        declared = read_declared_dependencies(repo)

        assert declared.known is False
        assert declared.sources == ()
        assert len(declared.warnings) == 1
        assert declared.warnings[0].startswith("pyproject.toml did not parse")


class TestMalformedManifest:
    """A manifest that did not parse must accuse nothing and say why.

    The failure this pins is the one where ``known`` stayed True over an empty
    package set, so every third-party import in the patch was reported as a
    hallucinated dependency and the parse warning that explained it was
    discarded.
    """

    def test_no_import_is_accused_of_being_hallucinated(self, facts, source, repo):
        (repo / "pyproject.toml").write_text(BROKEN_PYPROJECT, encoding="utf-8")

        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "import requests\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        findings = checks(report, CHECK_UNDECLARED_IMPORTS)
        assert [finding.severity for finding in findings] == [Severity.NOT_CHECKED]

    @requires_tomllib
    def test_the_parse_failure_reaches_the_report(self, facts, source, repo):
        (repo / "pyproject.toml").write_text(BROKEN_PYPROJECT, encoding="utf-8")

        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "import requests\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        assert len(report.warnings) == 1
        assert report.warnings[0].startswith("pyproject.toml did not parse")
        assert report.as_dict()["warnings"] == list(report.warnings)

    @requires_tomllib
    def test_the_gap_names_the_parse_failure_rather_than_a_missing_manifest(
        self, facts, source, repo
    ):
        (repo / "pyproject.toml").write_text(BROKEN_PYPROJECT, encoding="utf-8")

        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "import requests\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        gap = checks(report, CHECK_UNDECLARED_IMPORTS)[0]
        assert "did not parse" in gap.evidence

    @requires_tomllib
    def test_a_readable_manifest_carries_no_warnings(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "CONSTANT = 2")],
            facts(),
            source,
        )

        assert report.warnings == ()


class TestDegradation:
    """The line between input this module cannot read and a defect inside it.

    ``DEGRADED_ERRORS`` names the first kind. The second must reach the caller
    as a traceback: a check that turns a ``None`` dereference into a
    ``not checked`` line leaves six healthy-looking checks around it and no
    way to tell a coverage note from a crash.
    """

    def test_a_degraded_error_becomes_exactly_one_gap(self):
        def unreadable():
            message = "the fragment is not utf-8"
            raise OperationFailed(message)

        findings = _guarded(CHECK_ARITY, unreadable)

        assert [finding.severity for finding in findings] == [Severity.NOT_CHECKED]
        assert "OperationFailed" in findings[0].message

    @pytest.mark.parametrize(
        "error",
        [AttributeError, TypeError, KeyError, IndexError],
        ids=lambda error: error.__name__,
    )
    def test_a_defect_in_this_module_surfaces_as_one(self, error):
        def defective():
            message = "a renamed field"
            raise error(message)

        with pytest.raises(error):
            _guarded(CHECK_ARITY, defective)

    def test_a_fragment_the_source_cannot_read_is_reported_as_a_coverage_gap(self, facts):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "CONSTANT = 2")],
            facts(),
            _RaisingSource(OperationFailed),
        )

        gaps = checks(report, CHECK_COVERAGE)
        assert [gap.path for gap in gaps] == ["app.py"]
        assert gaps[0].severity is Severity.NOT_CHECKED

    def test_a_defect_parsing_a_fragment_surfaces_as_one(self, facts):
        with pytest.raises(AttributeError):
            lint_patch(
                [edit("app.py", "CONSTANT = 1", "CONSTANT = 2")],
                facts(),
                _RaisingSource(AttributeError),
            )


class _RaisingSource:
    """A fragment source whose parse always fails with one error class."""

    MESSAGE = "fragment parse"

    def __init__(self, error):
        self._error = error

    def symbols_for(self, text, language, path):
        raise self._error(self.MESSAGE)

    def imports_for(self, text, language, path):
        raise self._error(self.MESSAGE)

    def refs_for(self, text, language, path):
        raise self._error(self.MESSAGE)


class TestLiteralScanner:
    """What the bracket-depth scanner treats as text rather than as structure.

    The mechanism that keeps a docstring's brackets, a comment's parentheses
    and a string's commas out of the argument count. Characterised against
    Python's own grammar: every row below is what ``ast.parse`` would say
    about the same call.
    """

    @pytest.mark.parametrize(
        ("text", "arguments"),
        [
            ('f("a(b", c)', ['"a(b"', "c"]),
            ("f('it\\'s', x)", ["'it\\'s'", "x"]),
            ("f(f\"{d['k']}\", x)", ["f\"{d['k']}\"", "x"]),
            ('f("""a)b""", c)', ['"""a)b"""', "c"]),
            ('f("""doc (with [brackets]""", b)', ['"""doc (with [brackets]"""', "b"]),
            ("f('''x''', y)", ["'''x'''", "y"]),
            ('f("", b)', ['""', "b"]),
            ("f('''''', b)", ["''''''", "b"]),
            ('f(a)  # comment with ) and "quote', ["a"]),
            ("f(x, y)  # )", ["x", "y"]),
            ('f("#not a comment", b)', ['"#not a comment"', "b"]),
            ('f(a, "b, c")', ["a", '"b, c"']),
            ('f(r"\\d+", b)', ['r"\\d+"', "b"]),
            ('f("a\\"b", c)', ['"a\\"b"', "c"]),
            ('f(f"{x!r}", b)', ['f"{x!r}"', "b"]),
            ('f("s" if x else "t", y)', ['"s" if x else "t"', "y"]),
            ("f(a, \\\n  b)", ["a", "\\\n  b"]),
            ('f(f"{d["k"]}", b)', ['f"{d["k"]}"', "b"]),
            ('f(f"{d["k"]}, x")', ['f"{d["k"]}, x"']),
            # A nested same-quote literal holding the separator the scan looks
            # for. Taking its opening quote as the f-string's terminator hands
            # the rest of the line back with string and code polarity swapped.
            ('f(f"{d["a,b"]}", x)', ['f"{d["a,b"]}"', "x"]),
            ('f(f"{d["a)b"]}", x)', ['f"{d["a)b"]}"', "x"]),
            ('f(f"{d["a"]}{e["b"]}", x)', ['f"{d["a"]}{e["b"]}"', "x"]),
            ('f(f"{{a,b}}", x)', ['f"{{a,b}}"', "x"]),
            # A nested replacement field in the format spec. Its `}}` is the
            # spec closing and then the field closing, not an escaped brace,
            # and reading it as one left the scan inside the string for the
            # rest of the text -- every later call unreadable, and silently.
            ('f(f"{a:{w}}", x)', ['f"{a:{w}}"', "x"]),
            ('f(f"{a!r:>{w}}", x)', ['f"{a!r:>{w}}"', "x"]),
            ('f(f"{x:>{n}} tail", y)', ['f"{x:>{n}} tail"', "y"]),
            ('f(f"{{literal}}", x)', ['f"{{literal}}"', "x"]),
            ('f(f"{a}{b}", x)', ['f"{a}{b}"', "x"]),
        ],
    )
    def test_the_argument_scan_agrees_with_pythons_grammar(self, text, arguments):
        assert _positional_arguments(text, text.index("(")) == arguments

    @pytest.mark.parametrize(
        "text",
        [
            'f(r"a\\", b)',
            "f(a,  # note (\n    b,\n)",
        ],
    )
    def test_a_call_this_scanner_cannot_read_is_not_judged(self, text):
        assert _positional_arguments(text, text.index("(")) is None

    @pytest.mark.parametrize(
        ("text", "end"),
        [
            ('"abc" rest', 5),
            ("'abc' rest", 5),
            ('"""a "" b""" rest', 12),
            ("'''a'''", 7),
            ('"esc\\"aped" rest', 11),
            ('"unterminated', 13),
            ('"stops at\nnewline', 10),
            ("# a comment\nnext", 11),
            ("# to the end", 12),
            ('"""unterminated triple', 22),
        ],
    )
    def test_a_literal_ends_where_python_ends_it(self, text, end):
        assert _literal_end(text, 0) == end


class TestUndeclaredImports:
    @requires_tomllib
    def test_a_package_nothing_declares_is_reported(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "import nonexistent_pkg\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        findings = accusations(report, CHECK_UNDECLARED_IMPORTS)
        assert len(findings) == 1
        assert findings[0].severity is Severity.WARNING
        assert "nonexistent_pkg" in findings[0].message

    @requires_tomllib
    def test_the_finding_points_at_the_line_in_the_patched_file(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "import nonexistent_pkg\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        assert accusations(report, CHECK_UNDECLARED_IMPORTS)[0].location == "app.py:1"

    @requires_tomllib
    def test_a_declared_dependency_is_not_reported(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "import tree_sitter\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        assert checks(report, CHECK_UNDECLARED_IMPORTS) == []

    @requires_tomllib
    def test_an_optional_dependency_counts_as_declared(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "import fastmcp\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        assert checks(report, CHECK_UNDECLARED_IMPORTS) == []

    @requires_tomllib
    def test_a_dependency_group_entry_counts_as_declared(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "import pytest\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        assert checks(report, CHECK_UNDECLARED_IMPORTS) == []

    @requires_tomllib
    def test_the_standard_library_is_not_a_missing_dependency(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "import json\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        assert checks(report, CHECK_UNDECLARED_IMPORTS) == []

    @requires_tomllib
    def test_a_first_party_module_is_not_a_missing_dependency(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "from helpers import helper\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        assert checks(report, CHECK_UNDECLARED_IMPORTS) == []

    @requires_tomllib
    def test_a_package_the_repository_already_imports_is_treated_as_available(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "import yaml\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        assert checks(report, CHECK_UNDECLARED_IMPORTS) == []

    @requires_tomllib
    def test_a_relative_import_is_never_a_missing_dependency(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "from . import siblings\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        assert checks(report, CHECK_UNDECLARED_IMPORTS) == []

    @requires_tomllib
    def test_one_package_is_reported_once_per_file(self, facts, source):
        report = lint_patch(
            [
                edit("app.py", "CONSTANT = 1", "import nonexistent_pkg\n\nCONSTANT = 1"),
                edit(
                    "app.py",
                    'def farewell(name):\n    return "bye " + name',
                    'import nonexistent_pkg\n\n\ndef farewell(name):\n    return "bye " + name',
                    index=1,
                ),
            ],
            facts(),
            source,
        )

        assert len(accusations(report, CHECK_UNDECLARED_IMPORTS)) == 1

    def test_a_repository_with_no_manifest_is_reported_not_checked(
        self, facts, source, tmp_path, extractor
    ):
        bare = tmp_path / "bare"
        bare.mkdir()
        (bare / "app.py").write_text(APP, encoding="utf-8")

        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "import nonexistent_pkg\n\nCONSTANT = 1")],
            facts(bare),
            source,
        )

        findings = checks(report, CHECK_UNDECLARED_IMPORTS)
        assert [finding.severity for finding in findings] == [Severity.NOT_CHECKED]
        assert "no pyproject.toml" in findings[0].message

    def test_a_language_with_no_manifest_reader_is_reported_not_checked(self, facts, source, repo):
        (repo / "main.go").write_text("package main\n\nfunc main() {}\n", encoding="utf-8")

        report = lint_patch(
            [edit("main.go", "func main() {}", 'import "fmt"\n\nfunc main() {}')],
            facts(),
            source,
        )

        findings = checks(report, CHECK_UNDECLARED_IMPORTS)
        assert [finding.severity for finding in findings] == [Severity.NOT_CHECKED]
        assert "go" in findings[0].message


class TestTheInterpreterTheTablesCameFrom:
    """Which Python answered "is this a builtin" is a fact about the run.

    `dir(builtins)` and `sys.stdlib_module_names` describe the interpreter
    this process runs on. `tomllib` and `ExceptionGroup` both arrived in 3.11,
    so a newer interpreter passes a name the declared floor does not have and
    an older one accuses a name the repository may legitimately use. Neither
    direction is knowable here, so the disagreement is stated.
    """

    def test_the_declared_floor_is_read_from_the_manifest(self):
        parse = parse_pyproject_dependencies('[project]\nrequires-python = ">=3.10"\n')

        assert parse.requires_python == ">=3.10"

    @pytest.mark.parametrize(
        ("specifier", "floor"),
        [
            (">=3.10", (3, 10)),
            (">= 3.11, <4", (3, 11)),
            ("", None),
            ("<4", None),
        ],
    )
    def test_only_the_lower_bound_is_read(self, specifier, floor):
        assert python_floor(specifier) == floor

    def test_a_floor_below_this_interpreter_is_reported_as_a_gap(self, facts, source, repo):
        floor = f">={sys.version_info.major}.{sys.version_info.minor - 1}"
        (repo / "pyproject.toml").write_text(
            PYPROJECT.replace("version = ", f'requires-python = "{floor}"\nversion = '),
            encoding="utf-8",
        )

        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "CONSTANT = 2")],
            facts(),
            source,
        )

        gaps = checks(report, CHECK_COVERAGE)
        assert [gap.severity for gap in gaps] == [Severity.NOT_CHECKED]
        assert floor in gaps[0].evidence

    def test_a_floor_this_interpreter_matches_says_nothing(self, facts, source, repo):
        floor = f">={sys.version_info.major}.{sys.version_info.minor}"
        (repo / "pyproject.toml").write_text(
            PYPROJECT.replace("version = ", f'requires-python = "{floor}"\nversion = '),
            encoding="utf-8",
        )

        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "CONSTANT = 2")],
            facts(),
            source,
        )

        assert checks(report, CHECK_COVERAGE) == []

    def test_a_manifest_stating_no_floor_says_nothing(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "CONSTANT = 2")],
            facts(),
            source,
        )

        assert checks(report, CHECK_COVERAGE) == []


class TestDistributionNamesAreNotImportNames:
    """`PyYAML` provides `yaml`, and only its metadata says so.

    The environment mapping is stood in for rather than depended on: which
    distributions are installed beside these tests is not something a test may
    assert against.
    """

    @pytest.fixture
    def provides(self, monkeypatch):
        """Stand in for this environment's import-name to distribution map."""

        def install(mapping):
            monkeypatch.setattr(patchlint, "_provided_modules", lambda: mapping)

        return install

    def test_a_declared_distribution_covers_the_import_name_it_provides(
        self, facts, source, provides
    ):
        provides({"reqs": ["requests"], "tree_sitter": ["tree-sitter"]})

        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "import reqs\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        assert accusations(report, CHECK_UNDECLARED_IMPORTS) == []

    def test_an_import_no_installed_distribution_provides_is_still_reported(
        self, facts, source, provides
    ):
        provides({"reqs": ["requests"], "tree_sitter": ["tree-sitter"]})

        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "import nonexistent_pkg\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        assert len(accusations(report, CHECK_UNDECLARED_IMPORTS)) == 1

    def test_declared_distributions_this_environment_lacks_are_named(self, facts, source, provides):
        provides({})

        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "import nonexistent_pkg\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        gaps = [
            finding
            for finding in checks(report, CHECK_UNDECLARED_IMPORTS)
            if finding.severity is Severity.NOT_CHECKED
        ]
        assert len(gaps) == 1
        assert "4 declared distribution(s) are not installed" in gaps[0].message

    def test_nothing_is_said_when_every_declared_distribution_maps(self, facts, source, provides):
        provides(
            {
                "requests": ["requests"],
                "tree_sitter": ["tree-sitter"],
                "fastmcp": ["fastmcp"],
                "pytest": ["pytest"],
            }
        )

        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "import nonexistent_pkg\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        assert len(checks(report, CHECK_UNDECLARED_IMPORTS)) == 1


class TestShadowing:
    def test_a_new_definition_over_an_existing_name_is_reported(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "CONSTANT = 1\n\n\ndef greet(name):\n    return name")],
            facts(),
            source,
        )

        findings = checks(report, CHECK_SHADOWING)
        assert len(findings) == 1
        assert findings[0].severity is Severity.WARNING
        assert "greet" in findings[0].message
        assert "app.py:4" in findings[0].evidence

    def test_replacing_a_symbol_is_not_shadowing_it(self, facts, source):
        report = lint_patch(
            [
                edit(
                    "app.py",
                    'def greet(name):\n    return "hello " + name',
                    'def greet(name):\n    return "hi " + name',
                )
            ],
            facts(),
            source,
        )

        assert checks(report, CHECK_SHADOWING) == []

    def test_a_genuinely_new_name_is_not_shadowing(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "CONSTANT = 1\n\n\ndef shout(name):\n    return name")],
            facts(),
            source,
        )

        assert checks(report, CHECK_SHADOWING) == []

    def test_a_method_is_not_mistaken_for_a_module_level_definition(self, facts, source):
        report = lint_patch(
            [
                edit(
                    "app.py",
                    "    def method(self):\n        return 1",
                    "    def greet(self):\n        return 2",
                )
            ],
            facts(),
            source,
        )

        assert checks(report, CHECK_SHADOWING) == []

    def test_a_file_with_no_symbol_table_is_reported_not_checked(self, facts, source):
        report = lint_patch(
            [edit("brand_new.py", "", "def greet(name):\n    return name")],
            facts(),
            source,
        )

        findings = checks(report, CHECK_SHADOWING)
        assert [finding.severity for finding in findings] == [Severity.NOT_CHECKED]
        assert findings[0].path == "brand_new.py"


class TestNearDuplicates:
    def test_a_body_the_repository_already_has_is_reported(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", f"CONSTANT = 1\n\n\n{DUPLICATE_BODY}")],
            facts(),
            source,
        )

        findings = checks(report, CHECK_NEAR_DUPLICATES)
        assert len(findings) == 1
        assert findings[0].severity is Severity.ADVISORY
        assert "util_math.py:1" in findings[0].evidence

    def test_a_genuinely_new_body_is_not_reported(self, facts, source):
        body = (
            "def scale(values):\n"
            "    scaled = []\n"
            "    for value in values:\n"
            "        scaled.append(value / 3)\n"
            "    return scaled"
        )

        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", f"CONSTANT = 1\n\n\n{body}")],
            facts(),
            source,
        )

        assert checks(report, CHECK_NEAR_DUPLICATES) == []

    def test_a_trivial_body_is_below_the_floor_and_never_reported(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "CONSTANT = 1\n\n\ndef noop():\n    return 1")],
            facts(),
            source,
        )

        assert checks(report, CHECK_NEAR_DUPLICATES) == []

    def test_a_duplicate_in_another_class_is_not_hidden_by_the_replaced_one(
        self, facts, source, repo
    ):
        # `ThisClass.run` and `OtherClass.run` are two definitions. Matching
        # the replaced site on the tail of the name alone let a patch that
        # rewrites one suppress a genuine duplicate of the other.
        twins = (
            "class ThisClass:\n"
            "    def run(self, values):\n"
            "        total = 0\n"
            "        for value in values:\n"
            "            total = total + value * 2\n"
            "        return total\n"
            "\n"
            "\n"
            "class OtherClass:\n"
            "    def run(self, values):\n"
            "        total = 0\n"
            "        for value in values:\n"
            "            total = total + value * 2\n"
            "        return total\n"
        )
        (repo / "twins.py").write_text(twins, encoding="utf-8")
        block = twins.split("\n\n\n", maxsplit=1)[0].rstrip("\n")

        report = lint_patch([edit("twins.py", block, block)], facts(), source)

        findings = checks(report, CHECK_NEAR_DUPLICATES)
        assert len(findings) == 1
        assert "twins.py:10 (OtherClass.run)" in findings[0].evidence

    def test_the_body_index_is_built_once_per_repository(self, facts):
        # A caller lints several candidates against one set of facts. Building
        # the index inside each run re-normalised and re-hashed every function
        # body in the repository once per candidate.
        built = facts()

        assert built.bodies is built.bodies

    def test_rewriting_a_symbol_does_not_report_it_as_its_own_duplicate(self, facts, source):
        report = lint_patch(
            [edit("util_math.py", UTIL_MATH.rstrip("\n"), UTIL_MATH.rstrip("\n"))],
            facts(),
            source,
        )

        assert checks(report, CHECK_NEAR_DUPLICATES) == []

    def test_unread_files_are_reported_as_a_coverage_gap(self, facts, source, repo, extractor):
        scan = scan_repo(repo, extractor)
        partial = RepoFacts(
            files=scan.by_path(),
            texts={"app.py": APP},
            dependencies=read_declared_dependencies(repo),
            resolver=build_resolver(scan, build_ref_index(scan)),
        )

        report = lint_patch([edit("app.py", "CONSTANT = 1", "CONSTANT = 2")], partial, source)

        findings = checks(report, CHECK_NEAR_DUPLICATES)
        assert [finding.severity for finding in findings] == [Severity.NOT_CHECKED]
        assert "had no text supplied" in findings[0].message


class TestCoverage:
    def test_a_file_in_an_unknown_language_is_reported_not_checked(self, facts, source):
        report = lint_patch([edit("NOTES.md", "old", "new")], facts(), source)

        findings = checks(report, CHECK_COVERAGE)
        assert [finding.severity for finding in findings] == [Severity.NOT_CHECKED]
        assert ".md" in findings[0].message

    def test_a_file_with_no_extension_is_reported_not_checked(self, facts, source):
        report = lint_patch([edit("Makefile", "old", "new")], facts(), source)

        assert "(no extension)" in checks(report, CHECK_COVERAGE)[0].message

    def test_an_unanchorable_block_yields_a_finding_without_a_line(self, facts, source):
        # `return 1` appears in helpers.py and in app.py's Box.method, so the
        # block cannot be anchored to one place in the file.
        report = lint_patch(
            [
                edit(
                    "app.py",
                    "CONSTANT = 1",
                    "CONSTANT = 1\n\n\ndef greet(name):\n    return name",
                )
            ],
            facts(),
            source,
        )
        anchored = checks(report, CHECK_SHADOWING)[0]

        unanchored = lint_patch(
            [
                edit(
                    "app.py",
                    "no such text in this file",
                    "def greet(name):\n    return name",
                )
            ],
            facts(),
            source,
        )

        assert anchored.line > 0
        assert checks(unanchored, CHECK_SHADOWING)[0].location == "app.py"


class TestReport:
    def test_two_runs_produce_the_identical_report(self, facts, source):
        edits = [
            edit("app.py", "CONSTANT = 1", f"import nonexistent_pkg\n\n{DUPLICATE_BODY}"),
            edit("NOTES.md", "old", "new", index=1),
        ]
        built = facts()

        first = lint_patch(edits, built, source)
        second = lint_patch(edits, built, source)

        assert first.as_dict() == second.as_dict()

    def test_findings_are_ordered_by_check_then_location(self, facts, source):
        edits = [
            edit("app.py", "CONSTANT = 1", f"import nonexistent_pkg\n\n{DUPLICATE_BODY}"),
            edit("NOTES.md", "old", "new", index=1),
        ]

        report = lint_patch(edits, facts(), source)

        keys = [(finding.check, finding.path, finding.line) for finding in report.findings]
        assert keys == sorted(keys)

    def test_the_report_offers_no_verdict(self):
        assert not hasattr(lint_patch([], _empty_facts(), None), "ok")

    def test_a_patch_parsed_from_text_lints_the_same_way(self, facts, source):
        text = (
            "### app.py\n"
            "<<<<<<< SEARCH\n"
            "CONSTANT = 1\n"
            "=======\n"
            "import nonexistent_pkg\n"
            "\n"
            "CONSTANT = 1\n"
            ">>>>>>> REPLACE\n"
        )
        parsed = parse_blocks(text)

        report = lint_patch(parsed.edits, facts(), source)

        assert len(accusations(report, CHECK_UNDECLARED_IMPORTS)) == 1

    def test_an_empty_patch_produces_an_empty_report(self, facts, source):
        assert lint_patch([], facts(), source).findings == ()


def _empty_facts():
    """Facts about nothing: no files, no text, no manifest, nothing to resolve."""
    empty = RepoScan(root=Path(), files=(), skipped=())
    return RepoFacts(
        files={},
        texts={},
        dependencies=DeclaredDependencies(packages=frozenset(), sources=(), warnings=()),
        resolver=build_resolver(empty, build_ref_index(empty)),
    )


class TestDanglingReferences:
    def test_a_call_to_a_name_nothing_defines_is_reported(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "CONSTANT = compute_totals(1)")],
            facts(),
            source,
        )

        findings = checks(report, CHECK_DANGLING_REFERENCES)
        assert len(findings) == 1
        assert findings[0].severity is Severity.WARNING
        assert "compute_totals" in findings[0].message

    def test_a_near_miss_is_offered_as_the_name_that_was_meant(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "CONSTANT = compte(1)")],
            facts(),
            source,
        )

        finding = checks(report, CHECK_DANGLING_REFERENCES)[0]
        assert "did you mean compute?" in finding.message
        assert finding.evidence == "nearest existing names: compute"

    def test_a_call_to_an_existing_symbol_is_not_reported(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "CONSTANT = helper()")],
            facts(),
            source,
        )

        assert checks(report, CHECK_DANGLING_REFERENCES) == []

    def test_a_builtin_is_not_a_dangling_reference(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "CONSTANT = len([1, 2])")],
            facts(),
            source,
        )

        assert checks(report, CHECK_DANGLING_REFERENCES) == []

    def test_a_name_the_patch_defines_itself_is_not_dangling(self, facts, source):
        report = lint_patch(
            [
                edit(
                    "app.py",
                    "CONSTANT = 1",
                    "def local_helper():\n    return 1\n\n\nCONSTANT = local_helper()",
                )
            ],
            facts(),
            source,
        )

        assert checks(report, CHECK_DANGLING_REFERENCES) == []

    def test_a_local_the_patch_assigns_and_then_calls_is_not_dangling(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "handler = helper\nCONSTANT = handler()")],
            facts(),
            source,
        )

        assert checks(report, CHECK_DANGLING_REFERENCES) == []

    def test_a_name_inside_a_string_is_never_a_call(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", 'CONSTANT = "nonexistent_thing(1, 2)"')],
            facts(),
            source,
        )

        assert checks(report, CHECK_DANGLING_REFERENCES) == []

    def test_an_attribute_call_is_out_of_scope(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "CONSTANT = CONSTANT.no_such_method()")],
            facts(),
            source,
        )

        assert checks(report, CHECK_DANGLING_REFERENCES) == []

    def test_an_unknown_base_class_is_reported_as_inherited(self, facts, source):
        report = lint_patch(
            [
                edit(
                    "app.py",
                    "CONSTANT = 1",
                    "CONSTANT = 1\n\n\nclass Crate(NoSuchBase):\n    pass",
                )
            ],
            facts(),
            source,
        )

        finding = checks(report, CHECK_DANGLING_REFERENCES)[0]
        assert "inherits from 'NoSuchBase'" in finding.message

    def test_a_language_with_no_builtin_table_is_reported_not_checked(self, facts, source, repo):
        (repo / "main.go").write_text("package main\n\nfunc main() {}\n", encoding="utf-8")

        report = lint_patch(
            [edit("main.go", "func main() {}", "func main() { missing() }")],
            facts(),
            source,
        )

        findings = checks(report, CHECK_DANGLING_REFERENCES)
        assert [finding.severity for finding in findings] == [Severity.NOT_CHECKED]
        assert "builtin and keyword table" in findings[0].message


class TestNamesTheRestOfThePatchBinds:
    """A change split across two blocks is one patch, not two repositories.

    Splitting one change over two SEARCH/REPLACE blocks in a file is the
    ordinary shape of model output. Judged one block at a time against the
    pre-patch repository, the second block's use of the first block's
    definition reads as a hallucinated helper, and a definition the patch
    moves reads as both a removal and a redefinition.
    """

    def test_one_edits_definition_is_bound_for_the_next(self, facts, source):
        report = lint_patch(
            [
                edit("app.py", "CONSTANT = 1", "CONSTANT = 1\n\n\ndef seeded(x):\n    return x"),
                edit(
                    "app.py",
                    'def farewell(name):\n    return "bye " + name',
                    'def farewell(name):\n    return seeded("bye " + name)',
                    index=1,
                ),
            ],
            facts(),
            source,
        )

        assert checks(report, CHECK_DANGLING_REFERENCES) == []

    def test_a_definition_the_patch_moves_is_not_reported_as_shadowing(self, facts, source):
        report = lint_patch(
            [
                edit("app.py", 'def greet(name):\n    return "hello " + name', ""),
                edit(
                    "app.py",
                    'def farewell(name):\n    return "bye " + name',
                    'def farewell(name):\n    return "bye " + name\n\n\n'
                    'def greet(name):\n    return "hello " + name',
                    index=1,
                ),
            ],
            facts(),
            source,
        )

        assert checks(report, CHECK_SHADOWING) == []

    def test_a_definition_the_patch_moves_is_not_reported_as_removed(self, facts, source):
        report = lint_patch(
            [
                edit("helpers.py", "def helper():\n    return 1", ""),
                edit(
                    "helpers.py",
                    "",
                    "def helper():\n    return 2",
                    index=1,
                ),
            ],
            facts(),
            source,
        )

        assert checks(report, CHECK_DANGLING_CALLERS) == []

    def test_a_definition_the_patch_really_removes_is_still_reported(self, facts, source):
        report = lint_patch(
            [edit("helpers.py", "def helper():\n    return 1", "")],
            facts(),
            source,
        )

        assert len(checks(report, CHECK_DANGLING_CALLERS)) == 1


class TestNamesTheGrammarAlreadyScoped:
    """A local this patch binds is not a helper this repository is missing.

    Every case below is ordinary Python that the extractor's scope pass
    already labels. Judging the text with regular expressions instead
    produced a confident warning naming a real local as a hallucinated
    helper.
    """

    @pytest.mark.parametrize(
        "body",
        [
            "for index, (key, value) in enumerate(items):\n        value(key)",
            "with open('f') as (first, second):\n        first(second)",
            "callback = lambda z: z(1)\n    callback(2)",
            "first = second = int\n    second(1)",
            "def inner(handler):\n        return handler(1)",
            "if (handler := int):\n        handler(1)",
        ],
    )
    def test_a_locally_bound_name_is_not_a_dangling_reference(self, facts, source, body):
        report = lint_patch(
            [
                edit(
                    "app.py",
                    "    return 1",
                    f"    items = [(1, 2)]\n    {body}",
                )
            ],
            facts(),
            source,
        )

        assert checks(report, CHECK_DANGLING_REFERENCES) == []

    def test_a_call_to_a_name_nothing_binds_is_still_reported(self, facts, source):
        report = lint_patch(
            [edit("app.py", "    return 1", "    return absent_helper(1)")],
            facts(),
            source,
        )

        assert len(checks(report, CHECK_DANGLING_REFERENCES)) == 1


class TestElidedEdits:
    """The applier's elision rule has one owner, and this calls it.

    `apply_edits` expands `...` before it matches. Matching the block as
    written anchored nowhere, so every finding about an elided edit came back
    naming the file with no line -- the least useful place a reviewer can be
    sent.
    """

    def test_an_elided_edit_anchors_where_the_applier_would(self, facts, source):
        report = lint_patch(
            [
                edit(
                    "app.py",
                    '...\ndef farewell(name):\n    return "bye " + name',
                    "...\ndef farewell(name):\n    return absent_helper(name)",
                )
            ],
            facts(),
            source,
        )

        (finding,) = checks(report, CHECK_DANGLING_REFERENCES)
        assert finding.location == "app.py:9"

    def test_an_elision_with_no_anchor_still_refuses_to_guess(self, facts, source):
        report = lint_patch(
            [edit("app.py", "...", "    return absent_helper(1)")],
            facts(),
            source,
        )

        (finding,) = checks(report, CHECK_DANGLING_REFERENCES)
        assert finding.location == "app.py"


class TestNearMisses:
    def test_a_name_differing_only_in_style_comes_first(self):
        assert near_misses("reorder_list", ["reorderList", "reorder_lost"]) == (
            "reorderList",
            "reorder_lost",
        )

    def test_a_distant_name_is_not_a_near_miss(self):
        assert near_misses("quote", ["invoice"]) == ()

    def test_the_suggestion_list_is_bounded(self):
        assert len(near_misses("quote", ["quotes", "quota", "quode", "quoted"])) == 3

    def test_the_name_itself_is_never_its_own_suggestion(self):
        assert near_misses("quote", ["quote"]) == ()


class TestDanglingCallers:
    def test_a_removed_symbol_that_others_reference_is_reported(self, facts, source):
        report = lint_patch(
            [
                edit(
                    "helpers.py",
                    "def helper():\n    return 1",
                    "def helper_renamed():\n    return 1",
                )
            ],
            facts(),
            source,
        )

        findings = checks(report, CHECK_DANGLING_CALLERS)
        assert len(findings) == 1
        assert findings[0].severity is Severity.WARNING
        assert "helper" in findings[0].message
        assert "caller.py:5" in findings[0].evidence

    def test_a_local_of_the_same_name_is_not_a_broken_caller(
        self, facts, source, tmp_path, extractor
    ):
        """A parameter that shares the removed symbol's spelling is not a caller.

        This check exists to say a removal is unsafe, so a false positive is
        the expensive direction: it tells the author to abandon a rename that
        breaks nothing. ``Ref.role`` is what distinguishes the two,
        and the site collector has to read it.
        """
        root = tmp_path / "locals"
        root.mkdir()
        (root / "helpers.py").write_text("def helper():\n    return 1\n", encoding="utf-8")
        (root / "shadow.py").write_text("def run(helper):\n    return helper()\n", encoding="utf-8")

        report = lint_patch(
            [
                edit(
                    "helpers.py",
                    "def helper():\n    return 1",
                    "def helper_renamed():\n    return 1",
                )
            ],
            facts(root),
            source,
        )

        assert checks(report, CHECK_DANGLING_CALLERS) == []

    def test_a_symbol_the_patch_keeps_is_not_reported(self, facts, source):
        report = lint_patch(
            [edit("helpers.py", "def helper():\n    return 1", "def helper():\n    return 2")],
            facts(),
            source,
        )

        assert checks(report, CHECK_DANGLING_CALLERS) == []

    def test_a_removed_symbol_nothing_else_references_is_not_reported(self, facts, source):
        report = lint_patch(
            [
                edit(
                    "app.py",
                    'def farewell(name):\n    return "bye " + name',
                    'def parting(name):\n    return "bye " + name',
                )
            ],
            facts(),
            source,
        )

        assert checks(report, CHECK_DANGLING_CALLERS) == []

    def test_the_site_listing_is_capped_and_the_count_stays_exact(self, facts, source, repo):
        for index in range(8):
            (repo / f"user{index}.py").write_text(
                "from helpers import helper\n\n\ndef run():\n    return helper()\n",
                encoding="utf-8",
            )

        report = lint_patch(
            [
                edit(
                    "helpers.py",
                    "def helper():\n    return 1",
                    "def helper_renamed():\n    return 1",
                )
            ],
            facts(),
            source,
        )

        finding = checks(report, CHECK_DANGLING_CALLERS)[0]
        assert "9 reference(s)" in finding.message
        assert finding.evidence.count(":") == MAX_CALLER_SITES
        assert "and 4 more" in finding.evidence

    def test_a_file_with_no_symbol_table_is_reported_not_checked(self, facts, source):
        report = lint_patch(
            [edit("brand_new.py", "def gone():\n    return 1", "def here():\n    return 1")],
            facts(),
            source,
        )

        findings = checks(report, CHECK_DANGLING_CALLERS)
        assert [finding.severity for finding in findings] == [Severity.NOT_CHECKED]
        assert findings[0].path == "brand_new.py"


class TestArity:
    def test_too_many_arguments_are_reported(self, facts, source):
        report = lint_patch(
            [edit("util_math.py", "    return total", "    return compute(1, 2)")],
            facts(),
            source,
        )

        findings = checks(report, CHECK_ARITY)
        assert len(findings) == 1
        assert findings[0].severity is Severity.ADVISORY
        assert "2 positional argument(s)" in findings[0].message
        assert "takes 1" in findings[0].message

    def test_too_few_arguments_are_reported(self, facts, source):
        report = lint_patch(
            [edit("util_math.py", "    return total", "    return compute()")],
            facts(),
            source,
        )

        assert "0 positional argument(s)" in checks(report, CHECK_ARITY)[0].message

    def test_a_correct_call_is_not_reported(self, facts, source):
        report = lint_patch(
            [edit("util_math.py", "    return total", "    return compute([1])")],
            facts(),
            source,
        )

        assert checks(report, CHECK_ARITY) == []

    def test_the_positional_only_marker_is_not_a_parameter(self):
        # `/` says how the parameters before it may be spelled, not that there
        # is another one. Counting it reported a correct call one argument
        # short.
        assert _parameter_shape("def target(a, b, /, c) -> int") == _Shape(required=3, total=3)

    def test_a_star_still_stops_the_count(self):
        assert _parameter_shape("def target(a, b, *, c) -> int") is None

    @pytest.mark.parametrize(
        "signature",
        [
            "def target(key=lambda a, b: a) -> int",
            "def target(cb=lambda x: x) -> int",
        ],
    )
    def test_a_lambda_default_stops_the_count(self, signature):
        # A lambda's parameter list is closed by `:`, not by a bracket, so its
        # commas sit exactly where the parameter separators do. Splitting on
        # them read one optional parameter as two, one of them required.
        assert _parameter_shape(signature) is None

    def test_a_lambda_inside_a_bracket_still_counts(self):
        # Nested, its commas are at that bracket's depth and were never split.
        assert _parameter_shape("def target(a, b=sorted(x, key=lambda p, q: p)) -> int") == _Shape(
            required=1, total=2
        )

    @pytest.mark.parametrize(
        "signature",
        ["def target(lambda_fn, other) -> int", "def target(my_lambda, other) -> int"],
    )
    def test_an_identifier_holding_lambda_is_not_one(self, signature):
        assert _parameter_shape(signature) == _Shape(required=2, total=2)

    @pytest.mark.parametrize(
        "text",
        ["apply_to(lambda a, b: a + b)", "apply_to(lambda a: a)"],
    )
    def test_a_call_passing_a_lambda_is_not_judged(self, text):
        # `apply_to(lambda a, b: a + b)` passes one argument and was read as
        # passing two, so a correct call against a one-parameter helper was
        # reported as one argument too many.
        assert _positional_arguments(text, text.index("(")) is None

    @pytest.mark.parametrize(
        ("text", "arguments"),
        [
            ("g(sorted(x, key=lambda p, q: p), y)", ["sorted(x, key=lambda p, q: p)", "y"]),
            ('g("lambda a, b: a", z)', ['"lambda a, b: a"', "z"]),
        ],
    )
    def test_a_lambda_no_split_can_reach_leaves_the_call_readable(self, text, arguments):
        assert _positional_arguments(text, text.index("(")) == arguments

    def test_a_call_passing_a_lambda_reports_no_arity_finding(self, facts, source):
        report = lint_patch(
            [edit("util_math.py", "    return total", "    return compute(lambda a, b: a)")],
            facts(),
            source,
        )

        assert checks(report, CHECK_ARITY) == []

    def test_a_nested_def_shadowing_an_import_is_not_a_call(self, facts, source):
        # `_call_sites` takes only the occurrences the grammar called
        # references. Reading every row instead made the nested `def helper(`
        # line itself a call site, and made the call below it resolve to the
        # imported `helper()` the local one shadows -- two advisories about a
        # function that takes exactly the argument it is given.
        replacement = "def run():\n    def helper(one):\n        return one\n    return helper(1)"
        report = lint_patch(
            [edit("caller.py", "def run():\n    return helper()", replacement)],
            facts(),
            source,
        )

        assert checks(report, CHECK_ARITY) == []
        assert checks(report, CHECK_DANGLING_REFERENCES) == []

    def test_a_nested_format_spec_does_not_blind_the_rest_of_the_fragment(self, facts, source):
        # `f"{n:>{w}}"` closes a nested spec and then the field. Reading that
        # `}}` as an escaped brace left the scan inside the string, so every
        # call after it in the fragment reported no bracket span and this
        # check reported nothing at all -- about code it never read.
        report = lint_patch(
            [
                edit(
                    "util_math.py",
                    "    return total",
                    '    label = f"{total:>{width}}"\n    return compute(1, 2)',
                )
            ],
            facts(),
            source,
        )

        findings = checks(report, CHECK_ARITY)
        assert len(findings) == 1
        assert "2 positional argument(s)" in findings[0].message

    def test_a_file_the_scan_did_not_supply_is_a_stated_gap(self, facts, source):
        # Resolution needs the file's own import table, so a new file the
        # patch creates disables this check rather than passing it.
        report = lint_patch(
            [edit("new_module.py", "", "from helpers import helper\n\nhelper(1, 2, 3)\n")],
            facts(),
            source,
        )

        gaps = [finding for finding in checks(report, CHECK_ARITY) if finding.path]
        assert [gap.path for gap in gaps] == ["new_module.py"]
        assert gaps[0].severity is Severity.NOT_CHECKED

    def test_a_keyword_argument_makes_the_call_unjudgeable(self, facts, source):
        report = lint_patch(
            [edit("util_math.py", "    return total", "    return compute(values=[1], extra=2)")],
            facts(),
            source,
        )

        assert checks(report, CHECK_ARITY) == []

    def test_an_unpacked_argument_makes_the_call_unjudgeable(self, facts, source):
        report = lint_patch(
            [edit("util_math.py", "    return total", "    return compute(*[1, 2, 3])")],
            facts(),
            source,
        )

        assert checks(report, CHECK_ARITY) == []

    def test_a_method_is_never_judged(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "CONSTANT = method(1, 2, 3)")],
            facts(),
            source,
        )

        assert checks(report, CHECK_ARITY) == []

    def test_a_callee_this_repository_does_not_define_is_left_to_the_other_check(
        self, facts, source
    ):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "CONSTANT = absent_helper(1, 2, 3)")],
            facts(),
            source,
        )

        assert checks(report, CHECK_ARITY) == []
        assert checks(report, CHECK_DANGLING_REFERENCES) != []

    def test_a_language_with_no_signature_grammar_is_reported_not_checked(
        self, facts, source, repo
    ):
        (repo / "main.go").write_text("package main\n\nfunc main() {}\n", encoding="utf-8")

        report = lint_patch(
            [edit("main.go", "func main() {}", "func main() { missing() }")],
            facts(),
            source,
        )

        findings = checks(report, CHECK_ARITY)
        assert [finding.severity for finding in findings] == [Severity.NOT_CHECKED]
        assert "signature grammar" in findings[0].message


class TestCycleDelta:
    def test_a_cycle_the_patch_introduces_is_reported(self, facts, source):
        report = lint_patch(
            [
                edit("app.py", "CONSTANT = 1", "import helpers\n\nCONSTANT = 1"),
                edit(
                    "helpers.py",
                    "def helper():\n    return 1",
                    "import app\n\n\ndef helper():\n    return 1",
                    index=1,
                ),
            ],
            facts(),
            source,
        )

        findings = checks(report, CHECK_CYCLE_DELTA)
        assert len(findings) == 1
        assert findings[0].severity is Severity.WARNING
        assert findings[0].evidence == "app.py -> helpers.py -> app.py"

    def test_a_patch_that_adds_no_import_reports_nothing(self, facts, source):
        report = lint_patch(
            [edit("helpers.py", "def helper():\n    return 1", "def helper():\n    return 2")],
            facts(),
            source,
        )

        assert checks(report, CHECK_CYCLE_DELTA) == []

    def test_a_cycle_that_was_already_there_is_not_the_patch_s_doing(
        self, facts, source, repo, extractor
    ):
        (repo / "ring_a.py").write_text("import ring_b\n\n\nRING_A = 1\n", encoding="utf-8")
        (repo / "ring_b.py").write_text("import ring_a\n\n\nRING_B = 2\n", encoding="utf-8")

        report = lint_patch(
            [edit("ring_a.py", "RING_A = 1", "RING_A = 3")],
            facts(),
            source,
        )

        assert checks(report, CHECK_CYCLE_DELTA) == []

    def test_a_file_whose_text_was_not_supplied_is_reported_not_checked(
        self, source, repo, extractor
    ):
        scan = scan_repo(repo, extractor)
        partial = RepoFacts(
            files=scan.by_path(),
            texts={},
            dependencies=read_declared_dependencies(repo),
            resolver=build_resolver(scan, build_ref_index(scan)),
        )

        report = lint_patch([edit("app.py", "CONSTANT = 1", "CONSTANT = 2")], partial, source)

        findings = checks(report, CHECK_CYCLE_DELTA)
        assert [finding.severity for finding in findings] == [Severity.NOT_CHECKED]
        assert "post-patch import graph" in findings[0].message

    def test_an_edit_that_does_not_apply_is_reported_not_checked(self, facts, source):
        report = lint_patch(
            [edit("app.py", "NOT IN THE FILE", "CONSTANT = 2")],
            facts(),
            source,
        )

        findings = checks(report, CHECK_CYCLE_DELTA)
        assert [finding.severity for finding in findings] == [Severity.NOT_CHECKED]
        assert "did not apply" in findings[0].message
