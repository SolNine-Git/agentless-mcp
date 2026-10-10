"""scb-check's tokenizer rejects this file: a line separator inside a one-line string."""

LABEL = "left right"


def unreachable(a):
    return a or 1
