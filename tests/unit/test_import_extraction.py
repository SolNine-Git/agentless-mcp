"""Tests for tree-sitter import extraction.

Ported from mcp-local's ``tests/unit/test_import_extraction.py``; only the
module paths changed. See ``test_extractor.py`` for the grammar-drift note
that applies to the whole port.
"""

import pytest

from agentless_mcp.core.extractor import TreeSitterExtractor


@pytest.fixture
def extractor():
    return TreeSitterExtractor()


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
