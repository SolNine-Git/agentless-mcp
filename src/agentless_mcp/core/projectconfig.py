"""The optional per-repository config file, parsed as the untrusted data it is.

``.agentless-mcp.json`` at the root of an analysed repository lets a project
state its own defaults -- the budget its map wants, the names that collide
everywhere in it, the command that runs its tests -- so an agent does not have
to be told them on every call.

**The file is repository content.** It arrives from the same place as the
source this package is careful never to trust, so it is parsed exactly like
any other foreign input: one parse step at the boundary that produces typed,
bounded values or reports why it could not. Three rules follow from that and
none of them are negotiable.

* **Bounded, typed values only.** Every key has a type and a range, and a
  value outside either is dropped with a warning rather than clamped
  silently. No key is path-typed: a repository cannot name a file for this
  tool to read, which removes the whole class of "config points somewhere
  else" escapes before it starts. The file itself is held to the same rule --
  a ``.agentless-mcp.json`` that resolves outside the repository root is a
  repository naming a file too, and is refused rather than read.
* **Unknown keys are a warning, never an error.** A newer file read by an
  older build must not stop the tool from answering; the warning rides in the
  response envelope where the caller sees it. Bounded like every other value:
  the warnings ride in every response for this repository, so a file that is
  nothing but unknown keys must not be able to fill one.
* **The test command is inert here.** :func:`load` reads it, and nothing in
  the MCP surface can reach it. Only the CLI's ``validate`` uses it, only when
  the invocation passed no ``--test-cmd`` of its own, and it prints the
  resolved command in the run header -- because a command that came from the
  repository being judged must be visible before it runs.

An absent file, an unreadable one and a malformed one all produce the same
thing: an empty configuration whose ``warnings`` say which happened. There is
no failure mode here that stops a read command from answering.
"""

import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, TypeVar

from agentless_mcp.util.fslimits import file_stays_inside

CONFIG_FILENAME = ".agentless-mcp.json"

# A config file is a handful of scalars. Anything larger is not one, and
# reading it would be reading whatever was left in the repository under that
# name.
MAX_CONFIG_BYTES = 64 * 1024

# Bounds. Each is the range within which the value is a defensible answer to
# the question the key asks, not a limit of the implementation.
MIN_BUDGET = 200
MAX_BUDGET = 64_000
MIN_MAX_FILES = 1
MAX_MAX_FILES = 200
MAX_STOPLIST_ENTRIES = 200
MAX_STOPLIST_ENTRY_CHARS = 64
MAX_TEST_CMD_CHARS = 512
# Enough to name every key a hand-written file gets wrong. Past that the list
# stops being a diagnostic and starts being a way to spend the response's
# token budget on repository-supplied text.
MAX_UNKNOWN_KEY_WARNINGS = 8

# The choices a config file may set as the map's default. Deliberately
# narrower than ``map_service.GRANULARITIES``, which also carries 'body': a
# persistent default of the expensive body view is a footgun, so 'body' is
# wire-only and a config that names it is warned about and ignored. A test
# pins the divergence -- do not merge the two tuples.
GRANULARITIES = ("function", "file")

KEY_MAP_BUDGET = "map_budget"
KEY_MAX_FILES = "max_files"
KEY_GRANULARITY = "granularity"
KEY_DOCSTRINGS = "docstrings"
KEY_STOPLIST = "stoplist"
KEY_TEST_CMD = "test_cmd"
# Weight the reference graph's edges by what kind of relationship each one is
# -- inheritance above imports, imports above name references -- instead of by
# the shipped scheme. Off unless a repository asks for it, and **it is not
# language-neutral**: the base classes it reads come from ``ASTSymbol.bases``,
# which only the extractor's Python class handler fills in. Every other
# language records no bases, so this weights Python inheritance and weights
# nothing at all for TypeScript, Go, Java or Rust. Do not turn it on expecting
# a repository-wide effect on a repository that is not Python.
KEY_RELATION_WEIGHTS = "relation_weights"

