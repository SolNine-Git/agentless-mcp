"""Reading the JSON prompt files that ship inside this package.

Prompt text is data, so it is read the way foreign data is read anywhere else
here: through one parse step that returns typed values or raises. A file that
is missing from the wheel, a file that is not JSON, a key the code consumes
that the file does not carry, a key the file carries that no code consumes,
and a value that is blank are all the same class of defect -- a prompt the
agent would never see -- and all five raise :class:`PromptDataError`.

The parse happens at import time (see this package's ``__init__``), so the
defect surfaces when the server starts rather than when the first tool call
renders an empty description.

``parse_strings`` takes the text rather than the filename on purpose: it is
the pure half, so the failure modes above are testable without writing a
broken file to disk.
"""

import json
from collections.abc import Callable, Iterable, Mapping
from importlib import resources
from types import MappingProxyType
from typing import Any, TypeVar

PACKAGE = "agentless_mcp.prompts"

T = TypeVar("T")


class PromptDataError(RuntimeError):
    """A prompt file is missing, malformed, or does not match its manifest."""


def resource_text(filename: str) -> str:
    """Return one prompt file's text from the installed package."""
    try:
        return resources.files(PACKAGE).joinpath(filename).read_text(encoding="utf-8")
    except OSError as exc:
        message = f"prompt file {filename!r} is missing from the {PACKAGE} package: {exc}"
        raise PromptDataError(message) from exc


def parse_strings(filename: str, text: str, manifest: Iterable[str]) -> dict[str, str]:
    """Decode one prompt file, checked against the keys the code consumes."""
    try:
        document: Any = json.loads(text)
    except json.JSONDecodeError as exc:
        message = f"prompt file {filename!r} is not valid JSON: {exc}"
        raise PromptDataError(message) from exc

    if not isinstance(document, dict):
        kind = type(document).__name__
        message = f"prompt file {filename!r} must hold a JSON object, not {kind}"
        raise PromptDataError(message)

    expected = set(manifest)
    present = {str(key) for key in document}
    missing = sorted(expected - present)
    unknown = sorted(present - expected)
    if missing or unknown:
        message = (
            f"prompt file {filename!r} does not match the keys the code consumes: "
            f"missing {missing}, unknown {unknown}"
        )
        raise PromptDataError(message)

    values: dict[str, str] = {}
    for key in sorted(expected):
        value = document[key]
        if not isinstance(value, str) or not value.strip():
            message = f"prompt file {filename!r} key {key!r} must be a non-empty string"
            raise PromptDataError(message)
        values[key] = value
    return values


def build_record(
    filename: str,
    text: str,
    record: Callable[..., T],
    manifest: Iterable[str],
) -> T:
    """Build one frozen record out of a prompt file's text."""
    return record(**parse_strings(filename, text, manifest))


def load_record(filename: str, record: Callable[..., T], manifest: Iterable[str]) -> T:
    """Read a prompt file from the package and build its frozen record."""
    return build_record(filename, resource_text(filename), record, manifest)


def load_mapping(filename: str, manifest: Iterable[str]) -> Mapping[str, str]:
    """Read a prompt file whose keys are a known set of names, not fields."""
    return MappingProxyType(parse_strings(filename, resource_text(filename), manifest))
