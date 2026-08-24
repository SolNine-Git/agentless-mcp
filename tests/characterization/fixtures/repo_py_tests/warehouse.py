"""The warehouse floor: what is stocked and how it ships."""

from inventory import Inventory
from shipping import shipment_plan

FLOOR_LABEL = "floor"


def floor_summary(inventory: Inventory) -> str:
    """One line describing the floor's outstanding work."""
    plan = shipment_plan(inventory)
    crates = sum(count for _sku, count in plan)
    return f"{FLOOR_LABEL} {inventory.warehouse}: {len(plan)} SKUs, {crates} crates"


def is_busy(inventory: Inventory) -> bool:
    """True when the floor has anything to pack."""
    return bool(shipment_plan(inventory))
