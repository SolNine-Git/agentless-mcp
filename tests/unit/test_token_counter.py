"""Token-counter selection at the composition root.

The estimator is a contract, not an implementation detail: every budget in
this package is expressed in its units and the map's token pins were measured
with it. So the tests here are about *selection* -- what a process ends up
holding -- rather than about how well tiktoken counts.

The tiktoken half runs only where the ``tokens`` extra is installed, and skips
with the reason where it is not. Both directions are covered: with the extra
absent, asking for it must produce an actionable refusal rather than a
traceback.
"""

import importlib.util
from pathlib import Path

import pytest

from agentless_mcp import bootstrap
from agentless_mcp.adapters.cli.main import build_parser
from agentless_mcp.prompts import TOOL_DESCRIPTIONS
from agentless_mcp.util.errors import AgentlessError, OperationFailed
from agentless_mcp.util.tokens import (
    COUNTER_CHARS4,
    COUNTER_TIKTOKEN,
    TOKEN_COUNTERS,
    Chars4Counter,
    TokenCounter,
)

ROOT = Path(__file__).parents[2]

TIKTOKEN_INSTALLED = importlib.util.find_spec("tiktoken") is not None

SAMPLE = "def quote(sku):\n    return RATE\n"


class TestSelection:
    def test_no_choice_is_the_chars4_estimator(self):
        assert isinstance(bootstrap.select_counter(None), Chars4Counter)

    def test_the_chars4_name_selects_the_estimator(self):
        assert isinstance(bootstrap.select_counter(COUNTER_CHARS4), Chars4Counter)

    def test_an_unknown_name_is_refused_by_name(self):
        # argparse rejects an unknown value before this is reached, so only a
        # library caller arrives here. Answering them with the default would
        # estimate every budget with a counter they did not ask for.
        with pytest.raises(OperationFailed) as raised:
            bootstrap.select_counter("nonsense")
        message = str(raised.value)
        assert "nonsense" in message
        assert all(name in message for name in TOKEN_COUNTERS)

    def test_the_flag_is_read_out_of_an_argv_before_the_subcommand_parse(self):
        assert bootstrap.counter_choice(["map", "--repo", "/tmp"]) is None
        assert (
            bootstrap.counter_choice(["--token-counter", COUNTER_TIKTOKEN, "map"])
            == COUNTER_TIKTOKEN
        )

    def test_the_full_parser_accepts_the_same_flag(self):
        args = build_parser().parse_args(["--token-counter", COUNTER_CHARS4, "map"])
        assert args.token_counter == COUNTER_CHARS4


class TestProtocol:
    def test_the_default_counter_satisfies_the_protocol(self):
        assert isinstance(Chars4Counter(), TokenCounter)

    def test_the_default_counter_is_the_documented_estimate(self):
        assert Chars4Counter().count(SAMPLE) == len(SAMPLE) // 4


@pytest.mark.skipif(TIKTOKEN_INSTALLED, reason="the tokens extra is installed")
class TestWithoutTheExtra:
    def test_asking_for_tiktoken_refuses_with_the_install_command(self):
        with pytest.raises(AgentlessError, match="tokens' extra"):
            bootstrap.select_counter(COUNTER_TIKTOKEN)


@pytest.mark.skipif(not TIKTOKEN_INSTALLED, reason="the tokens extra is not installed")
class TestWithTheExtra:
    def test_the_tiktoken_counter_satisfies_the_protocol(self):
        counter = bootstrap.select_counter(COUNTER_TIKTOKEN)
        assert isinstance(counter, TokenCounter)
        assert isinstance(counter, bootstrap.TiktokenCounter)

    def test_it_counts_a_positive_number_of_tokens(self):
        assert bootstrap.select_counter(COUNTER_TIKTOKEN).count(SAMPLE) > 0

    def test_the_default_is_still_chars4_with_the_extra_installed(self):
        # The pins move only when a caller asks for them to.
        assert isinstance(bootstrap.select_counter(None), Chars4Counter)


