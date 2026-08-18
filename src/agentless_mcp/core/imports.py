"""Import-statement domain values extracted from parsed sources.

Pure value object, copied from the mcp-local domain layer. Only the pieces
the extractor produces live here; the rest of that module described a
different service's model and does not belong in this package.
"""

from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class ImportStatement:
    """A single import extracted from source code AST."""

    module: str
    names: tuple[str, ...]
    is_relative: bool
    relative_level: int
    line_number: int
    resolved_path: str
