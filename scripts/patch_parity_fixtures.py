"""Write the patchcheck crate's fixtures: lint reports, parser and lexer cases, interpreter tables.

Development-time only. Run it from the repository root, with every cache in a scratch directory:

    XDG_CACHE_HOME=<scratch> TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=<scratch grammar libraries> \
        uv run python scripts/patch_parity_fixtures.py

lint.json holds one case per lint_patch call: the repository files, the paths that the scan parsed
with their languages, the paths that have a text, the declared dependencies that the reference
reads from the files, the edits, and the text that json.dumps gives the report's as_dict form. The
cases start from tests/unit/test_patchlint.py, and added cases give each finding text and each
arity refusal. patchlint._provided_modules is stubbed to {}, because the Rust build reads no
installed distributions. units.json holds the cases of the manifest parsers, the near-miss ranking
and the arity lexer. interpreter.json holds the tables that _ALWAYS_BOUND reads. With --check, the
script compares its output with the committed fixtures and writes nothing.
"""

import builtins
import importlib.util
import json
import keyword
import os
import sys
import tempfile
from collections.abc import Callable, Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath
from types import ModuleType
from typing import Any
from unittest import mock

from fixture_files import GRAMMAR_OVERRIDE, Fixture, refuse_real_caches, write_or_check

from agentless_mcp.application.lint_service import read_declared_dependencies
from agentless_mcp.core import patchlint
from agentless_mcp.core.cache import OnDemandSource
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.patches import Edit, parse_blocks
from agentless_mcp.core.patchlint import ManifestParse, RepoFacts
from agentless_mcp.core.refs import build_ref_index, scan_repo
from agentless_mcp.core.resolve import build_resolver

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates" / "patchcheck" / "fixtures"
BASE_TESTS = ROOT / "tests" / "unit" / "test_patchlint.py"

Json = dict[str, Any]


def load_base() -> ModuleType:
    """Import tests/unit/test_patchlint.py, whose repository and cases the corpus starts from."""
    spec = importlib.util.spec_from_file_location("test_patchlint", BASE_TESTS)
    if spec is None or spec.loader is None:
        sys.exit(f"refused: {BASE_TESTS} does not load")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


BASE = load_base()
FILES: Mapping[str, str] = BASE.FIXTURE_FILES
GO_MAIN = "package main\n\nfunc main() {}\n"


def parameters(test: Callable[..., object]) -> list[tuple[Any, ...]]:
    """Return the argument tuples of the one parametrize mark on ``test``."""
    marks = [mark for mark in getattr(test, "pytestmark", ()) if mark.name == "parametrize"]
    if len(marks) != 1:
        sys.exit(f"refused: {test.__qualname__} has {len(marks)} parametrize marks")
    return [value if isinstance(value, tuple) else (value,) for value in marks[0].args[1]]


def edit(path: str, search: str, replace: str, index: int = 0) -> Edit:
    return Edit(index=index, path=path, search=search, replace=replace)


def constant(replace: str) -> Edit:
    """The edit that replaces app.py's CONSTANT line, which most cases start from."""
    return edit("app.py", "CONSTANT = 1", replace)


def files_with(changes: Mapping[str, str]) -> dict[str, str]:
    """Return the base repository with each named file replaced or added."""
    return {**FILES, **changes}


def with_floor(floor: str) -> dict[str, str]:
    """Return the base repository whose pyproject states ``requires-python = floor``."""
    pyproject = BASE.PYPROJECT.replace("version = ", f'requires-python = "{floor}"\nversion = ')
    return files_with({"pyproject.toml": pyproject})


@dataclass(frozen=True)
class Case:
    """One lint_patch call: a repository, the paths with a text (None: all), and the edits."""

    name: str
    edits: Sequence[Edit]
    files: Mapping[str, str] = field(default_factory=lambda: dict(FILES))
    texts: Sequence[str] | None = None


def dependencies_form(declared: patchlint.DeclaredDependencies) -> Json:
    return {
        "packages": sorted(declared.packages),
        "sources": list(declared.sources),
        "warnings": list(declared.warnings),
        "requires_python": declared.requires_python,
    }


def lint_case(case: Case, extractor: TreeSitterExtractor) -> Json:
    """Write the case's repository, scan it as the tests do, and lint the case's edits."""
    with tempfile.TemporaryDirectory(prefix="patchcheck-fixture-") as made:
        root = Path(made) / "repo"
        for path, text in case.files.items():
            target = root / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(text, encoding="utf-8")
        scan = scan_repo(root, extractor)
        if scan.skipped:
            sys.exit(f"refused: {case.name}: the scan skipped {scan.skipped}")
        files = scan.by_path()
        texted = sorted(files) if case.texts is None else sorted(case.texts)
        declared = read_declared_dependencies(root)
        facts = RepoFacts(
            files=files,
            texts={path: case.files[path] for path in texted},
            dependencies=declared,
            resolver=build_resolver(scan, build_ref_index(scan)),
        )
        with mock.patch.object(patchlint, "_provided_modules", dict):
            report = patchlint.lint_patch(list(case.edits), facts, OnDemandSource(extractor))
    return {
        "name": case.name,
        "files": dict(case.files),
        "scanned": [[path, files[path].language] for path in sorted(files)],
        "texts": texted,
        "dependencies": dependencies_form(declared),
        "edits": [entry.as_dict() for entry in case.edits],
        "expect": json.dumps(report.as_dict()),
    }


