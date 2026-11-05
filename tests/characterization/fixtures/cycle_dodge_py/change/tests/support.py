"""Test support: make gains a local import of harness (negative, a test path)."""


def ready():
    return True


def make():
    import harness

    return harness.run()
