"""Catalogue lookup, the one module that reads across the two packages."""

from decimal import Decimal

from billing.money import format_amount
from catalog.product import Product, parse_product
from catalog.stock import StockLevels

RESULT_LIMIT = 20


def load_catalogue(rows: list[dict[str, str]]) -> list[Product]:
    """Parse every catalogue row, keeping the order the rows arrived in."""
    return [parse_product(row) for row in rows]


def find_by_title(catalogue: list[Product], text: str) -> list[Product]:
    """Return the products whose title contains ``text``, code order."""
    matched = [product for product in catalogue if text.lower() in product.title.lower()]
    matched.sort(key=lambda product: product.code)
    return matched[:RESULT_LIMIT]


def describe(product: Product, levels: StockLevels, price: Decimal) -> str:
    """Render one search result, priced and with its stock level."""
    suffix = " (low stock)" if levels.is_low(product) else ""
    return f"{product.short_title()}  {format_amount(price)}{suffix}"
