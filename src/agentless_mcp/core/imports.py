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
    # True when this import brings *every* name the target defines into the
    # importing file's own namespace, so a bare reference to one of them is
    # evidence that the file imported it. C's `#include` and Python's
    # `from x import *` are the two forms in the table that do this.
    #
    # False for the far commoner case, which is what makes it worth a field:
    # `import a.b` in Python, `import "fmt"` in Go and `import * as ns` in
    # TypeScript all bind a module *object*. A bare reference to something
    # that module defines is a NameError, not a caller.
    binds_all: bool = False
