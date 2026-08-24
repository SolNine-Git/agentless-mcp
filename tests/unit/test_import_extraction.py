"""Tests for tree-sitter import extraction.

Ported from mcp-local's ``tests/unit/test_import_extraction.py``; only the
module paths changed. See ``test_extractor.py`` for the grammar-drift note
that applies to the whole port.
"""

import pytest

from agentless_mcp.core import grammars, refs, resolve
from agentless_mcp.core.extractor import LANGUAGE_CONFIGS, ImportBinding


class TestBareImports:
    def test_simple_import(self, extractor):
        source = "import os"
        imports = extractor.extract_imports_from_source(source, "python", "test.py")
        assert len(imports) == 1
        assert imports[0].module == "os"
        assert imports[0].names == ()
        assert imports[0].is_relative is False
        assert imports[0].relative_level == 0

    def test_dotted_import(self, extractor):
        source = "import os.path"
        imports = extractor.extract_imports_from_source(source, "python", "test.py")
        assert len(imports) == 1
        assert imports[0].module == "os.path"

    def test_aliased_import(self, extractor):
        source = "import numpy as np"
        imports = extractor.extract_imports_from_source(source, "python", "test.py")
        assert len(imports) == 1
        assert imports[0].module == "numpy"


class TestFromImports:
    def test_simple_from_import(self, extractor):
        source = "from pathlib import Path"
        imports = extractor.extract_imports_from_source(source, "python", "test.py")
        assert len(imports) == 1
        assert imports[0].module == "pathlib"
        assert "Path" in imports[0].names
        assert imports[0].is_relative is False

    def test_multiple_names(self, extractor):
        source = "from agentless_mcp.core.symbols import ASTSymbol, SymbolKind"
        imports = extractor.extract_imports_from_source(source, "python", "test.py")
        assert len(imports) == 1
        assert imports[0].module == "agentless_mcp.core.symbols"
        assert "ASTSymbol" in imports[0].names
        assert "SymbolKind" in imports[0].names

    def test_aliased_from_import(self, extractor):
        source = "from collections import OrderedDict as OD"
        imports = extractor.extract_imports_from_source(source, "python", "test.py")
        assert len(imports) == 1
        assert "OrderedDict" in imports[0].names

    def test_wildcard_import(self, extractor):
        source = "from module import *"
        imports = extractor.extract_imports_from_source(source, "python", "test.py")
        assert len(imports) == 1
        assert "*" in imports[0].names


class TestRelativeImports:
    def test_single_dot_import(self, extractor):
        source = "from . import utils"
        imports = extractor.extract_imports_from_source(source, "python", "test.py")
        assert len(imports) == 1
        assert imports[0].is_relative is True
        assert imports[0].relative_level == 1

    def test_double_dot_import(self, extractor):
        source = "from ..core import Base"
        imports = extractor.extract_imports_from_source(source, "python", "test.py")
        assert len(imports) == 1
        assert imports[0].is_relative is True
        assert imports[0].relative_level == 2
        assert imports[0].module == "core"

    def test_relative_with_module(self, extractor):
        source = "from .symbols import ASTSymbol"
        imports = extractor.extract_imports_from_source(source, "python", "test.py")
        assert len(imports) == 1
        assert imports[0].is_relative is True
        assert imports[0].relative_level == 1
        assert imports[0].module == "symbols"
        assert "ASTSymbol" in imports[0].names


class TestEdgeCases:
    def test_empty_file(self, extractor):
        imports = extractor.extract_imports_from_source("", "python", "test.py")
        assert imports == []

    def test_no_imports(self, extractor):
        source = "x = 1\ndef foo(): pass"
        imports = extractor.extract_imports_from_source(source, "python", "test.py")
        assert imports == []

    def test_line_numbers(self, extractor):
        source = "x = 1\nimport os\nfrom sys import path"
        imports = extractor.extract_imports_from_source(source, "python", "test.py")
        assert len(imports) == 2
        assert imports[0].line_number == 2
        assert imports[1].line_number == 3

    def test_multiple_imports(self, extractor):
        source = """
import os
import sys
from pathlib import Path
from typing import Any
"""
        imports = extractor.extract_imports_from_source(source, "python", "test.py")
        assert len(imports) == 4


# Every language whose import binds a module object rather than any of its
# names, named one at a time. A row that is not here has to declare a binding
# shape of its own, which is what stops the next language added from
# inheriting `MODULE_OBJECT` by omission -- the omission that demoted every
# Java named import from `resolved-via-import` to `unique`.
#
# `swift` is on this list and should not be: `import Foundation` brings the
# module's public names in unqualified, which is the `binds_all` shape rather
# than either of the two below. It is recorded here so the claim is wrong in
# writing rather than wrong in silence.
BINDS_A_MODULE_OBJECT = frozenset(
    {"go", "lua", "bash", "ruby", "swift", "json", "toml", "yaml", "hcl", "sql"}
)

