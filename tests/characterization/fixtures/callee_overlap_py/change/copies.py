def a_head(value):
    return value * 2

def a_body(value):
    return value * 3

def a_tail(value):
    return value * 4

def a_mark(value):
    return value * 5


def b_split(value):
    return value * 3

def b_strip(value):
    return value * 4

def b_key(value):
    return value * 5

def b_value(value):
    return value * 6


def q_wrap(value):
    return value * 4

def q_weigh(value):
    return value * 5

def q_seal(value):
    return value * 6

def q_ship(value):
    return value * 7


def sum_order(order):
    head = a_head(order)
    body = a_body(head)
    tail = a_tail(body)
    return a_mark(tail)


def parse_line(line):
    parts = b_split(line)
    clean = b_strip(parts)
    return b_key(clean) + b_value(clean)


def pack_items(items):
    packed = 0
    for item in items:
        packed += q_seal(q_weigh(q_wrap(item)))
    return q_ship(packed)


def total_order(order):
    head = a_head(order)
    body = a_body(head)
    tail = a_tail(body)
    return a_mark(tail)


def parse_entry(line):
    parts = b_split(line)
    clean = b_strip(parts)
    if clean == 0:
        return b_key(parts)
    return b_key(clean) + b_value(clean) * 2


def bundle_items(items):
    weight = sum(q_weigh(q_wrap(item)) for item in items)
    if weight > 100:
        return q_ship(weight)
    return q_ship(q_seal(weight))
