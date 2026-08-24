"""The published product list, priced at a tier."""

from pricing import PriceBook, Tier, format_money

CATALOG_TITLE = "Warehouse catalogue"


def catalogue_lines(prices: PriceBook, tier: Tier) -> list[str]:
    """Render one line per SKU at the given tier."""
    lines = [CATALOG_TITLE]
    for sku, price in prices.price_list(tier):
        lines.append(f"{sku.ljust(16)}{format_money(price)}")
    return lines


def headline_sku(prices: PriceBook, tier: Tier) -> str:
    """The SKU the catalogue leads with."""
    return prices.cheapest(tier)
