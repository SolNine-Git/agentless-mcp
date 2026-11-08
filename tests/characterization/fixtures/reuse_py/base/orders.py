import csv

from models import Order


def read_orders(path):
    with open(path, encoding="utf-8") as handle:
        rows = list(csv.DictReader(handle))
    return [Order.from_row(row) for row in rows if row.get("status") != "void"]
