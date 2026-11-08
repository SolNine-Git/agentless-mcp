import operator


def newest_entry(entries):
    dated = [entry for entry in entries if entry.stamp is not None]
    return max(dated, key=operator.attrgetter("stamp"), default=None)
