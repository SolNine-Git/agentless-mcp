"""Layout: imports chain_c when it loads."""

import chain_c


def layout():
    return chain_c.measure()
