"""Comment shapes the skeletonizer used to leak into its own output."""

# A whole-line comment at module level.
TAX_RATE = 0.2  # a trailing comment on a module-level constant


def first_line_comment(amount):
    total = amount * TAX_RATE  # the comment that used to overwrite the sentinel
    return total * 2


def later_line_comment(amount):
    total = amount
    # a whole-line comment inside a body
    return total * TAX_RATE  # a trailing comment on the last body line


class Ledger:
    RATE = 1  # a trailing comment on a class attribute

    def post(self, item):  # a trailing comment on a signature line
        checked = item  # and one inside the body it introduces
        return checked
