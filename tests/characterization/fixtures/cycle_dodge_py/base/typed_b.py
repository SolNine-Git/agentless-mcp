"""Validation, typed against typed_a."""

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    import typed_a


def validate(value: "typed_a.Value") -> bool:
    return value is not None
