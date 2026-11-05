"""Models: Item.render gains a local import of views (positive, new, through shop/__init__.py)."""


class Item:
    def __init__(self, name):
        self.name = name
