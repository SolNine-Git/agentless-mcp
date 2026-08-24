"""The running account every invoice is posted against.

Imports ``billing.invoice`` for its type alone, which is what closes the
import cycle this fixture exists to hold: the cycles view has to find it.
"""

from decimal import Decimal

from billing.invoice import Invoice
from billing.money import format_amount, sum_amounts


REFERENCE_PREFIX = "INV"


def next_reference(sequence: int) -> str:
    """Mint the reference an invoice is posted under."""
    if sequence < 1:
        message = "a posting sequence starts at one"
        raise ValueError(message)
    return f"{REFERENCE_PREFIX}-{sequence:05d}"


class Ledger:
    """Posted invoices for one customer, oldest first."""

    def __init__(self, customer: str) -> None:
        """Start an empty ledger for a named customer."""
        if not customer:
            message = "a ledger needs a customer name"
            raise ValueError(message)
        self.customer = customer
        self._posted: list[Invoice] = []

    def post(self, invoice: Invoice) -> None:
        """Append an invoice, refusing a duplicate reference."""
        for held in self._posted:
            if held.reference == invoice.reference:
                message = f"{invoice.reference} is already posted"
                raise ValueError(message)
        self._posted.append(invoice)

    def balance(self) -> Decimal:
        """Total every posted invoice."""
        return sum_amounts([invoice.gross() for invoice in self._posted])

    def statement(self) -> list[str]:
        """Render one line per posted invoice, then the balance."""
        lines = [f"{invoice.reference}  {format_amount(invoice.gross())}" for invoice in self._posted]
        lines.append(f"balance  {format_amount(self.balance())}")
        return lines