def manifest_cases() -> list[Case]:
    return [
        Case(
            "malformed_manifest_accuses_nothing",
            [constant("import requests\n\nCONSTANT = 1")],
            files_with({"pyproject.toml": BASE.BROKEN_PYPROJECT}),
        ),
        Case("readable_manifest_has_no_warnings", [constant("CONSTANT = 2")]),
        Case(
            "unreadable_manifest_and_requirements",
            [constant("import requests\n\nCONSTANT = 1")],
            files_with({"pyproject.toml": BASE.NOT_TOML, "requirements.txt": "requests\n"}),
        ),
    ]


UNDECLARED_IMPORTS = {
    "undeclared_package_is_reported": "import nonexistent_pkg",
    "declared_dependency": "import tree_sitter",
    "optional_dependency": "import fastmcp",
    "dependency_group_entry": "import pytest",
    "standard_library": "import json",
    "first_party_module": "from helpers import helper",
    "package_the_repository_imports": "import yaml",
    "relative_import": "from . import siblings",
    "submodules_of_two_packages": "import nonexistent_pkg.sub\nfrom other_missing.deep import x",
    "standard_library_submodules": (
        "import os.path\nfrom __future__ import annotations\nimport importlib.metadata"
    ),
    "aliases_report_one_package_once": (
        "import nonexistent_pkg as np, json\nfrom nonexistent_pkg import thing"
    ),
    "undistributed_package_name": "import reqs",
    "namespace_directory_without_init": "from pkg import mod",
    "dotted_first_party_module": "import pkg.mod",
    "non_ascii_package": "import Ünïcode_pkg",
}


def undeclared_cases() -> list[Case]:
    package = files_with({"pkg/mod.py": "VALUE = 1\n"})
    cases = [
        Case(name, [constant(f"{imports}\n\nCONSTANT = 1")], package)
        for name, imports in UNDECLARED_IMPORTS.items()
    ]
    many = BASE.PYPROJECT.replace(
        '"requests ; python_version',
        '"alpha",\n    "beta",\n    "gamma",\n    "requests ; python_version',
    )
    return [
        *cases,
        Case(
            "one_package_once_per_file",
            [
                constant("import nonexistent_pkg\n\nCONSTANT = 1"),
                edit(
                    "app.py",
                    'def farewell(name):\n    return "bye " + name',
                    'import nonexistent_pkg\n\n\ndef farewell(name):\n    return "bye " + name',
                    index=1,
                ),
            ],
        ),
        Case(
            "one_package_in_two_files",
            [
                constant("import nonexistent_pkg\n\nCONSTANT = 1"),
                edit(
                    "helpers.py",
                    "def helper():\n    return 1",
                    "import nonexistent_pkg\n\n\ndef helper():\n    return 1",
                    index=1,
                ),
            ],
        ),
        Case(
            "repository_without_a_manifest",
            [constant("import nonexistent_pkg\n\nCONSTANT = 1")],
            {"app.py": BASE.APP},
        ),
        Case(
            "requirements_file_alone",
            [constant("import requests\nimport missing_pkg\n\nCONSTANT = 1")],
            {"app.py": BASE.APP, "requirements.txt": "requests==2.0\n"},
        ),
        Case(
            "unmapped_gap_elides_past_three",
            [constant("import nonexistent_pkg\n\nCONSTANT = 1")],
            files_with({"pyproject.toml": many}),
        ),
        Case("import_in_a_new_file", [edit("brand_new.py", "", "import nonexistent_pkg\n")]),
        Case(
            "language_without_a_manifest_reader",
            [edit("main.go", "func main() {}", 'import "fmt"\n\nfunc main() {}')],
            files_with({"main.go": GO_MAIN}),
        ),
    ]


FLOORS_IN_A_MANIFEST = {
    "floor_below_the_tables": ">=3.12",
    "floor_the_tables_match": ">=3.13",
    "floor_above_the_tables": ">=3.14",
    "floor_in_fullwidth_digits": ">=\uff13.\uff11\uff13",
    "floor_with_an_upper_bound": ">= 3.10, <4",
    "floor_with_leading_zeros": ">=03.013",
}


def interpreter_cases() -> list[Case]:
    cases = [
        Case(name, [constant("CONSTANT = 2")], with_floor(floor))
        for name, floor in FLOORS_IN_A_MANIFEST.items()
    ]
    go_only = {**with_floor(">=3.12"), "main.go": GO_MAIN}
    return [
        *cases,
        Case(
            "floor_with_no_python_fragment",
            [edit("main.go", "func main() {}", "func main() { missing() }")],
            go_only,
        ),
    ]


def shadowing_cases() -> list[Case]:
    twice = "def dup():\n    return 1\n\n\ndef dup():\n    return 2\n"
    return [
        Case(
            "new_definition_over_an_existing_name",
            [constant("CONSTANT = 1\n\n\ndef greet(name):\n    return name")],
        ),
        Case(
            "replacing_a_symbol",
            [
                edit(
                    "app.py",
                    'def greet(name):\n    return "hello " + name',
                    'def greet(name):\n    return "hi " + name',
                )
            ],
        ),
        Case(
            "genuinely_new_name",
            [constant("CONSTANT = 1\n\n\ndef shout(name):\n    return name")],
        ),
        Case(
            "method_is_not_module_level",
            [
                edit(
                    "app.py",
                    "    def method(self):\n        return 1",
                    "    def greet(self):\n        return 2",
                )
            ],
        ),
        Case(
            "file_with_no_symbol_table",
            [edit("brand_new.py", "", "def greet(name):\n    return name")],
        ),
        Case(
            "class_over_an_existing_class",
            [constant("CONSTANT = 1\n\n\nclass Box:\n    pass")],
        ),
        Case(
            "first_definition_wins",
            [edit("twice.py", "", "def dup():\n    return 3\n")],
            files_with({"twice.py": twice}),
        ),
        Case(
            "unanchored_block",
            [edit("app.py", "no such text in this file", "def greet(name):\n    return name")],
        ),
    ]


