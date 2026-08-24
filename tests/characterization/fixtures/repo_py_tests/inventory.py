"""Inventory tracking for a small warehouse.

Deliberately ordinary code: the skeletonizer is only interesting when the
input looks like something someone would actually commit, with real bodies
rather than one-liners.
"""

import json
from dataclasses import dataclass
from pathlib import Path

MAX_ITEMS = 500
DEFAULT_WAREHOUSE = "main"

# Reorder thresholds are per-category because bolts and lathes do not move
# at remotely the same rate.
REORDER_THRESHOLDS = {
    "fastener": 250,
    "tool": 5,
    "consumable": 40,
}


@dataclass(frozen=True)
class Item:
    """A single stocked item."""

    sku: str
    category: str
    quantity: int

    def needs_reorder(self) -> bool:
        """True when the item has fallen under its category threshold."""
        threshold = REORDER_THRESHOLDS.get(self.category)
        if threshold is None:
            # An unknown category is a data error, not a reason to reorder.
            return False
        if self.quantity < 0:
            message = f"{self.sku} has a negative quantity"
            raise ValueError(message)
        return self.quantity < threshold


class Inventory:
    """The stock held in one warehouse."""

    warehouse: str = DEFAULT_WAREHOUSE

    def __init__(self, warehouse: str = DEFAULT_WAREHOUSE) -> None:
        """Start an empty inventory for a warehouse."""
        if not warehouse:
            message = "warehouse name must not be empty"
            raise ValueError(message)
        self.warehouse = warehouse
        self._items: dict[str, Item] = {}
        self._history: list[tuple[str, str]] = []

    def add(self, item: Item) -> None:
        """Add an item, refusing to exceed the per-warehouse ceiling."""
        if len(self._items) >= MAX_ITEMS:
            message = f"{self.warehouse} already holds {MAX_ITEMS} items"
            raise ValueError(message)
        if item.sku in self._items:
            existing = self._items[item.sku]
            merged = Item(
                sku=item.sku,
                category=item.category,
                quantity=existing.quantity + item.quantity,
            )
            self._items[item.sku] = merged
            self._history.append(("merge", item.sku))
            return
        self._items[item.sku] = item
        self._history.append(("add", item.sku))

    def remove(self, sku: str) -> Item:
        """Remove and return an item by SKU."""
        # Popping with a default would turn a typo into a silent no-op.
        try:
            item = self._items.pop(sku)
        except KeyError as exc:
            message = f"{sku} is not stocked in {self.warehouse}"
            raise KeyError(message) from exc
        self._history.append(("remove", sku))
        return item

    def reorder_list(self) -> list[Item]:
        """Return every item that has fallen under its threshold."""
        due: list[Item] = []
        for item in self._items.values():
            if item.needs_reorder():
                due.append(item)
        due.sort(key=lambda item: item.sku)
        return due

    def audit_trail(self) -> list[str]:
        """Render the mutation history oldest first."""
        rendered: list[str] = []
        for index, (action, sku) in enumerate(self._history, start=1):
            rendered.append(f"{index:03d} {action:<6} {sku}")
        return rendered


def load_inventory(path: Path) -> Inventory:
    """Read an inventory from a JSON file written by ``dump_inventory``."""
    payload = json.loads(path.read_text(encoding="utf-8"))
    inventory = Inventory(payload["warehouse"])
    for row in payload["items"]:
        item = Item(
            sku=row["sku"],
            category=row["category"],
            quantity=int(row["quantity"]),
        )
        inventory.add(item)
    return inventory


def dump_inventory(inventory: Inventory, path: Path) -> None:
    """Write an inventory back out as JSON.

    The output is sorted and indented so a diff between two dumps is readable
    by a human reviewing a stock movement, which matters more here than the
    handful of bytes compact separators would save. Only the items due for
    reorder are written: this dump feeds the purchasing spreadsheet, not a
    backup, and a full snapshot lives in the warehouse database instead.
    """
    payload = {
        "warehouse": inventory.warehouse,
        "items": [
            {"sku": item.sku, "category": item.category, "quantity": item.quantity}
            for item in inventory.reorder_list()
        ],
    }
    path.write_text(json.dumps(payload, indent=2, sort_keys=True), encoding="utf-8")
