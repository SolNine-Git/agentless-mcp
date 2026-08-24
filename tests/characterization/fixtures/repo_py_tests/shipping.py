"""Packing the reorder list into shipments."""

from inventory import Inventory, Item

MAX_PER_CRATE = 24


def crates_for(item: Item) -> int:
    """How many crates one item's quantity needs."""
    whole, remainder = divmod(item.quantity, MAX_PER_CRATE)
    return whole + (1 if remainder else 0)


def shipment_plan(inventory: Inventory) -> list[tuple[str, int]]:
    """One (SKU, crate count) pair per item due for reorder."""
    return [(item.sku, crates_for(item)) for item in inventory.reorder_list()]
