"""Tests for pricing, the reports that quote it, and the stock behind both.

The point of this fixture file is the edge direction. Every reference below
runs from here into the production modules and nothing runs back, so the
ranking that scores inbound weight cannot place this file however directly it
exercises the code above it.
"""

from decimal import Decimal

from inventory import Inventory, Item
from pricing import PriceBook, Tier, format_money
from reports import reorder_report


def build_book() -> PriceBook:
    """A price book with two SKUs, one of them cheap."""
    return PriceBook({"bolt-m6": Decimal("0.40"), "lathe-7": Decimal("980.00")})


def test_a_retail_quote_takes_the_markup_and_no_discount():
    book = build_book()

    assert book.quote("bolt-m6", Tier.RETAIL) == Decimal("0.54")


def test_a_wholesale_quote_comes_in_under_a_retail_one():
    book = build_book()

    assert book.quote("lathe-7", Tier.WHOLESALE) < book.quote("lathe-7", Tier.RETAIL)


def test_money_renders_with_the_configured_currency():
    assert format_money(Decimal("1.005")) == "1.01 EUR"


def test_an_empty_warehouse_has_nothing_to_reorder():
    stock = Inventory("main")
    stock.add(Item(sku="bolt-m6", category="fastener", quantity=900))

    assert reorder_report(stock, build_book()) == "nothing to reorder\n"