KNOWN_KEYS = (
    KEY_MAP_BUDGET,
    KEY_MAX_FILES,
    KEY_GRANULARITY,
    KEY_DOCSTRINGS,
    KEY_STOPLIST,
    KEY_TEST_CMD,
    KEY_RELATION_WEIGHTS,
)

# Characters that would make a stoplist entry something other than an
# identifier: separators, quotes and whitespace. An entry is matched against
# tree-sitter identifier text, so anything holding one of these could never
# match anything and is far more likely to be an attempt at something else.
_FORBIDDEN_ENTRY_CHARS = frozenset("/\\:;'\"`$*?<>|\0")


@dataclass(frozen=True)
class ProjectConfig:
    """One repository's declared defaults, already validated and bounded.

    Every field is ``None`` when the file did not set it, which is what lets
    the precedence rule stay a one-liner at each call site: an explicit
    argument wins, then this, then the built-in default. ``stoplist`` is the
    exception and defaults to empty, because "no additions" and "the key was
    absent" call for the same behaviour.
    """

    path: Path | None = None
    map_budget: int | None = None
    max_files: int | None = None
    granularity: str | None = None
    docstrings: bool | None = None
    stoplist: frozenset[str] = frozenset()
    test_cmd: str | None = None
    relation_weights: bool | None = None
    warnings: tuple[str, ...] = ()

    @property
    def present(self) -> bool:
        """True when a config file was actually read."""
        return self.path is not None

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this configuration."""
        return {
            "path": str(self.path) if self.path is not None else None,
            KEY_MAP_BUDGET: self.map_budget,
            KEY_MAX_FILES: self.max_files,
            KEY_GRANULARITY: self.granularity,
            KEY_DOCSTRINGS: self.docstrings,
            KEY_STOPLIST: sorted(self.stoplist),
            KEY_TEST_CMD: self.test_cmd,
            KEY_RELATION_WEIGHTS: self.relation_weights,
            "warnings": list(self.warnings),
        }


EMPTY = ProjectConfig()

_T = TypeVar("_T")


def resolve(explicit: _T | None, configured: _T | None, default: _T) -> _T:
    """Apply the precedence rule: the invocation, then the repository, then us.

    One function rather than a conditional per setting, because the ordering
    is a single decision and a caller that spelled it differently would be a
    bug nobody could see from the call site.
    """
    if explicit is not None:
        return explicit
    if configured is not None:
        return configured
    return default


def load(repo_root: Path) -> ProjectConfig:
    """Read and validate ``repo_root/.agentless-mcp.json``.

    Never raises. A repository with no config file gets :data:`EMPTY`; every
    other outcome -- unreadable, not JSON, not an object, oversized, pointing
    out of the tree -- gets an empty configuration carrying the reason as a
    warning.
    """
    path = repo_root / CONFIG_FILENAME
    if not path.is_file():
        # Two different facts, and only one of them is silent. Nothing at that
        # name is the ordinary case and gets no warning. *Something* at that
        # name that is not a readable regular file -- a directory, a FIFO, a
        # dangling symlink -- is a config that will silently do nothing, which
        # is the case every other refusal path here exists to make visible.
        # `exists` follows links, so a dangling one needs the second test.
        if not path.exists() and not path.is_symlink():
            return EMPTY
        return _refused(f"{CONFIG_FILENAME} is not a readable regular file; ignored")

    if not file_stays_inside(path, repo_root.resolve()):
        # `is_file` and `read_text` both follow links, so without this a
        # repository names a file outside itself for this tool to read. A
        # warning rather than a raise: nothing here stops a read command.
        return _refused(f"{CONFIG_FILENAME} resolves outside the repository root; ignored")

    text, refusal = _bounded_text(path)
    if text is None:
        return _refused(refusal)

    document, refusal = _decode(text)
    if document is None:
        return _refused(refusal)

    return parse(document, path)


def _bounded_text(path: Path) -> tuple[str | None, str]:
    """Read the config file if it is small enough, or say why it was not read."""
    try:
        size = path.stat().st_size
    except OSError as exc:
        return None, f"{CONFIG_FILENAME} could not be stat'd: {exc.strerror}"

    if size > MAX_CONFIG_BYTES:
        return (
            None,
            f"{CONFIG_FILENAME} is {size} bytes, over the {MAX_CONFIG_BYTES}-byte cap; ignored",
        )

    # One byte past the cap, so the read itself enforces it. The stat above is
    # an early-out that names the size; it cannot be the bound, because a
    # writer that grows the file between the two calls would then be read
    # whole. Repository content is the untrusted source this module names, and
    # an active writer is the only precondition that needs.
    try:
        with path.open("rb") as handle:
            raw = handle.read(MAX_CONFIG_BYTES + 1)
    except OSError as exc:
        return None, f"{CONFIG_FILENAME} could not be read: {exc.strerror}"

    if len(raw) > MAX_CONFIG_BYTES:
        return (
            None,
            (
                f"{CONFIG_FILENAME} grew past the {MAX_CONFIG_BYTES}-byte cap while it "
                "was being read; ignored"
            ),
        )

    try:
        return raw.decode("utf-8"), ""
    except UnicodeDecodeError:
        return None, f"{CONFIG_FILENAME} is not UTF-8 text; ignored"


def _decode(text: str) -> tuple[dict[str, Any] | None, str]:
    """Decode the config text into an object, or say why it is not one."""
    try:
        document = json.loads(text)
    except json.JSONDecodeError as exc:
        return None, f"{CONFIG_FILENAME} is not valid JSON ({exc.msg} at line {exc.lineno})"

    if not isinstance(document, dict):
        return None, f"{CONFIG_FILENAME} must hold a JSON object; ignored"
    return document, ""


def parse(document: dict[str, Any], path: Path | None = None) -> ProjectConfig:
    """Validate an already-decoded config document.

    Split from :func:`load` so the schema can be tested without a filesystem,
    and so the file-shaped failures above stay in one place.
    """
    warnings: list[str] = []

    # The values first, deliberately. `envelope.MAX_CONFIG_WARNINGS` is 8 and
    # so is `MAX_UNKNOWN_KEY_WARNINGS`, so whichever warnings are appended
    # first are the ones an operator gets to read. A value this build rejected
    # is about a key the file meant to set and is actionable; an unknown key is
    # a key this build ignores, and a newer file read by an older one is the
    # ordinary reason for it. Ordered the other way, eight unknown keys pushed
    # every warning about the file's own malformed values out of the response.
    map_budget = _bounded_int(document, KEY_MAP_BUDGET, MIN_BUDGET, MAX_BUDGET, warnings)
    max_files = _bounded_int(document, KEY_MAX_FILES, MIN_MAX_FILES, MAX_MAX_FILES, warnings)
    granularity = _choice(document, KEY_GRANULARITY, GRANULARITIES, warnings)
    docstrings = _boolean(document, KEY_DOCSTRINGS, warnings)
    stoplist = _stoplist(document, warnings)
    test_cmd = _command(document, warnings)
    relation_weights = _boolean(document, KEY_RELATION_WEIGHTS, warnings)

    unknown = sorted(key for key in document if key not in KNOWN_KEYS)
    # `{key!r}` happens to escape a newline in a repository-authored key,
    # which is convenient and is not the guarantee: the warning is made safe
    # for a line-oriented answer by `application/envelope`, at the sink.
    warnings.extend(
        f"unknown key {key!r} in {CONFIG_FILENAME}: ignored (known keys: {', '.join(KNOWN_KEYS)})"
        for key in unknown[:MAX_UNKNOWN_KEY_WARNINGS]
    )
    if len(unknown) > MAX_UNKNOWN_KEY_WARNINGS:
        suppressed = len(unknown) - MAX_UNKNOWN_KEY_WARNINGS
        warnings.append(
            f"{suppressed} further unknown keys in {CONFIG_FILENAME}: warnings suppressed"
        )

    return ProjectConfig(
        path=path,
        map_budget=map_budget,
        max_files=max_files,
        granularity=granularity,
        docstrings=docstrings,
        stoplist=stoplist,
        test_cmd=test_cmd,
        relation_weights=relation_weights,
        warnings=tuple(warnings),
    )


def _refused(reason: str) -> ProjectConfig:
    """Return an empty configuration carrying why the file was not used."""
    return ProjectConfig(warnings=(reason,))


def _bounded_int(
    document: dict[str, Any], key: str, low: int, high: int, warnings: list[str]
) -> int | None:
    """Read an integer key within its bounds, warning instead of clamping."""
    if key not in document:
        return None

    value = document[key]
    # `bool` is an `int` in Python and `true` is not a budget.
    if not isinstance(value, int) or isinstance(value, bool):
        warnings.append(f"{key} in {CONFIG_FILENAME} must be an integer; ignored")
        return None
    if not low <= value <= high:
        warnings.append(f"{key} in {CONFIG_FILENAME} must be between {low} and {high}; ignored")
        return None
    return value


def _choice(
    document: dict[str, Any], key: str, allowed: tuple[str, ...], warnings: list[str]
) -> str | None:
    """Read a key restricted to a fixed set of strings."""
    if key not in document:
        return None

    value = document[key]
    if not isinstance(value, str) or value not in allowed:
        warnings.append(f"{key} in {CONFIG_FILENAME} must be one of {', '.join(allowed)}; ignored")
        return None
    return value


def _boolean(document: dict[str, Any], key: str, warnings: list[str]) -> bool | None:
    """Read a boolean key, refusing the truthy-string spellings of one."""
    if key not in document:
        return None

    value = document[key]
    if not isinstance(value, bool):
        warnings.append(f"{key} in {CONFIG_FILENAME} must be true or false; ignored")
        return None
    return value


def _stoplist(document: dict[str, Any], warnings: list[str]) -> frozenset[str]:
    """Read the stoplist additions: a bounded list of bare identifier names."""
    if KEY_STOPLIST not in document:
        return frozenset()

    value = document[KEY_STOPLIST]
    if not isinstance(value, list):
        warnings.append(f"{KEY_STOPLIST} in {CONFIG_FILENAME} must be a list of names; ignored")
        return frozenset()

    if len(value) > MAX_STOPLIST_ENTRIES:
        warnings.append(
            f"{KEY_STOPLIST} in {CONFIG_FILENAME} holds {len(value)} entries, over the "
            f"{MAX_STOPLIST_ENTRIES} cap; only the first {MAX_STOPLIST_ENTRIES} are used"
        )

    names: set[str] = set()
    rejected = 0
    for entry in value[:MAX_STOPLIST_ENTRIES]:
        if _is_name(entry):
            names.add(entry)
        else:
            rejected += 1

    if rejected:
        warnings.append(
            f"{KEY_STOPLIST} in {CONFIG_FILENAME}: {rejected} entries are not bare names "
            f"of at most {MAX_STOPLIST_ENTRY_CHARS} characters and were dropped"
        )
    return frozenset(names)


def _is_name(entry: Any) -> bool:
    """True for a plain identifier-shaped string, which is all this list holds."""
    return (
        isinstance(entry, str)
        and bool(entry)
        and len(entry) <= MAX_STOPLIST_ENTRY_CHARS
        and not any(character.isspace() for character in entry)
        and not (_FORBIDDEN_ENTRY_CHARS & set(entry))
    )


def _command(document: dict[str, Any], warnings: list[str]) -> str | None:
    """Read the default test command: a bounded, single-line string.

    Validated here and *used* nowhere in this package. The one caller is the
    CLI's ``validate``, which takes it only when the invocation named no
    command of its own and prints it before running it.
    """
    if KEY_TEST_CMD not in document:
        return None

    value = document[KEY_TEST_CMD]
    if not isinstance(value, str) or not value.strip():
        warnings.append(f"{KEY_TEST_CMD} in {CONFIG_FILENAME} must be a non-empty string; ignored")
        return None
    if len(value) > MAX_TEST_CMD_CHARS:
        warnings.append(
            f"{KEY_TEST_CMD} in {CONFIG_FILENAME} is longer than "
            f"{MAX_TEST_CMD_CHARS} characters; ignored"
        )
        return None
    if "\n" in value or "\r" in value:
        warnings.append(f"{KEY_TEST_CMD} in {CONFIG_FILENAME} must be a single line; ignored")
        return None
    return value
