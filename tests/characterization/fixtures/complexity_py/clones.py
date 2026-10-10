"""Type-1 and Type-2 clones, with the clone-normalization quirks."""

from typing import TYPE_CHECKING


def total_a(items):
    """Sum the items."""  # trailing comment
    total = 0
    for item in items:
        total += item
    return total


def total_b(values):
    total = 0
    for value in values:
        total += value
    return total


def scale_a(rows):
    out = []
    for row in rows:
        out.append(row * 2)
    return out


def scale_b(cols):
    acc = []
    for col in cols:
        acc.append(col * 3)
    return acc


def first(data):
    for entry in data:
        if entry:
            yield entry


def second(data):
    for entry in data:
        if entry:
            yield entry


if TYPE_CHECKING:
    def typed_a(items):
        if items:
            return items
        return None


def typed_b(values):
    if values:
        return values
    return None
