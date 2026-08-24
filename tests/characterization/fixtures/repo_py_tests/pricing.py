"""Price calculation with tiered discounts."""

from decimal import ROUND_HALF_UP, Decimal
from enum import Enum

BASE_MARKUP = Decimal("1.35")
CURRENCY = "EUR"
CENTS = Decimal("0.01")


class Tier(str, Enum):
    """Customer pricing tiers."""

    RETAIL = "retail"
    TRADE = "trade"
    WHOLESALE = "wholesale"


DISCOUNTS = {
    Tier.RETAIL: Decimal("0.00"),
    Tier.TRADE: Decimal("0.10"),
    Tier.WHOLESALE: Decimal("0.22"),
}


class PriceBook:
    """Cost prices keyed by SKU, plus the rules that turn them into quotes."""

    def __init__(self, costs: dict[str, Decimal]) -> None:
        """Hold a copy of the cost table so callers cannot mutate it later."""
        self._costs: dict[str, Decimal] = {}
        for sku, cost in costs.items():
            if cost < 0:
                message = f"negative cost price for {sku}"
                raise ValueError(message)
            self._costs[sku] = Decimal(cost)

    def cost_of(self, sku: str) -> Decimal:
        """Return the cost price for a SKU."""
        try:
            return self._costs[sku]
        except KeyError as exc:
            message = f"no cost price recorded for {sku}"
            raise LookupError(message) from exc

    def quote(self, sku: str, tier: Tier, quantity: int = 1) -> Decimal:
        """Quote a line total for a SKU at a tier."""
        if quantity < 1:
            message = "quantity must be at least one"
            raise ValueError(message)
        unit = self.cost_of(sku) * BASE_MARKUP
        # Discounts apply to the marked-up price, not the cost price.
        discount = DISCOUNTS[tier]
        discounted = unit * (Decimal("1.00") - discount)
        line_total = discounted * quantity
        return line_total.quantize(CENTS, rounding=ROUND_HALF_UP)

    def price_list(self, tier: Tier) -> list[tuple[str, Decimal]]:
        """Return every SKU with its unit price at a tier, SKU order."""
        rows: list[tuple[str, Decimal]] = []
        for sku in sorted(self._costs):
            rows.append((sku, self.quote(sku, tier)))
        return rows

    def cheapest(self, tier: Tier) -> str:
        """Return the SKU with the lowest unit price at a tier."""
        rows = self.price_list(tier)
        if not rows:
            message = "price book is empty"
            raise LookupError(message)
        best_sku, best_price = rows[0]
        for sku, price in rows[1:]:
            if price < best_price:
                best_sku, best_price = sku, price
        return best_sku


def format_money(amount: Decimal) -> str:
    """Render an amount with the configured currency suffix."""
    quantized = amount.quantize(CENTS, rounding=ROUND_HALF_UP)
    return f"{quantized} {CURRENCY}"
