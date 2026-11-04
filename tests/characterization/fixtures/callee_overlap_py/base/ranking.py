from helpers import rk_label, rk_score, rk_sort, rk_trim



def rank_items(items):
    scored = []
    for item in items:
        scored.append(rk_score(item))
    kept = rk_trim(rk_sort(len(scored)))
    return [rk_label(score) for score in scored[:kept]]
