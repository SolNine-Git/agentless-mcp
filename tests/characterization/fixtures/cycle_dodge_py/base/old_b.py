"""Gets: imports old_a when it loads."""

import old_a


def get():
    return old_a.__name__
