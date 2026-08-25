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

    **It is not a model's tokenizer and does not track one, and how far it
    misses depends on the view.** Measured 2026-08-25 against ``cl100k_base``
    over the committed goldens, the real-to-estimated ratio runs 0.979 to
    1.264. The id-dense views drift furthest and always upward -- a map golden
    reaches 1.264, so an 8000-token budget buys near 10,000 real ones -- while
    ``lint`` output sits at 0.979, where the estimator counts *over*. There is
    therefore no single correction factor, and a caller sizing a real context
    window should measure the view they actually call rather than scale this
    number by a constant.

    Stable ids, type annotations and path separators are what make the dense
    views dense: a run of punctuation tokenizes to well under four characters
    a token, so the estimator is most wrong exactly where the output is
    densest. Ordinary source, which is most of an overview body, is closest to
    four.

    ``TestTheEstimatorAgainstARealTokenizer`` in
    ``tests/unit/test_token_counter.py`` pins the map goldens' ratio, so a
    rendering change that moves this materially fails there instead of leaving
    a stale number in this docstring. The CLI's ``--token-counter tiktoken``
    swaps in the real counter; the MCP server declares no such flag and always
    counts with this one.

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
