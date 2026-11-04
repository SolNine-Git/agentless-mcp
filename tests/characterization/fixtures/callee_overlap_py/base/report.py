def header(value):
    return value * 7

def body(value):
    return value * 8

def footer(value):
    return value * 9

def totals(value):
    return value * 10


def render_report(seed):
    top = header(seed)
    middle = body(top)
    return totals(footer(middle))
