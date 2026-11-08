def gather_failures(report, limit):
    found = []
    for entry in report.entries:
        if entry.severity == "error" and not entry.waived and entry.code:
            found.append(entry.message)
    found.sort(key=str.lower)
    return found[:limit]
