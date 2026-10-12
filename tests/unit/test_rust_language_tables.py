"""The Rust language tables in crates/syntax/languages/ equal the Python tables they port."""

import importlib
import importlib.util
import sys
from pathlib import Path
from types import ModuleType

import pytest

SCRIPTS = Path(__file__).resolve().parents[2] / "scripts"


@pytest.fixture
def script(monkeypatch: pytest.MonkeyPatch) -> ModuleType:
    # The script imports fixture_files from its own directory.
    monkeypatch.syspath_prepend(SCRIPTS)
    spec = importlib.util.spec_from_file_location("syntax_tables", SCRIPTS / "syntax_tables.py")
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_the_committed_tables_are_a_fresh_rendering_of_the_python_tables(script: ModuleType):
    differences = importlib.import_module("fixture_files").differences(script.contents())
    assert not differences, f"run scripts/syntax_tables.py: {differences}"


def test_every_language_has_one_file(script: ModuleType):
    assert len(script.contents()) == 22
