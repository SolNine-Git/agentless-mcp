"""Reporting helpers that stitch inventory and pricing together."""

from decimal import Decimal

from inventory import Inventory, Item
from pricing import PriceBook, Tier, format_money

REPORT_WIDTH = 72
SEPARATOR = "-" * REPORT_WIDTH


class ReportError(Exception):
    """Raised when a report cannot be produced from the data given."""


def stock_value(inventory: Inventory, prices: PriceBook, tier: Tier) -> Decimal:
    """Total the quoted value of everything due for reorder."""
    total = Decimal("0.00")
    for item in inventory.reorder_list():
        try:
            line_total = prices.quote(item.sku, tier, item.quantity)
        except LookupError as exc:
            message = f"cannot value {item.sku}: {exc}"
            raise ReportError(message) from exc
        total += line_total
    return total


def reorder_report(inventory: Inventory, prices: PriceBook) -> str:
    """Render a plain-text reorder report."""
    rows = inventory.reorder_list()
    if not rows:
        return "nothing to reorder\n"

    lines = [f"reorder report for {inventory.warehouse}", SEPARATOR]
    running = Decimal("0.00")
    for item in rows:
        # Trade pricing is what the purchasing team actually pays.
        line_total = prices.quote(item.sku, Tier.TRADE, item.quantity)
        running += line_total
        lines.append(_format_row(item, line_total))
    lines.append(SEPARATOR)
    lines.append(f"{'total'.ljust(22)}{format_money(running)}")
    return "\n".join(lines) + "\n"


def _format_row(item: Item, line_total: Decimal) -> str:
    """Format one report row, left-aligning the SKU."""
    sku = item.sku.ljust(16)
    quantity = str(item.quantity).rjust(6)
    money = format_money(line_total)
    return f"{sku}{quantity}  {money}"


def check_report_inputs(inventory: Inventory, prices: PriceBook) -> None:
    """Fail loudly when a report would silently under-report."""
    missing: list[str] = []
    for item in inventory.reorder_list():
        try:
            prices.cost_of(item.sku)
        except LookupError:
            missing.append(item.sku)
    if missing:
        listed = ", ".join(sorted(missing))
        message = f"no cost price for {len(missing)} SKUs: {listed}"
        raise ReportError(message)


def summarise(inventory: Inventory, prices: PriceBook) -> dict[str, str]:
    """Return a small dictionary suitable for a status endpoint."""
    rows = inventory.reorder_list()
    summary = {
        "warehouse": inventory.warehouse,
        "due": str(len(rows)),
        "value": format_money(stock_value(inventory, prices, Tier.TRADE)),
    }
    if rows:
        summary["first_sku"] = rows[0].sku
    return summary
