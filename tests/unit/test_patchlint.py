"""The deterministic hallucination checks over a parsed patch."""

from pathlib import Path

import pytest

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
    _positional_arguments,
    lint_patch,
    near_misses,
    normalize_distribution,
    parse_pyproject_dependencies,
    parse_requirements,
    read_declared_dependencies,
    requirement_name,
)
from agentless_mcp.core.refs import RepoScan, build_ref_index, scan_repo
from agentless_mcp.core.resolve import build_resolver
from agentless_mcp.util.errors import AgentlessError

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
            raise AgentlessError(message)

        findings = _guarded(CHECK_ARITY, unreadable)

        assert [finding.severity for finding in findings] == [Severity.NOT_CHECKED]
        assert "AgentlessError" in findings[0].message

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
            _RaisingSource(AgentlessError),
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


class TestManifestParserUnavailable:
    """What an explicitly unavailable TOML parser reports.

    The normal runtime has a parser on every supported Python version. These
    tests keep the degraded-input contract explicit by simulating an
    unavailable parser.

    Nothing here skips: the degraded path is stood in for rather than depended
    on, so the contract remains tested on every supported interpreter.
    """

    @pytest.fixture
    def no_toml_parser(self, monkeypatch):
        """Stand in for a runtime where no TOML parser is available."""
        monkeypatch.setattr(patchlint, "_load_toml", lambda _text: None)

    def test_the_manifest_declares_nothing_knowable_rather_than_nothing(self, no_toml_parser):
        parse = parse_pyproject_dependencies(PYPROJECT)

        assert parse.parsed is False
        assert parse.packages == frozenset()
        assert parse.warnings == (
            ("pyproject.toml not read: no TOML parser is available, so this check did not run"),
        )

    def test_a_manifest_that_could_not_be_read_is_not_a_source(self, repo, no_toml_parser):
        declared = read_declared_dependencies(repo)

        assert declared.known is False
        assert declared.packages == frozenset()
        assert len(declared.warnings) == 1

    def test_the_import_check_reports_a_gap_rather_than_hallucinations(
        self, facts, source, no_toml_parser
    ):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "import nonexistent_pkg\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        findings = checks(report, CHECK_UNDECLARED_IMPORTS)
        assert [finding.severity for finding in findings] == [Severity.NOT_CHECKED]
        assert "no TOML parser is available" in findings[0].evidence

    def test_the_reason_reaches_the_report(self, facts, source, no_toml_parser):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "CONSTANT = 2")],
            facts(),
            source,
        )

        assert len(report.warnings) == 1
        assert "no TOML parser is available" in report.warnings[0]

    def test_the_other_checks_still_run(self, facts, source, no_toml_parser):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "def greet(name):\n    return name\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        assert checks(report, CHECK_SHADOWING)


class TestUndeclaredImports:
    @requires_tomllib
    def test_a_package_nothing_declares_is_reported(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "import nonexistent_pkg\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        findings = checks(report, CHECK_UNDECLARED_IMPORTS)
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

        assert checks(report, CHECK_UNDECLARED_IMPORTS)[0].location == "app.py:1"

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

        assert len(checks(report, CHECK_UNDECLARED_IMPORTS)) == 1

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

        assert len(checks(report, CHECK_UNDECLARED_IMPORTS)) == 1

    def test_an_empty_patch_produces_an_empty_report(self, facts, source):
        assert lint_patch([], facts(), source).findings == ()

    def test_severity_filtering_returns_report_order(self, facts, source):
        report = lint_patch(
            [edit("app.py", "CONSTANT = 1", "import nonexistent_pkg\n\nCONSTANT = 1")],
            facts(),
            source,
        )

        assert report.of_severity(Severity.WARNING) == tuple(
            finding for finding in report.findings if finding.severity is Severity.WARNING
        )


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
