"""How many of each catalogue product are held."""

from catalog.product import Product

LOW_STOCK_LEVEL = 12


class StockLevels:
    """Held quantities keyed by product code."""

    def __init__(self) -> None:
        """Start with nothing held."""
        self._held: dict[str, int] = {}

    def record(self, product: Product, quantity: int) -> None:
        """Record the quantity held of one product."""
        if quantity < 0:
            message = f"{product.code} cannot hold a negative quantity"
            raise ValueError(message)
        self._held[product.code] = quantity

    def held(self, product: Product) -> int:
        """Return the quantity held, refusing an unrecorded product."""
        try:
            return self._held[product.code]
        except KeyError as exc:
            message = f"{product.code} has no recorded stock level"
            raise LookupError(message) from exc

    def is_low(self, product: Product) -> bool:
        """Say whether a product has fallen under the low-stock level."""
        return self.held(product) < LOW_STOCK_LEVEL
