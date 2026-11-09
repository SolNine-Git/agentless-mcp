import operator


def span_at(line, spans):
    best = None
    for index, (start, end) in sorted(enumerate(spans), key=operator.itemgetter(1)):
        if start <= line <= end:
            best = index if best is None else min(best, index)
    return best
