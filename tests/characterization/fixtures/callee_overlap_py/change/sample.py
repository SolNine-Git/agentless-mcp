from helpers import stat_high, stat_low, stat_mean, stat_spread



def describe_sample(sample):
    center = stat_mean(len(sample) + 1)
    width = stat_spread(center)
    return stat_low(width) * 2 + stat_high(width)
