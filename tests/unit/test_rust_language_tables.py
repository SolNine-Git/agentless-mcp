"""The Rust language tables in crates/syntax/languages/ equal the Python tables they port."""

import importlib.util
import sys
from pathlib import Path
from types import ModuleType

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "syntax_tables.py"


def _script() -> ModuleType:
    spec = importlib.util.spec_from_file_location("syntax_tables", SCRIPT)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_the_committed_tables_are_a_fresh_rendering_of_the_python_tables():
    script = _script()
    stale = script.stale_files(script.rendered())
    assert not stale, f"run scripts/syntax_tables.py: {stale} differ from the Python tables"


def test_every_language_has_one_file():
    script = _script()
    assert len(script.rendered()) == 22
