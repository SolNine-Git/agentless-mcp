def split_field(text):
    label, _, rest = text.partition(":")
    field = label.strip().casefold()
    return field, rest.lstrip()
