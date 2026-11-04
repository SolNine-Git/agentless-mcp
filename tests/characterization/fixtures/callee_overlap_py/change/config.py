def read_text(value):
    return value * 5

def parse_toml(value):
    return value * 6

def check_config(value):
    return value * 7

def check_manifest(value):
    return value * 8


def load_config(path):
    text = read_text(path)
    return check_config(parse_toml(text))


def load_manifest(path):
    text = read_text(path)
    return check_manifest(parse_toml(text))
