"""Python complexity matches scb-check v0.1.3 on the parity fixtures.

The goldens come from scripts/scb_parity_goldens.py, which runs scb-check under
uvx. Regenerate them deliberately: predict the diff, run the script, read the diff.
"""

import json
import sys
from collections import Counter
from pathlib import Path

import pytest

from agentless_mcp.core.complexity import erosion, measure
from agentless_mcp.util.errors import UnmeasurableSource

if sys.version_info < (3, 12):
    pytest.skip(
        "parity with scb-check v0.1.3 is defined on Python 3.12 and later",
        allow_module_level=True,
    )

HERE = Path(__file__).parent
FIXTURES = HERE / "fixtures" / "complexity_py"
GOLDENS = HERE / "goldens" / "complexity"
TOLERANCE = 1e-9


def load(stem):
    return json.loads((GOLDENS / f"{stem}.json").read_text(encoding="utf-8"))


def measured(stem):
    path = FIXTURES / f"{stem}.py"
    return measure(path.read_text(encoding="utf-8"), path.name, "python")


STEMS = sorted(path.stem for path in GOLDENS.glob("*.json"))
MEASURED = [stem for stem in STEMS if load(stem)["status"] == "ok"]
REJECTED = [stem for stem in STEMS if load(stem)["status"] != "ok"]


def test_every_fixture_has_a_golden_and_every_golden_a_fixture():
    assert sorted(path.stem for path in FIXTURES.glob("*.py")) == STEMS


@pytest.mark.parametrize("stem", STEMS)
def test_the_golden_comes_from_the_pinned_reference(stem):
    golden = load(stem)
    assert golden["reference"] == "scb-check 0.1.3"
    assert tuple(int(part) for part in golden["python"].split(".")) >= (3, 12)
    assert golden["fixture"] == f"tests/characterization/fixtures/complexity_py/{stem}.py"


@pytest.mark.parametrize("stem", MEASURED)
class TestParity:
    def test_the_unit_inventories_are_equal_both_ways(self, stem):
        golden = Counter((unit["name"], unit["start_line"]) for unit in load(stem)["units"])
        ours = Counter(
            (unit.qualname.rsplit(".", 1)[-1], unit.start_line) for unit in measured(stem)
        )
        assert ours == golden

    def test_cc_and_sloc_match_exactly(self, stem):
        golden = Counter(
            (unit["name"], unit["start_line"], unit["cc"], unit["sloc"])
            for unit in load(stem)["units"]
        )
        ours = Counter(
            (unit.qualname.rsplit(".", 1)[-1], unit.start_line, unit.cc, unit.sloc)
            for unit in measured(stem)
        )
        assert ours == golden

    def test_erosion_matches(self, stem):
        assert abs(erosion(measured(stem)) - load(stem)["erosion"]) < TOLERANCE


@pytest.mark.parametrize("stem", REJECTED)
def test_a_file_scb_check_cannot_read_is_a_typed_failure(stem):
    with pytest.raises(UnmeasurableSource):
        measured(stem)
