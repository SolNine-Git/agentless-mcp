"""Network: fetch gains a local import of a module outside the repository (negative)."""


def base():
    return 0


def fetch():
    import requests

    return requests.get
