from helpers import rk_label, rk_score, rk_sort, rk_trim



def order_items(items):
    limit = rk_trim(rk_sort(len(items)))
    scores = sorted(rk_score(item) for item in items)
    labels = list(map(rk_label, scores))
    return labels[:limit]
