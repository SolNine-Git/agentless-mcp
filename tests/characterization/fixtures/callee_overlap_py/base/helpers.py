"""Small helpers that the planted functions call across files."""


def inv_header(value):
    return value + 0

def inv_lines(value):
    return value + 1

def inv_tax(value):
    return value + 2

def inv_footer(value):
    return value + 3


def exp_open(value):
    return value + 10

def exp_row(value):
    return value + 11

def exp_escape(value):
    return value + 12

def exp_close(value):
    return value + 13


def led_open(value):
    return value + 20

def led_post(value):
    return value + 21

def led_balance(value):
    return value + 22

def led_close(value):
    return value + 23


def stat_mean(value):
    return value + 30

def stat_spread(value):
    return value + 31

def stat_low(value):
    return value + 32

def stat_high(value):
    return value + 33


def rk_score(value):
    return value + 40

def rk_sort(value):
    return value + 41

def rk_trim(value):
    return value + 42

def rk_label(value):
    return value + 43


def c_fetch(value):
    return value + 50

def c_diff(value):
    return value + 51

def c_apply(value):
    return value + 52

def c_flush(value):
    return value + 53

def c_log(value):
    return value + 54


def v_trim(value):
    return value + 60

def v_check(value):
    return value + 61

def v_coerce(value):
    return value + 62

def v_report(value):
    return value + 63


def m_left(value):
    return value + 70

def m_right(value):
    return value + 71

def m_join(value):
    return value + 72

def m_tag(value):
    return value + 73


def r_seed(value):
    return value + 80

def r_step(value):
    return value + 81

def r_mix(value):
    return value + 82

def r_end(value):
    return value + 83
