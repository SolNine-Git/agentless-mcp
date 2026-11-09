import operator


def enclosing(line, spans):
    ordered = sorted(enumerate(spans), key=operator.itemgetter(1))
    hits = [index for index, (start, end) in ordered if start <= line <= end]
    return min(hits, default=None)
