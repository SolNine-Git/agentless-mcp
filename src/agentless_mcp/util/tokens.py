"""TokenCounter protocol and the default chars/4 estimator.

The package never imports a tokenizer: a model-free tool has no model whose
vocabulary it could match. Budgets are therefore expressed against an
estimator behind this protocol, so a caller that does have a tokenizer can
supply one without touching the budget logic.

The names of the selectable counters live here with the protocol, and the
implementations that need a third-party package live at the composition root
in :mod:`agentless_mcp.bootstrap`. Nothing below the adapters may import an
optional dependency, and the estimator below is what "no optional dependency"
means in practice: chars/4 is the default, it is what the token regression
pins measure, and installing an extra must not silently move them.
"""

from typing import Protocol, runtime_checkable

COUNTER_CHARS4 = "chars4"
COUNTER_TIKTOKEN = "tiktoken"

TOKEN_COUNTERS = (COUNTER_CHARS4, COUNTER_TIKTOKEN)


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

    Floor division means any text shorter than four characters costs nothing,
    so a non-empty string can be free against a budget. That is safe for the
    consumers there are: ``map_service._pack`` is a bisection over a finite
    candidate list, which terminates whatever a candidate costs, and
    ``_auto_budget`` clamps its estimate into a fixed band. A consumer that
    loops until the cost rises would need a positive floor, and there is none.
    """

    __slots__ = ()

    def count(self, text: str) -> int:
        """Return ``len(text) // 4``."""
        return len(text) // 4
