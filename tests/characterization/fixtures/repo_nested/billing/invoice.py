"""One invoice: its lines, its tax, and how it is rendered."""

from dataclasses import dataclass
from decimal import Decimal

from billing.ledger import next_reference
from billing.money import format_amount, sum_amounts
from billing.tax import tax_on

INVOICE_WIDTH = 48


@dataclass(frozen=True)
class InvoiceLine:
    """One charged line on an invoice."""

    description: str
    net: Decimal
    band: str

    def tax(self) -> Decimal:
        """Return the tax due on this line."""
        return tax_on(self.net, self.band)


@dataclass(frozen=True)
class Invoice:
    """A whole invoice, identified by its reference."""

    reference: str
    lines: tuple[InvoiceLine, ...]

    def net(self) -> Decimal:
        """Total the net of every line."""
        return sum_amounts([line.net for line in self.lines])

    def gross(self) -> Decimal:
        """Total the net and the tax of every line."""
        return sum_amounts([line.net + line.tax() for line in self.lines])


def open_invoice(sequence: int, lines: tuple[InvoiceLine, ...]) -> Invoice:
    """Open an invoice under the next ledger reference."""
    return Invoice(reference=next_reference(sequence), lines=lines)


def render_invoice(invoice: Invoice) -> str:
    """Render an invoice as plain text, one line per charge."""
    if not invoice.lines:
        return f"{invoice.reference}: nothing charged\n"

    rendered = [invoice.reference, "-" * INVOICE_WIDTH]
    for line in invoice.lines:
        label = line.description.ljust(28)
        rendered.append(f"{label}{format_amount(line.net + line.tax())}")
    rendered.append("-" * INVOICE_WIDTH)
    rendered.append(f"{'gross'.ljust(28)}{format_amount(invoice.gross())}")
    return "\n".join(rendered) + "\n"
