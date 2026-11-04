from helpers import v_check, v_coerce, v_report, v_trim



def validate_form(field):
    trimmed = v_trim(field)
    if v_check(trimmed) == 0:
        return v_report(trimmed)
    return v_coerce(trimmed)
