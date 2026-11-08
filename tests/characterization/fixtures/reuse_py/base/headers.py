def parse_header(line):
    name, _, value = line.partition(":")
    key = name.strip().casefold()
    return key, value.lstrip()
