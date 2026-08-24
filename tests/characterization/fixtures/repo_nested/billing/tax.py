"""Tax rates and the arithmetic that applies them."""

from decimal import Decimal

from billing.money import round_amount

STANDARD_RATE = Decimal("0.20")
REDUCED_RATE = Decimal("0.05")

RATES_BY_BAND = {
    "standard": STANDARD_RATE,
    "reduced": REDUCED_RATE,
    "zero": Decimal("0.00"),
}


def rate_for_band(band: str) -> Decimal:
    """Return the tax rate for a band, refusing an unknown one."""
    try:
        return RATES_BY_BAND[band]
    except KeyError as exc:
        message = f"no tax rate is defined for band {band}"
        raise LookupError(message) from exc


def tax_on(net: Decimal, band: str) -> Decimal:
    """Return the tax due on a net amount in one band."""
    if net < 0:
        message = "a negative net amount cannot be taxed"
        raise ValueError(message)
    return round_amount(net * rate_for_band(band))
