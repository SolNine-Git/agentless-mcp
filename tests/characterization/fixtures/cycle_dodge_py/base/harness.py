"""The harness imports the test support when it loads."""

import tests.support


def run():
    return tests.support.ready()
