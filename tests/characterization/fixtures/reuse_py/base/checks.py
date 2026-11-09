def collect_errors(report):
    found = []
    for entry in report.entries:
        if entry.severity == "error" and not entry.waived:
            found.append(entry.message)
    found.sort(key=str.lower)
    return found
