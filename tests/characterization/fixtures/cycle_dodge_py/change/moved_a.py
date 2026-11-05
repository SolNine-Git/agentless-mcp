"""Totals: the module-level import of moved_b moves into total (positive, moved)."""


def total():
    import moved_b

    return moved_b.count() + 1


def size():
    return 2
