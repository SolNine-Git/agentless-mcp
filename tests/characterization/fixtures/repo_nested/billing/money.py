"""Amount arithmetic and formatting for the billing package."""

from decimal import ROUND_HALF_UP, Decimal

BILLING_CURRENCY = "GBP"
BILLING_PRECISION = Decimal("0.01")


def round_amount(amount: Decimal) -> Decimal:
    """Round an amount to the billing precision, half away from zero."""
    return amount.quantize(BILLING_PRECISION, rounding=ROUND_HALF_UP)


def format_amount(amount: Decimal) -> str:
    """Render an amount with the billing currency suffix."""
    return f"{round_amount(amount)} {BILLING_CURRENCY}"


def sum_amounts(amounts: list[Decimal]) -> Decimal:
    """Total a list of amounts, rounding once at the end."""
    running = Decimal("0.00")
    for amount in amounts:
        running += amount
    return round_amount(running)
