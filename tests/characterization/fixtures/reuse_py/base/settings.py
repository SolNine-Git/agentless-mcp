import json
from pathlib import Path

KNOWN = {"verbose", "root", "jobs"}


def load_settings(path):
    raw = Path(path).read_text(encoding="utf-8")
    data = json.loads(raw)
    data.setdefault("verbose", False)
    return validate_keys(data)


def validate_keys(data):
    unknown = sorted(set(data) - KNOWN)
    if unknown:
        raise ValueError(unknown)
    return data
