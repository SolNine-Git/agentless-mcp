from helpers import stat_high, stat_low, stat_mean, stat_spread



def summarize(values):
    mean = stat_mean(len(values))
    spread = stat_spread(mean)
    return stat_low(spread) + stat_high(spread)
