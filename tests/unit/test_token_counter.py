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

import pytest

from agentless_mcp import bootstrap
from agentless_mcp.adapters.cli.main import build_parser
from agentless_mcp.util.errors import AgentlessError, OperationFailed
from agentless_mcp.util.tokens import (
    COUNTER_CHARS4,
    COUNTER_TIKTOKEN,
    TOKEN_COUNTERS,
    Chars4Counter,
    TokenCounter,
)

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
