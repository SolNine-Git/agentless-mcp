"""The command surface that drives the modules above."""

from audit import audit_lines
from catalog import catalogue_lines
from customers import Customer
from inventory import Inventory
from orders import raise_order
from pricing import PriceBook, Tier
from warehouse import floor_summary


def run_cli(inventory: Inventory, prices: PriceBook, command: str) -> str:
    """Dispatch one command against the warehouse."""
    if command == "catalogue":
        return "\n".join(catalogue_lines(prices, Tier.TRADE))
    if command == "floor":
        return floor_summary(inventory)
    if command == "order":
        return raise_order(inventory, prices, 1)
    if command == "audit":
        return "\n".join(audit_lines(inventory, prices, Customer("acme")))
    message = f"unknown command {command}"
    raise ValueError(message)
