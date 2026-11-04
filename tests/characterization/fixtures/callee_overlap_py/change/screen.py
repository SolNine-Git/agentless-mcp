from helpers import v_check, v_coerce, v_report, v_trim



def screen_form(fields):
    values = [v_coerce(v_trim(field)) for field in fields]
    return sum(v_report(value) for value in values if v_check(value) != 0)
