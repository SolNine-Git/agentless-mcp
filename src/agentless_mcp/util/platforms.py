"""Which operating-system family this process is running on.

Two places in the package have to behave differently on Windows -- the index
write lock and the bounded test runner's process-group kill -- and both were
POSIX-only until Phase 4. Naming the family once, as a pure function of a
platform string, is what lets each of them be unit-tested on either platform
without pretending to be the other: the dispatch is testable, the platform
calls themselves are not, and no amount of monkeypatching would change that.

``win32`` is what CPython reports on every Windows build, 64-bit included.
``cygwin`` reports itself as such and is POSIX, which is why the test is a
prefix match on ``win`` rather than an equality against a list.
"""

POSIX = "posix"
WINDOWS = "windows"


def family(platform: str) -> str:
    """Return the OS family ``platform`` belongs to."""
    return WINDOWS if platform.startswith("win") else POSIX
