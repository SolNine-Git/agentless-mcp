from collections import Counter


def combine_tallies(first, second):
    total = Counter(first)
    total.update(second)
    for name in list(total):
        if total[name] <= 0:
            del total[name]
    return dict(total.most_common())