TWINS = (
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

NEW_BODY = (
    "def scale(values):\n"
    "    scaled = []\n"
    "    for value in values:\n"
    "        scaled.append(value / 3)\n"
    "    return scaled"
)

BOX_METHOD = "class Box:\n    def method(self):\n        return 1"
DUPLICATE_METHOD = (
    f"{BOX_METHOD}\n\n"
    "    def tally(self, values):\n        total = 0\n        for value in values:\n"
    "            total = total + value * 2\n        return total"
)


def duplicate_cases() -> list[Case]:
    block = TWINS.split("\n\n\n", maxsplit=1)[0].rstrip("\n")
    copies = files_with(
        {f"copy{index}.py": BASE.UTIL_MATH.replace("compute", f"copy{index}") for index in range(4)}
    )
    pasted = constant(f"CONSTANT = 1\n\n\n{BASE.DUPLICATE_BODY}")
    return [
        Case("body_the_repository_has", [pasted]),
        Case("genuinely_new_body", [constant(f"CONSTANT = 1\n\n\n{NEW_BODY}")]),
        Case("trivial_body", [constant("CONSTANT = 1\n\n\ndef noop():\n    return 1")]),
        Case(
            "duplicate_in_another_class",
            [edit("twins.py", block, block)],
            files_with({"twins.py": TWINS}),
        ),
        Case(
            "rewriting_a_symbol",
            [edit("util_math.py", BASE.UTIL_MATH.rstrip("\n"), BASE.UTIL_MATH.rstrip("\n"))],
        ),
        Case("unread_files", [constant("CONSTANT = 2")], texts=["app.py"]),
        Case("sites_past_three_are_counted", [pasted], copies),
        Case("duplicate_method", [edit("app.py", BOX_METHOD, DUPLICATE_METHOD)]),
        Case(
            "duplicate_in_an_unanchored_block",
            [edit("app.py", "absent anchor", BASE.DUPLICATE_BODY)],
        ),
    ]


def coverage_cases() -> list[Case]:
    return [
        Case("unknown_language", [edit("NOTES.md", "old", "new")]),
        Case("no_extension", [edit("Makefile", "old", "new")]),
        Case("dotfile_has_no_extension", [edit(".bashrc", "old", "new")]),
        Case(
            "unsupported_suffixes_are_counted",
            [
                edit("a.md", "old", "new"),
                edit("b.md", "old", "new", index=1),
                edit("c.txt", "old", "new", index=2),
            ],
        ),
        Case(
            "two_edits_with_an_unknown_language",
            [
                constant(f"import nonexistent_pkg\n\n{BASE.DUPLICATE_BODY}"),
                edit("NOTES.md", "old", "new", index=1),
            ],
        ),
        Case("empty_patch", []),
        Case("patch_parsed_from_text", parse_blocks(PARSED_PATCH).edits),
    ]


PARSED_PATCH = (
    "### app.py\n"
    "<<<<<<< SEARCH\n"
    "CONSTANT = 1\n"
    "=======\n"
    "import nonexistent_pkg\n"
    "\n"
    "CONSTANT = 1\n"
    ">>>>>>> REPLACE\n"
)

DANGLING_REPLACEMENTS = {
    "call_to_a_name_nothing_defines": "CONSTANT = compute_totals(1)",
    "near_miss_is_offered": "CONSTANT = compte(1)",
    "call_to_an_existing_symbol": "CONSTANT = helper()",
    "builtin_call": "CONSTANT = len([1, 2])",
    "name_the_patch_defines": "def local_helper():\n    return 1\n\n\nCONSTANT = local_helper()",
    "local_assigned_then_called": "handler = helper\nCONSTANT = handler()",
    "name_inside_a_string": 'CONSTANT = "nonexistent_thing(1, 2)"',
    "attribute_call": "CONSTANT = CONSTANT.no_such_method()",
    "unknown_base_class": "CONSTANT = 1\n\n\nclass Crate(NoSuchBase):\n    pass",
    "interpreter_names_are_bound": (
        "exit(0)\ncopyright()\n__import__('x')\nExceptionGroup('a', [])\n"
        "PythonFinalizationError()\nasyncio(1)\nCONSTANT = tomllib(1)"
    ),
    "signature_names_are_bound": (
        "def run(a, *args, cb=None, **kw):\n    return cb(a) + args(1) + kw(2)\n\n\nCONSTANT = 1"
    ),
    "import_names_are_bound": "from os import path as joined\nCONSTANT = joined(1) + os(2)",
    "keyword_base_is_not_a_base": (
        "class Crate(NoSuchBase, metaclass=NoSuchMeta):\n    pass\n\n\nCONSTANT = 1"
    ),
    "dotted_and_generic_bases": "class Crate(pkg.Missing, Generic[T]):\n    pass\n\n\nCONSTANT = 1",
    "same_name_reported_once": "CONSTANT = missing(1) + missing(2)",
    "call_in_a_comment": "# missing(1)\nCONSTANT = 1",
}

QUOTE_NAMES = ("quotes", "quota", "quode", "quoted")


def scoped_cases() -> list[Case]:
    scoped = BASE.TestNamesTheGrammarAlreadyScoped
    bodies = parameters(scoped.test_a_locally_bound_name_is_not_a_dangling_reference)
    return [
        Case(
            f"locally_bound_{index}",
            [edit("app.py", "    return 1", f"    items = [(1, 2)]\n    {body}")],
        )
        for index, (body,) in enumerate(bodies)
    ]


def dangling_cases() -> list[Case]:
    cases = [Case(name, [constant(replace)]) for name, replace in DANGLING_REPLACEMENTS.items()]
    unicode = files_with(
        {"names.py": "def café(x):\n    return x\n\n\ndef reorderList(x):\n    return x\n"}
    )
    quotes = files_with(
        {"quotes.py": "".join(f"def {name}():\n    return 1\n\n\n" for name in QUOTE_NAMES)}
    )
    script = files_with({"app.js": "const a = 1;\n", "b.js": "export const b = 2;\n"})
    return [
        *cases,
        *scoped_cases(),
        Case("call_nothing_binds", [edit("app.py", "    return 1", "    return absent_helper(1)")]),
        Case("non_ascii_near_miss", [constant("CONSTANT = cafe(1) + reorder_list(2)")], unicode),
        Case("near_misses_are_bounded", [constant("CONSTANT = quote(1)")], quotes),
        Case(
            "language_without_a_builtin_table",
            [edit("main.go", "func main() {}", "func main() { missing() }")],
            files_with({"main.go": GO_MAIN}),
        ),
        Case("call_in_a_new_file", [edit("brand_new.py", "", "CONSTANT = missing(1)\n")]),
        Case(
            "javascript_fragment",
            [edit("app.js", "const a = 1;", "import { b } from './b.js';\nconst a = missing(1);")],
            script,
        ),
    ]


def patch_wide_cases() -> list[Case]:
    return [
        Case(
            "one_edits_definition_binds_the_next",
            [
                constant("CONSTANT = 1\n\n\ndef seeded(x):\n    return x"),
                edit(
                    "app.py",
                    'def farewell(name):\n    return "bye " + name',
                    'def farewell(name):\n    return seeded("bye " + name)',
                    index=1,
                ),
            ],
        ),
        Case(
            "moved_definition_is_not_shadowing",
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
        ),
        Case(
            "moved_definition_is_not_removed",
            [
                edit("helpers.py", "def helper():\n    return 1", ""),
                edit("helpers.py", "", "def helper():\n    return 2", index=1),
            ],
        ),
        Case("really_removed_definition", [edit("helpers.py", "def helper():\n    return 1", "")]),
        Case(
            "elided_edit_anchors",
            [
                edit(
                    "app.py",
                    '...\ndef farewell(name):\n    return "bye " + name',
                    "...\ndef farewell(name):\n    return absent_helper(name)",
                )
            ],
        ),
        Case("elision_with_no_anchor", [edit("app.py", "...", "    return absent_helper(1)")]),
        Case(
            "bare_elision_anchors_the_first_unique_line",
            [edit("app.py", "...", "def extra():\n    return absent_helper(1)")],
        ),
    ]


RENAME = edit("helpers.py", "def helper():\n    return 1", "def helper_renamed():\n    return 1")
USER = "from helpers import helper\n\n\ndef run():\n    return helper()\n"
PAIR = {
    "pair.py": "def first():\n    return 1\n\n\ndef second():\n    return 2\n",
    "pair_user.py": "from pair import first, second\n\nVALUE = second() + first()\n",
}


def caller_cases() -> list[Case]:
    users = files_with({f"user{index}.py": USER for index in range(8)})
    locals_only = {
        "helpers.py": "def helper():\n    return 1\n",
        "shadow.py": "def run(helper):\n    return helper()\n",
    }
    removed_pair = edit("pair.py", PAIR["pair.py"].rstrip("\n"), "")
    return [
        Case("removed_symbol_others_reference", [RENAME]),
        Case("local_of_the_same_name", [RENAME], locals_only),
        Case(
            "symbol_the_patch_keeps",
            [edit("helpers.py", "def helper():\n    return 1", "def helper():\n    return 2")],
        ),
        Case(
            "removed_symbol_nothing_references",
            [
                edit(
                    "app.py",
                    'def farewell(name):\n    return "bye " + name',
                    'def parting(name):\n    return "bye " + name',
                )
            ],
        ),
        Case("site_listing_is_capped", [RENAME], users),
        Case(
            "callers_file_with_no_symbol_table",
            [edit("brand_new.py", "def gone():\n    return 1", "def here():\n    return 1")],
        ),
        Case("two_removed_names_in_name_order", [removed_pair], files_with(PAIR)),
        Case(
            "removed_name_still_defined_elsewhere",
            [RENAME],
            files_with({"again.py": "def helper():\n    return 3\n"}),
        ),
    ]


ARITY_REPLACEMENTS = {
    "too_many_arguments": "    return compute(1, 2)",
    "too_few_arguments": "    return compute()",
    "correct_call": "    return compute([1])",
    "call_passing_a_lambda": "    return compute(lambda a, b: a)",
    "keyword_argument": "    return compute(values=[1], extra=2)",
    "unpacked_argument": "    return compute(*[1, 2, 3])",
    "keyword_splat": "    return compute(**options)",
    "trailing_comma": "    return compute(1,)",
    "unbalanced_call": "    return compute(1, 2",
    "nested_format_spec": '    label = f"{total:>{width}}"\n    return compute(1, 2)',
    "multiline_call": "    return compute(\n        1,\n        2\n    )",
    "multiline_call_with_trailing_comma": "    return compute(\n        1,\n        2,\n    )",
    "fstring_argument": '    return compute(f"{total!r}", 2)',
    "comparison_arguments": "    return compute(total == 1) + compute(total >= 1, total)",
    "call_in_a_comment_and_a_string": '    # compute(1, 2)\n    return "compute(1, 2)"',
    "builtin_callee": "    return len(1, 2, 3)",
    "introduced_callee": "    def local(a):\n        return a\n    return local(1, 2)",
}

SIGNATURES = (
    "def star_target(*values):\n    return values\n\n\n"
    "def keyword_only(a, *, b):\n    return a\n\n\n"
    "def very_long_function_name_for_truncation(alpha, beta, gamma, delta, epsilon, zeta):\n"
    "    return alpha\n\n\n"
    "def sorter(key=lambda a, b: a):\n    return key\n\n\n"
    "def positional(a, b, /, c):\n    return a\n\n\n"
    "def optional(a, b=1):\n    return a\n\n\n"
    "def typed(a: int = 3, b: str = 'x') -> int:\n    return a\n\n\n"
    "def twice(a):\n    return a\n\n\n"
    "def twice(a, b):\n    return a\n\n\n"
    "class Shape:\n    pass\n\n\n"
    "def open(a):\n    return a\n\n\n"
    "@staticmethod\n"
    "def decorated(a):\n    return a\n\n\n"
    "def plain(a):\n    return a\n\n\n"
    "ANCHOR = 1\n"
)

SIGNATURE_CALLS = {
    "star_signature": "star_target(1, 2)",
    "keyword_only_signature": "keyword_only(1, 2, 3)",
    "truncated_signature": "very_long_function_name_for_truncation(1)",
    "lambda_default_signature": "sorter(1, 2, 3)",
    "positional_only_marker": "positional(1, 2)",
    "optional_parameters": "optional(1, 2, 3)",
    "annotated_defaults": "typed(1, 2, 3) + typed()",
    "two_same_file_candidates": "twice(1, 2, 3)",
    "class_callee": "Shape(1, 2)",
    "builtin_named_callee": "open(1, 2, 3)",
    "decorated_callee": "decorated(1, 2)",
    "plain_callee": "plain(1, 2)",
}

NESTED_DEF = "def run():\n    def helper(one):\n        return one\n    return helper(1)"


def signature_cases() -> list[Case]:
    signatures = files_with({"shapes.py": SIGNATURES})
    return [
        Case(name, [edit("shapes.py", "ANCHOR = 1", f"ANCHOR = {call}")], signatures)
        for name, call in SIGNATURE_CALLS.items()
    ]


def arity_cases() -> list[Case]:
    cases = [
        Case(name, [edit("util_math.py", "    return total", replace)])
        for name, replace in ARITY_REPLACEMENTS.items()
    ]
    remote = files_with({"remote.py": "def remote(a):\n    return a\n"})
    return [
        *cases,
        *signature_cases(),
        Case("unique_tier_is_not_strong_evidence", [constant("CONSTANT = remote(1, 2)")], remote),
        Case(
            "import_the_patch_adds_is_not_in_scope",
            [constant("from util_math import compute\n\nCONSTANT = compute(1, 2)")],
        ),
        Case(
            "imported_callee",
            [edit("caller.py", "    return helper()", "    return helper(1, 2)")],
        ),
        Case(
            "nested_def_shadowing_an_import",
            [edit("caller.py", "def run():\n    return helper()", NESTED_DEF)],
        ),
        Case(
            "file_the_scan_did_not_supply",
            [edit("new_module.py", "", "from helpers import helper\n\nhelper(1, 2, 3)\n")],
        ),
        Case("method_is_never_judged", [constant("CONSTANT = method(1, 2, 3)")]),
        Case("callee_nothing_defines", [constant("CONSTANT = absent_helper(1, 2, 3)")]),
        Case(
            "language_without_a_signature_grammar",
            [edit("main.go", "func main() {}", "func main() { missing() }")],
            files_with({"main.go": GO_MAIN}),
        ),
    ]


RINGS = {
    "ring_a.py": "import ring_b\n\n\nRING_A = 1\n",
    "ring_b.py": "import ring_a\n\n\nRING_B = 2\n",
}
CHAIN = {
    "chain_a.py": "import chain_b\n\n\nA = 1\n",
    "chain_b.py": "import chain_c\n\n\nB = 1\n",
    "chain_c.py": "C = 1\n",
}
SCRIPTS = {"one.js": "const one = 1;\n", "two.js": "const two = 2;\n"}
CLOSING_EDITS = (
    constant("import helpers\n\nCONSTANT = 1"),
    edit(
        "helpers.py",
        "def helper():\n    return 1",
        "import app\n\n\ndef helper():\n    return 1",
        index=1,
    ),
)


def cycle_cases() -> list[Case]:
    scripts = [
        edit("one.js", "const one = 1;", "import './two.js';\nconst one = 1;"),
        edit("two.js", "const two = 2;", "import './one.js';\nconst two = 2;", index=1),
    ]
    return [
        Case("cycle_the_patch_introduces", CLOSING_EDITS),
        Case(
            "patch_that_adds_no_import",
            [edit("helpers.py", "def helper():\n    return 1", "def helper():\n    return 2")],
        ),
        Case(
            "cycle_that_was_there",
            [edit("ring_a.py", "RING_A = 1", "RING_A = 3")],
            files_with(RINGS),
        ),
        Case("new_cycle_beside_an_old_one", CLOSING_EDITS, files_with(RINGS)),
        Case(
            "three_file_cycle",
            [edit("chain_c.py", "C = 1", "import chain_a\n\n\nC = 1")],
            files_with(CHAIN),
        ),
        Case("texts_not_supplied", [constant("CONSTANT = 2")], texts=[]),
        Case("edit_that_does_not_apply", [edit("app.py", "NOT IN THE FILE", "CONSTANT = 2")]),
        Case("javascript_cycle", scripts, files_with(SCRIPTS)),
    ]


def lint_cases() -> list[Case]:
    return [
        *manifest_cases(),
        *undeclared_cases(),
        *interpreter_cases(),
        *shadowing_cases(),
        *duplicate_cases(),
        *coverage_cases(),
        *dangling_cases(),
        *patch_wide_cases(),
        *caller_cases(),
        *arity_cases(),
        *cycle_cases(),
    ]


CHECKS = (
    patchlint.CHECK_UNDECLARED_IMPORTS,
    patchlint.CHECK_SHADOWING,
    patchlint.CHECK_NEAR_DUPLICATES,
    patchlint.CHECK_DANGLING_REFERENCES,
    patchlint.CHECK_DANGLING_CALLERS,
    patchlint.CHECK_ARITY,
    patchlint.CHECK_CYCLE_DELTA,
    patchlint.CHECK_COVERAGE,
)


def lint_fixture() -> Fixture:
    extractor = TreeSitterExtractor()
    cases = lint_cases()
    names = [case.name for case in cases]
    repeated = sorted({name for name in names if names.count(name) > 1})
    if repeated:
        sys.exit(f"refused: repeated case names: {repeated}")
    rows = [lint_case(case, extractor) for case in cases]
    reported = {row["check"] for case in rows for row in json.loads(case["expect"])["findings"]}
    missing = sorted(set(CHECKS) - reported)
    if missing:
        sys.exit(f"refused: no case reports a row for {missing}")
    return {"lint_patch": section(rows)}


PYPROJECTS = (
    "",
    "project = 1\n",
    '[project]\noptional-dependencies = ["a"]\n',
    '[project.optional-dependencies]\nz = ["zed"]\na = "notalist"\nB = [1, "bee", true]\n',
    '[dependency-groups]\ndev = ["pytest", {include-group = "test"}]\ntest = ["coverage"]\n',
    "dependency-groups = 3\n",
    "[project]\nrequires-python = 3.10\n",
    "[project]\nrequires-python = 1979-05-27\n",
    '[project]\ndependencies = ["!!!", "ok", " spaced ", "Ünicode", "pkg@ url", "x;y"]\n',
    '[project]\ndependencies = [\n  "a",\n]\n',
    '[project]\nname = "a"\nname = "b"\n',
    "a = " + "[" * 80 + "1" + "]" * 80 + "\n",
    "[project\n",
    '[project]\ndependencies = ["a"]\n[project]\n',
    "x = 1979-02-29\n",
    "x = 1979-05-27T07:32:00.999999999\n",
    "x = 07:32:00.123456789\n",
    "x = 0x_1\n",
    "x = 1__0\n",
    "x = +inf\n",
    '"" = 1\n',
    's = "a\x7fb"\n',
    's = "\\u0000"\n',
    "x = 1\r\n",
    "x = 1\ry = 2\n",
    's = "\\uD800"\n',
    "x = 2000-01-01T24:00:00\n",
    "x = 1979-05-27 07:32:00Z\n",
    "x = 0001-01-01\n",
    "x = 1979-13-01\n",
    "a.b = 1\n[a]\nc = 2\n",
    "x = 1.\n",
    "x = .5\n",
)

# The recorded deviations: documents that only the toml crate reads (TOML 1.1, a byte order mark,
# a 60th second, year 0000), and documents that only tomllib reads (deep nesting, wide numbers).
PYPROJECTS_RUST_ONLY = (
    "x = {a = 1,}\n",
    "x = {\n  a = 1\n}\n",
    's = "\\e"\n',
    's = "\\x41"\n',
    "t = 07:32\n",
    '\ufeff[project]\nname = "x"\n',
    "\ufeff\n",
    "x = 1979-01-01T00:00:60\n",
    "x = 0000-01-01\n",
)
PYPROJECTS_PYTHON_ONLY = (
    "a = " + "[" * 81 + "1" + "]" * 81 + "\n",
    "x = 99999999999999999999\n",
    "x = 9223372036854775808\n",
    "x = -9223372036854775809\n",
    "x = 1e400\n",
)

REQUIREMENTS = (
    "-r other.txt\n--index-url https://example.invalid\n\n# comment\nrequests==2.0\n",
    "a\r\nb\r\n",
    "pkg # comment\n#only a comment\n  -e .\n",
    "x\x0by\x0cz\x1cw\x85v\u2028u\u2029t\n",
    "Foo_Bar==1\nfoo[bar]>=1\n  \npkg;marker\n-r other.txt\ncafé\n",
    "\u3000requests\u3000\n",
    "",
)

REQUIREMENT_NAMES = (
    "requests",
    "tree-sitter>=0.25,<0.27",
    "uvicorn[standard]>=0.30",
    'tomli ; python_version < "3.11"',
    "mypkg @ git+https://example.invalid/mypkg",
    "!!!",
    " requests ",
    "requests;python_version",
    "a_b.c-D",
    "--opt",
    "Ünicode",
    "\u3000spaced\u3000",
    "",
    "9lives",
    "_private",
)

DISTRIBUTIONS = (
    "Tree_Sitter.Core",
    "a--b__c..d",
    "ÄBC",
    "\u0130x",
    "\u03a3\u0391\u03a3",
    "a-_.b",
    "",
    "x.",
    ".x",
)

FLOORS = (
    ">=3.10",
    ">= 3.11, <4",
    "",
    "<4",
    ">=3.13",
    ">=\uff13.\uff11\uff13",
    "==3.10",
    ">=3",
    ">=3.",
    "~=3.10",
    ">=10.20",
    ">=\u30003.10",
    ">=\t3.10",
    "<3.9,>=3.8",
    ">=003.013",
    "> =3.10",
    ">=3.10.1",
    ">=\u0663.\u0661\u0663",
)

NEAR_MISSES = (
    ("reorder_list", ["reorderList", "reorder_lost"]),
    ("quote", ["invoice"]),
    ("quote", ["quotes", "quota", "quode", "quoted"]),
    ("quote", ["quote"]),
    ("café", ["cafe", "Café", "cafes"]),
    ("x", ["y", "xx", "xyz"]),
    ("a", ["b", "b"]),
    ("ab", ["ba"]),
    ("\u0130", ["i\u0307"]),
    ("abc", ["ABC_"]),
    ("", ["a", "ab", "abc"]),
    ("kitten", ["sitting", "kitchen", "mitten", "bitten"]),
)

POSITIONAL = (
    "f()",
    "f( )",
    "f(a=1)",
    "f(a==1)",
    "f(=1)",
    "f(a<=1, b!=2, c>=3)",
    "f(**k)",
    "f(a, *b)",
    "f(xlambda, lambda_fn)",
    "f(a, (b, c), [d, e], {f: g})",
    "f(rf'{a}', Rb'x', u'y')",
    "f(elf'x', y)",
    "f(f'''{a}\n''', b)",
    "f(f'{a}', '{', b)",
    "f(f'{a}}}', b)",
    "f(a) + g(b, c)",
    "f(a",
    "f(a, b))",
    'f("a\\\\", b)',
    "f(é, ü)",
    "f(x if y else z, w)",
    "f(lambda: 0)",
    "f(g(lambda a, b: a))",
)

SHAPES = (
    "def f()",
    "def f( )",
    "def f(a, b=1)",
    "def f(a, *args)",
    "def f(a, /)",
    "def f(a, b, ...",
    "class X",
    "def f(a, b",
    "def f(a: int = 3, b: str)",
    "def f(a=(1, 2), b)",
    "def f(a, b) -> Tuple[int, int]",
    "def f(a,)",
    "def f(x=lambda: 0)",
    "def f(a, **kw)",
    "def f(a='(', b)",
    "  def f(a)  ",
    "def f(a=b==c)",
)

LITERALS = (
    ("x = 'a' rest", 4),
    ("rf'{x}' rest", 2),
    ("elf'x' rest", 3),
    ("f'''{a}\n''' rest", 1),
    ("f'{a}' rest", 1),
    ("f'{\"a\"}' rest", 1),
    ("f'{a:{b}}' rest", 1),
    ("f'{{x' rest", 1),
    ("f'{a\nb}' rest", 1),
    ("f'x\\'y' rest", 1),
    ("b'x' rest", 1),
    ('F"x" rest', 1),
    ("_f'x' rest", 2),
    ("'' rest", 0),
    ("'''''' rest", 0),
    ("'a\\\nb' rest", 0),
    ("f'{a' rest", 1),
    ("f'''{'''} ''' x", 1),
)

SIGNATURE_NAMES = (
    "def f(a, b=1, *args, c: int = 2, **kw) -> X",
    "def f(self)",
    "def f(",
    "class X(Base)",
    "def f((a))",
    "def f(a, b) -> g(c)",
    "def f(*, key)",
    "def f(é, _x, 1y)",
    "def f",
)

SUFFIXES = (
    "a.py",
    "a.",
    ".bashrc",
    "a/b.tar.gz",
    "a/..",
    "x/.",
    "dir/",
    "Makefile",
    "a.b/c",
    "..py",
    "a..py",
    "src/x.PY",
    "x.tsx",
    "x.h",
    "",
    "/",
)


def section(cases: Iterable[object]) -> list[object]:
    """Return ``cases`` as one fixture section."""
    return list(cases)


def manifest_form(parse: ManifestParse) -> Json:
    return {
        "packages": sorted(parse.packages),
        "warnings": list(parse.warnings),
        "parsed": parse.parsed,
        "requires_python": parse.requires_python,
    }


def pyproject_rows(texts: Iterable[str]) -> list[Json]:
    return [
        {"text": text, "expect": manifest_form(patchlint.parse_pyproject_dependencies(text))}
        for text in texts
    ]


def deviation_rows(texts: Iterable[str], parsed: bool) -> list[Json]:
    """Return the rows of ``texts``, refusing one whose reference outcome is not ``parsed``."""
    rows = pyproject_rows(texts)
    other = [row["text"] for row in rows if row["expect"]["parsed"] is not parsed]
    if other:
        sys.exit(f"refused: the reference parsed={not parsed} for the deviation cases {other}")
    return rows


def positional_texts() -> list[str]:
    scanner = BASE.TestLiteralScanner
    arity = BASE.TestArity
    pairs = [
        *parameters(scanner.test_the_argument_scan_agrees_with_pythons_grammar),
        *parameters(arity.test_a_lambda_no_split_can_reach_leaves_the_call_readable),
    ]
    singles = [
        *parameters(scanner.test_a_call_this_scanner_cannot_read_is_not_judged),
        *parameters(arity.test_a_call_passing_a_lambda_is_not_judged),
    ]
    return [row[0] for row in (*pairs, *singles)] + list(POSITIONAL)


def shape_texts() -> list[str]:
    arity = BASE.TestArity
    recorded = [
        "def target(a, b, /, c) -> int",
        "def target(a, b, *, c) -> int",
        "def target(a, b=sorted(x, key=lambda p, q: p)) -> int",
        *(row[0] for row in parameters(arity.test_a_lambda_default_stops_the_count)),
        *(row[0] for row in parameters(arity.test_an_identifier_holding_lambda_is_not_one)),
    ]
    return [*recorded, *SHAPES]


def shape_form(text: str) -> list[int] | None:
    shape = patchlint._parameter_shape(text)
    return None if shape is None else [shape.required, shape.total]


def floor_form(specifier: str) -> list[int] | None:
    floor = patchlint.python_floor(specifier)
    return None if floor is None else list(floor)


def suffix_row(path: str) -> Json:
    suffix = PurePosixPath(path).suffix
    language = TreeSitterExtractor.SUPPORTED_EXTENSIONS.get(suffix)
    return {"path": path, "suffix": suffix, "language": language}


def positional_row(text: str) -> Json:
    offset = text.index("(")
    return {"text": text, "offset": offset, "expect": patchlint._positional_arguments(text, offset)}


def manifest_sections() -> Fixture:
    texts = [
        BASE.PYPROJECT,
        BASE.BROKEN_PYPROJECT,
        BASE.NOT_TOML,
        BASE.NON_ARRAY_DEPENDENCIES,
        '[project]\nrequires-python = ">=3.10"\n',
        *PYPROJECTS,
    ]
    return {
        "parse_pyproject": section(pyproject_rows(texts)),
        "parse_pyproject_rust_only": section(deviation_rows(PYPROJECTS_RUST_ONLY, parsed=False)),
        "parse_pyproject_python_only": section(deviation_rows(PYPROJECTS_PYTHON_ONLY, parsed=True)),
        "parse_requirements": [
            {"text": text, "expect": sorted(patchlint.parse_requirements(text))}
            for text in REQUIREMENTS
        ],
        "requirement_name": [
            {"text": text, "expect": patchlint.requirement_name(text)} for text in REQUIREMENT_NAMES
        ],
        "normalize_distribution": [
            {"text": text, "expect": patchlint.normalize_distribution(text)}
            for text in DISTRIBUTIONS
        ],
        "python_floor": [{"text": text, "expect": floor_form(text)} for text in FLOORS],
    }


def lexer_sections() -> Fixture:
    literal = BASE.TestLiteralScanner.test_a_literal_ends_where_python_ends_it
    literal_rows = [(row[0], 0) for row in parameters(literal)] + list(LITERALS)
    return {
        "near_misses": [
            {"name": name, "known": known, "expect": list(patchlint.near_misses(name, known))}
            for name, known in NEAR_MISSES
        ],
        "positional_arguments": [positional_row(text) for text in positional_texts()],
        "parameter_shape": [{"text": text, "expect": shape_form(text)} for text in shape_texts()],
        "literal_end": [
            {"text": text, "start": start, "expect": patchlint._literal_end(text, start)}
            for text, start in literal_rows
        ],
        "signature_names": [
            {"text": text, "expect": sorted(patchlint._signature_names(text))}
            for text in SIGNATURE_NAMES
        ],
        "suffix": [suffix_row(path) for path in SUFFIXES],
    }


def units_fixture() -> Fixture:
    return {**manifest_sections(), **lexer_sections()}


def interpreter_fixture() -> Fixture:
    names = set(dir(builtins))
    words = set(keyword.kwlist)
    modules = set(sys.stdlib_module_names)
    if names | words | modules | {"self", "cls"} != set(patchlint._ALWAYS_BOUND):
        sys.exit("refused: the tables do not make patchlint._ALWAYS_BOUND")
    return {
        "version": [list(sys.version_info[:2])],
        "builtins": section(sorted(names)),
        "keywords": section(sorted(words)),
        "stdlib_module_names": section(sorted(modules)),
    }


def main() -> int:
    refuse_real_caches()
    if not os.environ.get(GRAMMAR_OVERRIDE):
        sys.exit(f"refused: set {GRAMMAR_OVERRIDE} to a scratch copy of the grammar libraries")
    os.environ["AGENTLESS_MCP_NO_DOWNLOAD"] = "1"
    os.environ["AGENTLESS_MCP_NO_AUTO_WARM"] = "1"
    builders: dict[str, Callable[[], Fixture]] = {
        "lint": lint_fixture,
        "units": units_fixture,
        "interpreter": interpreter_fixture,
    }
    return write_or_check((__doc__ or "").splitlines()[0], FIXTURES, builders)


if __name__ == "__main__":
    raise SystemExit(main())
