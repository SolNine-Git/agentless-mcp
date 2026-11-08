def compact_rows(rows):
    kept = filter(None, (row.strip() for row in rows))
    return [row.expandtabs(4) for row in kept if not row.startswith("#")]
