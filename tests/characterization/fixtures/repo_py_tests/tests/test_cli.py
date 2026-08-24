"""Tests for the command surface.

One command and the smallest price book it will accept: enough to exercise
the dispatch and no more.
"""

from decimal import Decimal

from cli import run_cli
from pricing import PriceBook


def test_an_unknown_command_is_refused_rather_than_ignored():
    prices = PriceBook({"bolt-m6": Decimal("0.40")})

    try:
        run_cli(None, prices, "wat")
    except ValueError as exc:
        assert "unknown command" in str(exc)
