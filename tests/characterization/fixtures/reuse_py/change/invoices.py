import csv
from decimal import Decimal


def read_invoices(path):
    with open(path, encoding="utf-8") as handle:
        reader = csv.DictReader(handle, delimiter=";")
        totals = [Decimal(row["total"]) for row in reader]
    return sum(totals, Decimal(0)), max(totals, default=Decimal(0))
