def flag(value):
    return value * 6

def describe(value):
    return value * 7

def register(value):
    return value * 8


def arg_alpha():
    return register(describe(flag(1)))


def arg_beta():
    return register(describe(flag(2)))


def arg_gamma():
    return register(describe(flag(3)))


def arg_delta():
    return register(describe(flag(4)))


def arg_epsilon():
    return register(describe(flag(5)))


def arg_zeta():
    return register(describe(flag(6)))
