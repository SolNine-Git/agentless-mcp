"""Turning a quote into an invoice line."""

from decimal import Decimal

from customers import Customer, tier_of
from pricing import PriceBook, format_money


def invoice_line(prices: PriceBook, customer: Customer, sku: str, quantity: int) -> str:
    """Render one invoice line for a customer's order."""
    total = prices.quote(sku, tier_of(customer), quantity)
    return f"{sku} x{quantity}  {format_money(total)}"


def invoice_total(prices: PriceBook, customer: Customer, skus: list[str]) -> Decimal:
    """Total one invoice across several SKUs."""
    running = Decimal("0.00")
    for sku in skus:
        running += prices.quote(sku, tier_of(customer))
    return running
