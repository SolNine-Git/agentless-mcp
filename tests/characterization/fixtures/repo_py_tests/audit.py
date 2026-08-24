"""A read-only audit over the orders and invoices raised."""

from billing import invoice_total
from customers import Customer
from inventory import Inventory
from orders import order_count
from pricing import PriceBook

AUDIT_HEADER = "audit"


def audit_lines(inventory: Inventory, prices: PriceBook, customer: Customer) -> list[str]:
    """One line per audited fact."""
    skus = [item.sku for item in inventory.reorder_list()]
    return [
        AUDIT_HEADER,
        f"orders: {order_count(inventory)}",
        f"exposure: {invoice_total(prices, customer, skus)}",
    ]