@pytest.mark.skipif(not TIKTOKEN_INSTALLED, reason="the tokens extra is not installed")
class TestTheEstimatorAgainstARealTokenizer:
    """How far chars/4 misses a real BPE count, pinned on committed output.

    The gap is documented in `Chars4Counter`, in the agent guide and in two
    tool descriptions, and it had already been stated wrongly twice: once as
    11-18% and once as 13-15%, when the measured spread across these fixtures
    is wider than either and runs in both directions. Prose cannot hold a
    number that moves whenever the renderer changes, so this is the gate that
    holds it instead.

    The goldens are the input because they are committed, deterministic and
    already regenerated whenever the rendering changes -- the same property
    that makes them goldens makes them the honest sample here.
    """

    # Widest ratio the map goldens may span before the documented figures are
    # stale. Measured 2026-08-25: 1.170 (repo_ts) to 1.264 (repo_py). The pin
    # is deliberately looser than the measurement, because it is guarding a
    # docstring rather than a budget -- a rendering tweak that moves density a
    # little should not fail, and one that changes it in kind should.
    MAP_RATIO_FLOOR = 1.10
    MAP_RATIO_CEILING = 1.35

    def map_goldens(self):
        root = ROOT / "tests" / "characterization" / "goldens" / "map"
        found = sorted(root.glob("*.map.txt"))
        assert found, f"no map goldens under {root}"
        return found

    def test_the_map_goldens_stay_inside_the_documented_ratio(self):
        counter = bootstrap.select_counter(COUNTER_TIKTOKEN)
        for path in self.map_goldens():
            text = path.read_text(encoding="utf-8")
            estimated = Chars4Counter().count(text)
            assert estimated > 0, path.name
            ratio = counter.count(text) / estimated
            assert self.MAP_RATIO_FLOOR <= ratio <= self.MAP_RATIO_CEILING, (
                f"{path.name}: real/estimated is {ratio:.3f}, outside the "
                f"{self.MAP_RATIO_FLOOR}-{self.MAP_RATIO_CEILING} this package documents. "
                "Re-measure the figures in util/tokens.py, the agent guide and the "
                "repo_map and orient tool descriptions, then move this pin."
            )

    def test_the_published_band_is_the_band_under_test(self):
        """The number an agent reads at call time is the number this pins.

        The figure had been stated wrongly twice in prose -- 11-18%, then
        13-15% -- and a third time as "roughly 15-20%", which the guide's own
        measured table contradicted on two rows. Prose cannot hold a number
        that moves whenever the renderer does, and a band nothing checks is
        the same defect wearing a percent sign. So the tool descriptions
        publish the band this class enforces, and this is the join.

        Deliberately a string match on the descriptions rather than a
        recomputation: what ships to a model is the sentence, and a sentence
        is what goes stale.
        """
        low = round((self.MAP_RATIO_FLOOR - 1) * 100)
        high = round((self.MAP_RATIO_CEILING - 1) * 100)
        published = f"a real BPE count runs {low}-{high}% higher"

        for key in ("repo_map", "orient"):
            assert published in TOOL_DESCRIPTIONS[key], (
                f"{key} no longer publishes the {published!r} this test pins. "
                "Move both together or move neither."
            )

    def test_the_estimator_is_not_uniformly_low(self):
        """The docstring claims the direction varies by view; this is why.

        Stated as one-directional, the guidance would be "scale by 1.2 and you
        are safe", which is wrong for `lint` output -- there the estimator
        counts over, so scaling up over-reserves. Pinned so the claim in
        `Chars4Counter` cannot quietly become false.
        """
        counter = bootstrap.select_counter(COUNTER_TIKTOKEN)
        goldens = ROOT / "tests" / "characterization" / "goldens"
        ratios = {}
        for path in sorted(goldens.rglob("*.txt")):
            text = path.read_text(encoding="utf-8")
            estimated = Chars4Counter().count(text)
            if estimated:
                ratios[path.name] = counter.count(text) / estimated

        assert ratios, "no goldens rendered any text"
        assert min(ratios.values()) < 1.0, (
            "every golden now estimates low; the documented 0.979-1.264 spread "
            f"and its both-directions claim are stale (min {min(ratios.values()):.3f})"
        )
        assert max(ratios.values()) > 1.15, (
            "no golden estimates materially low any more; the id-density figures "
            f"are stale (max {max(ratios.values()):.3f})"
        )
