import json
from pathlib import Path

from settings import validate_keys


def read_config(location):
    text = Path(location).read_text(encoding="utf-8")
    loaded = json.loads(text)
    loaded.setdefault("verbose", False)
    return validate_keys(loaded)
