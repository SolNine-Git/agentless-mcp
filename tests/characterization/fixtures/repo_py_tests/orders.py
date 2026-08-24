"""Purchase orders raised off the reorder report."""

from inventory import Inventory
from pricing import PriceBook
from reports import reorder_report

ORDER_PREFIX = "PO"


def raise_order(inventory: Inventory, prices: PriceBook, number: int) -> str:
    """Render one purchase order document."""
    header = f"{ORDER_PREFIX}-{number:05d}"
    return f"{header}\n{reorder_report(inventory, prices)}"


def order_count(inventory: Inventory) -> int:
    """How many order lines the current reorder list would raise."""
    return len(inventory.reorder_list())
