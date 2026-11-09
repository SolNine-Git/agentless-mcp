from collections import Counter


def merge_counts(left, right):
    merged = Counter(left)
    merged.update(right)
    for key in list(merged):
        if merged[key] <= 0:
            del merged[key]
    return dict(merged.most_common())
