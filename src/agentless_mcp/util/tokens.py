"""TokenCounter protocol and the default chars/4 estimator.

The package never imports a tokenizer: a model-free tool has no model whose
vocabulary it could match. Budgets are therefore expressed against an
estimator behind this protocol, so a caller that does have a tokenizer can
supply one without touching the budget logic.
"""

from typing import Protocol, runtime_checkable


@runtime_checkable
class TokenCounter(Protocol):
    """Counts tokens in a string. Implementations must be pure."""

    def count(self, text: str) -> int:
        """Return the estimated token count of ``text``."""
        ...


class Chars4Counter:
    """The default estimator: one token per four characters, floor division.

    Deliberately crude and deliberately pinned by tests. Its job is to make
    budgets reproducible, not to match any particular vocabulary.
    """

    __slots__ = ()

    def count(self, text: str) -> int:
        """Return ``len(text) // 4``."""
        return len(text) // 4
