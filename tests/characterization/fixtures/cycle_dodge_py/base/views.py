"""Views: import the shop package when they load."""

from shop import Item


def show(item):
    return f"<{item.name}>"


def blank():
    return show(Item(""))