LAST_SEGMENT_SOURCES = {
    "java": ("A.java", "import com.acme.Money;\n"),
    "kotlin": ("A.kt", "import com.acme.Money\n"),
    "scala": ("A.scala", "import pricing.Money\n"),
    "csharp": ("A.cs", "using App.Money;\n"),
    "php": ("A.php", "<?php\nuse App\\Money;\n"),
}


class TestEveryRowDeclaresHowItsImportBinds:
    """The table states the claim; this asserts no row inherits one.

    `import_binding` decides whether a bare reference may be read as evidence
    of an import, which is the difference between a tier a caller is told to
    trust and one it is told not to. The default is the safe answer for Python
    and Go and the wrong answer for Java, so a row that never mentions the
    field is a row nobody decided.
    """

    def test_no_row_is_left_to_the_default_by_accident(self):
        defaulted = {
            language
            for language, config in LANGUAGE_CONFIGS.items()
            if config.import_binding is ImportBinding.MODULE_OBJECT
        }

        assert defaulted == BINDS_A_MODULE_OBJECT

    def test_the_allow_list_names_only_rows_that_exist(self):
        assert set(LANGUAGE_CONFIGS) >= BINDS_A_MODULE_OBJECT


class TestLastSegmentImports:
    """`import com.acme.Money;` binds `Money`, in all five spellings of it."""

    @pytest.mark.parametrize("language", sorted(LAST_SEGMENT_SOURCES))
    def test_the_final_path_segment_is_the_bound_name(self, language, extractor):
        if language not in grammars.warmed_languages():
            pytest.skip(f"grammar for {language} is not in the local pack cache")
        path, source = LAST_SEGMENT_SOURCES[language]

        (statement,) = extractor.extract_imports_from_source(source, language, path)

        assert statement.bound_names() == (("Money", "Money"),)

    def test_a_wildcard_binds_no_single_name(self, extractor):
        # `import com.acme.*` names a package, whose target is a directory and
        # therefore no file this resolver can offer. Reading the path's last
        # segment there would bind `acme`, which the file never spells.
        if "java" not in grammars.warmed_languages():
            pytest.skip("grammar for java is not in the local pack cache")
        (statement,) = extractor.extract_imports_from_source(
            "import com.acme.*;\n", "java", "A.java"
        )

        assert statement.names == ()

    def test_an_alias_binds_the_alias_and_keeps_the_member(self, extractor):
        # Resolution needs `Money` to find the file; only `M` is spellable here.
        if "kotlin" not in grammars.warmed_languages():
            pytest.skip("grammar for kotlin is not in the local pack cache")
        (statement,) = extractor.extract_imports_from_source(
            "import com.acme.Money as M\n", "kotlin", "A.kt"
        )

        assert statement.bound_names() == (("Money", "M"),)

    def test_a_selector_list_binds_every_name_it_lists(self, extractor):
        # Scala puts the package on the path and the members in braces, so the
        # last segment is the package and the names are somewhere else.
        if "scala" not in grammars.warmed_languages():
            pytest.skip("grammar for scala is not in the local pack cache")
        (statement,) = extractor.extract_imports_from_source(
            "import pricing.{Money, Tax}\n", "scala", "A.scala"
        )

        assert statement.module == "pricing"
        assert statement.names == ("Money", "Tax")


class TestANamedImportIsACaller:
    """The tier the binding shape decides, end to end.

    A bare `Money` in a file that imports `com.acme.Money` resolved at
    `resolved-via-import` before the whole-module evidence arm was narrowed,
    and at `unique` afterwards, because Java's row never replaced the arm it
    lost. `unique` means only that the repository spells the name once.
    """

    def test_a_java_named_import_resolves_at_the_imported_tier(self, tmp_path, extractor):
        if "java" not in grammars.warmed_languages():
            pytest.skip("grammar for java is not in the local pack cache")
        files = {
            "com/acme/Money.java": (
                "package com.acme;\npublic class Money {\n"
                "    public static int of(int v) { return v; }\n}\n"
            ),
            "App.java": (
                "import com.acme.Money;\n\npublic class App {\n"
                "    public int run() {\n        return Money.of(3);\n    }\n}\n"
            ),
        }
        for relative, text in files.items():
            (tmp_path / relative).parent.mkdir(parents=True, exist_ok=True)
            (tmp_path / relative).write_text(text, encoding="utf-8")

        scan = refs.scan_repo(tmp_path, extractor)
        _, resolved = resolve.resolve_repo(scan, refs.build_ref_index(scan))
        tiers = {
            edge.tier
            for edge in resolved.edges
            if edge.name == "Money" and edge.relation is resolve.Relation.REFERENCES
        }

        assert tiers == {resolve.Tier.IMPORTED}
