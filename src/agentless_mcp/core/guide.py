"""The agent usage guide that ships inside this package, and its sections.

The guide is the one long-form document an agent needs and the one an
install-only user cannot reach: ``uv tool install agentless-mcp`` leaves no
checkout, so a guide that lives in ``docs/`` beside the source is a path that
does not exist on the machine running the tool. It therefore ships as package
data under ``agentless_mcp/docs/`` and is read back through
``importlib.resources``, which resolves to the source tree in development and
to the installed package after an install, over the same code path.

Sections exist because the guide is 45 KB and an agent usually wants one tool's
entry. Splitting is done here rather than in the CLI so the rules are testable
without a parser: a heading is a heading only outside a fenced code block, and
the addressable levels are ``##`` and ``###`` -- the title is the whole
document and ``####`` headings are paragraphs of their parent.

Fences follow the CommonMark rule: an opener is a run of three or more
backticks or tildes, and only a run of the *same character*, *at least as
long*, with nothing but spaces after it, closes it. A shorter run, or one of
the other character, is content. The guide today uses three backticks
throughout, so the rule is invisible in the current file; it is written this
way so that a future guide edit using a longer or nested fence cannot silently
turn its contents into sections.
"""

import re
from functools import lru_cache
from importlib import resources

PACKAGE = "agentless_mcp"
GUIDE_DIRECTORY = "docs"
GUIDE_FILENAME = "agent-guide.md"

# Addressable heading levels. '#' is the document title and '####' headings are
# paragraphs inside their parent section, so neither is a section anchor.
SECTION_LEVELS = (2, 3)

_FENCE = re.compile(r"^ {0,3}(`{3,}|~{3,})(.*)$")
_HEADING = re.compile(r"^ {0,3}(#{1,6})\s+(.*?)\s*#*\s*$")
_NON_ALPHANUMERIC = re.compile(r"[^A-Za-z0-9]+")
_PARENTHESISED = re.compile(r"\([^)]*\)")


class GuideDataError(RuntimeError):
    """The packaged guide is missing, unreadable, or structurally ambiguous.

    A ``RuntimeError`` rather than an ``AtlasError`` on purpose: the CLI maps
    ``AtlasError`` onto an exit code and prints it as a one-line refusal, which
    is the right shape for "this repository has no such symbol" and the wrong
    shape for "this installation is broken". This one propagates.
    """


def _resource_text() -> str:
    """Return the packaged guide's text, or say which install is broken."""
    resource = resources.files(PACKAGE) / GUIDE_DIRECTORY / GUIDE_FILENAME
    try:
        return resource.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        # A guide that is present but not valid UTF-8 is the same broken
        # install as an absent one, and `UnicodeDecodeError` is a
        # `ValueError`, so it escaped a handler that named only `OSError`.
        message = (
            f"the agent guide cannot be read from the {PACKAGE} package "
            f"(expected {GUIDE_DIRECTORY}/{GUIDE_FILENAME}): {exc}"
        )
        raise GuideDataError(message) from exc


def _slug(heading: str) -> str:
    """Derive a section name from a heading's text.

    The names an agent already knows are the tool names, and every per-tool
    heading leads with one: ``### `refs` (`find_referencing_symbols`) -- fan-in``
    becomes ``refs``. So the derivation cuts the prose after the first ``--``,
    drops the parenthesised MCP name, and hyphenates what is left.
    """
    text = heading.split(" -- ", maxsplit=1)[0]
    text = _PARENTHESISED.sub(" ", text).replace("`", "")
    return _NON_ALPHANUMERIC.sub("-", text).strip("-").lower()


def _headings(lines: list[str]) -> list[tuple[int, int, str]]:
    """Every heading outside a fenced block, as (index, level, text)."""
    found: list[tuple[int, int, str]] = []
    fence: tuple[str, int] | None = None
    for index, line in enumerate(lines):
        opener = _FENCE.match(line)
        if opener is not None:
            run = opener.group(1)
            character, length = run[0], len(run)
            if fence is None:
                fence = (character, length)
            elif character == fence[0] and length >= fence[1] and not opener.group(2).strip():
                fence = None
            continue
        if fence is not None:
            continue
        heading = _HEADING.match(line)
        if heading is not None:
            found.append((index, len(heading.group(1)), heading.group(2)))
    return found


@lru_cache(maxsize=1)
def _sections() -> tuple[tuple[str, str], ...]:
    """Split the packaged guide into (name, text) pairs, in document order.

    A section runs to the next heading of the same or a shallower level, so a
    ``##`` section contains its ``###`` children and both are addressable. The
    overlap is the point: ``per-tool-usage`` is the whole reference and
    ``refs`` is one entry in it.
    """
    lines = _resource_text().splitlines()
    headings = _headings(lines)
    sections: list[tuple[str, str]] = []
    for position, (index, level, text) in enumerate(headings):
        if level not in SECTION_LEVELS:
            continue
        end = len(lines)
        for following_index, following_level, _ in headings[position + 1 :]:
            if following_level <= level:
                end = following_index
                break
        sections.append((_slug(text), "\n".join(lines[index:end]).rstrip("\n")))

    names = [name for name, _ in sections]
    duplicates = sorted({name for name in names if names.count(name) > 1})
    if duplicates:
        message = (
            f"two guide headings derive the same section name {duplicates}: "
            "one would shadow the other, so rename one heading"
        )
        raise GuideDataError(message)
    return tuple(sections)


def guide_text() -> str:
    """Return the whole packaged guide."""
    return _resource_text()


def section_names() -> tuple[str, ...]:
    """Return every addressable section name, in document order."""
    return tuple(name for name, _ in _sections())


def section_text(name: str) -> str | None:
    """Return one section's text, or ``None`` when no section has that name."""
    for candidate, text in _sections():
        if candidate == name:
            return text
    return None
