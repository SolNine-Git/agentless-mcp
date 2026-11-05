"""Plain: use gains a local import of plain_b, which never imports plain_a (negative)."""


def base():
    return 0


def use():
    import plain_b

    return plain_b.helper()
