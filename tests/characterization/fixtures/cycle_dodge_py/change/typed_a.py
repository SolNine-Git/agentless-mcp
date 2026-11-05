"""Typed: check gains a local import of typed_b, which imports typed_a only for a type checker
(negative)."""


def base():
    return 0


def check():
    import typed_b

    return typed_b.validate(0)
