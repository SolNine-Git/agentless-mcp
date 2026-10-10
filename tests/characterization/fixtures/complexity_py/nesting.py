"""Nesting, classes, decorators, async, and definitions scb-check does not visit."""

import functools
from typing import TYPE_CHECKING


def outer(items):
    def inner(x):
        if x:
            return [y for y in x if y]
        return []

    class Local:
        def method(self, v):
            return v or None

    if items:
        def hidden(z):
            return z and items
        return hidden
    return inner


class Service:
    """Service docstring."""

    class Nested:
        def deep(self, a, b):
            return a if a else b

    @staticmethod
    def build(flag):
        while flag:
            flag = False
        return flag

    @functools.cache
    async def fetch(self, key):
        async for row in self.rows(key):
            if row:
                return row
        return None

    async def rows(self, key):
        yield key


@functools.wraps(outer)
def wrapped(*args):
    return outer(*args) or ()


if TYPE_CHECKING:
    def typed_only(value):
        if value:
            return value
        return None

try:
    def guarded(value):
        return value and True
except ImportError:
    guarded = None

if __name__ == "__main__":
    def main():
        return 0 if outer([]) else 1
