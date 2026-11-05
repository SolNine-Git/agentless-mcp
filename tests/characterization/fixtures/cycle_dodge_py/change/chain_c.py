"""Measures: report gains a local import of chain_a (positive, new, a 3-file chain)."""


def measure():
    return 3


def report():
    from chain_a import render

    return render()
