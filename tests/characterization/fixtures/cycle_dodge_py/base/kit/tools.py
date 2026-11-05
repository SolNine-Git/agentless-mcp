"""Tools: import kit.parts when they load."""

from .parts import Part


class Tool:
    def __init__(self, part):
        self.part = part


def spare():
    return Tool(Part())
