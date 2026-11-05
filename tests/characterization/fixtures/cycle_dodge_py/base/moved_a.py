"""Totals: the module-level import of moved_b moves into total (positive, moved)."""

import moved_b


def total():
    return moved_b.count() + 1


def size():
    return 2
