"""Nested function shapes: a closure and a callback."""

from functools import reduce


def make_adder(step):
    def add(value):
        return value + step

    return add


def total(values):
    return reduce(lambda carried, value: carried + value, values, 0)
