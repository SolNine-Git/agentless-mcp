"""The installed package version: one lookup and one fallback for every surface that reports it."""

from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as distribution_version

DISTRIBUTION_NAME = "agentless-mcp"
UNKNOWN_VERSION = "0+unknown"


def package_version() -> str:
    """Return the installed distribution's version, or ``UNKNOWN_VERSION`` outside an install."""
    try:
        return distribution_version(DISTRIBUTION_NAME)
    except PackageNotFoundError:
        return UNKNOWN_VERSION
