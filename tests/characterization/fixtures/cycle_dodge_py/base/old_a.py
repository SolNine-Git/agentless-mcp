"""Old: fetch has closed a cycle through old_b since the base; the change adds a function
above it (negative)."""


def fetch():
    import old_b

    return old_b.get()
