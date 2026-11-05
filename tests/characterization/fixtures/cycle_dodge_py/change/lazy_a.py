"""Lazy: use gains a local import of lazy_b, which imports lazy_a only in a function (negative)."""


def base():
    return 0


def use():
    import lazy_b

    return lazy_b.back()
