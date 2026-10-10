"""Branch, loop, comprehension, and match counting."""

import sys


def route(kind, flag):
    if kind == "a" and flag:
        return 1
    elif kind == "b" or not flag:
        return 2
    elif kind == "c":
        return 3
    else:
        return 4


def loops(items):
    for item in items:
        if item:
            break
    else:
        return None
    while items:
        items.pop()
    else:
        pass
    assert items is not None, "items"
    return 0 if items else 1


def handle(job):
    try:
        job.run()
    except (ValueError, KeyError):
        return 1
    except OSError as exc:
        job.log(exc)
    else:
        return 3
    finally:
        job.close()
    try:
        job.retry()
    except* RuntimeError:
        job.log("group")


def shapes(rows, flag):
    evens = [r for r in rows if r % 2 == 0 if r]
    pairs = {r: c for r in rows for c in r if c}
    unique = {r for r in rows}
    total = sum(r for r in rows if flag or r)
    return evens, pairs, unique, total


def dispatch(command):
    match command:
        case ["go", where] if where:
            return where
        case ["stop"] | ["halt"]:
            return None
        case {"k": value} if value and command:
            return value
        case _:
            return lambda x: x if x else None


def busy(a, b, c, d):
    if a:
        if b and c:
            return 1
        elif c or d:
            return 2
    for x in a:
        while x and b:
            x -= 1
    result = [y for y in b if y] or [z for z in c if z and d]
    try:
        return result if result else None
    except Exception:
        return None


identity = lambda v: v if v else None

if sys.version_info >= (3, 12):
    pass
