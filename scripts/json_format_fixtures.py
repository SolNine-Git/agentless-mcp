"""Write the json crate's format fixtures: float text, the packed form, and json.dumps of values.

Development-time only. Run it from the repository root:

    .venv/bin/python scripts/json_format_fixtures.py

A float is stored as "0x" and the 16 hex digits of its bits. The value_text section holds JSON
documents with the default json.dumps text of what json.loads reads from them. With --check, the
script compares its output with the committed fixtures and writes nothing.
"""

import json
import math
import random
import struct
import sys
from pathlib import Path
from typing import Any

from fixture_files import Fixture, write_or_check

FIXTURES = Path(__file__).resolve().parents[1] / "crates" / "json" / "fixtures" / "format"
PLACES = (1, 3, 4)
SEED = 20261012
E_ACUTE = chr(0xE9)
Y_DIAERESIS = chr(0xFF)
LINE_SEPARATOR = chr(0x2028)
# In code point order U+FFFF sorts before U+1F600, and in UTF-16 order it sorts after.
LAST_BMP = chr(0xFFFF)
EMOJI = chr(0x1F600)


def from_bits(raw: int) -> float:
    (value,) = struct.unpack("<d", struct.pack("<Q", raw))
    return value


def bits(x: float) -> str:
    (raw,) = struct.unpack("<Q", struct.pack("<d", x))
    return f"0x{raw:016x}"


SPECIALS = [
    0.0,
    -0.0,
    math.inf,
    -math.inf,
    math.nan,
    from_bits(0xFFF8_0000_0000_0000),
    from_bits(0x7FF0_0000_0000_0001),
]
SUBNORMALS = [
    from_bits(0x0000_0000_0000_0001),
    from_bits(0x0008_0000_0000_0000),
    from_bits(0x000F_FFFF_FFFF_FFFF),
    from_bits(0x0010_0000_0000_0000),
    -from_bits(0x0000_0000_0000_0001),
]
LARGE = [1e15, 1e16, 1e21, 1e22, 1e100, 1e308, sys.float_info.max, 9007199254740993.0]
LARGE += [123456789012345678.0, 2.0**63, -1e16]
SMALL = [1e-4, 1e-5, 9.99995e-5, 9.999949e-5, 0.000099999949, 1.5e-7, 1e-300, -1e-5]
TIES = [0.25, 0.75, 1.25, 0.0625, 0.03125, 0.09375, 0.15625, 1.0625, 2.5625, 0.5, 2.5]
TIES += [-0.25, -0.0625, -0.03125, -0.09375, 0.000244140625, 0.00006103515625]
GENERAL_TIES = [123456.5, 123457.5, 1234565.0, 1234575.0, 999999.5, 999999.4999]
GENERAL_TIES += [9999995.0, 0.1234565, 12345.65, 100000.0, 1000000.0, 999995.0]
TYPICAL = [1.0, 0.5, 100.0, 1.5, 0.1, 0.2, 0.3, 0.1 + 0.2, 1 / 3, 2 / 3, 0.123456789]
TYPICAL += [13.2664991614216, 0.06060606060606061, 0.85, 1e-6, 123.456, 99.99995, 9.9995]


def random_values() -> list[float]:
    chooser = random.Random(SEED)
    patterns = [from_bits(chooser.getrandbits(64)) for _ in range(300)]
    spread = [chooser.choice((-1.0, 1.0)) * 10.0 ** chooser.uniform(-8.0, 20.0) for _ in range(300)]
    return patterns + spread


def values() -> list[float]:
    every = SPECIALS + SUBNORMALS + LARGE + SMALL + TIES + GENERAL_TIES + TYPICAL
    return every + random_values()


def packed(document: Any) -> str:
    return json.dumps(document, ensure_ascii=True, separators=(",", ":"), sort_keys=True)


DOCUMENTS: list[Any] = [
    {},
    [],
    {"b": 1, "a": 2},
    {"z": {"y": [3, {"b": None, "a": True}], "x": False}, "": "empty key"},
    {"10": "ten", "2": "two", "1": "one", "0": "zero"},
    {"b": 0, "B": 1, "a": 2, "A": 3, "_": 4, "-": 5},
    {E_ACUTE: 1, "e": 2, LAST_BMP: 3, EMOJI: 4, Y_DIAERESIS: 5},
    {"floats": [-0.0, 0.0, 1e16, 1e-05, 0.1, 1.5, 123456789.125, 1e22, 5e-324]},
    {"ints": [0, -1, 9007199254740993, -9223372036854775808, 18446744073709551615]},
    {"text": 'quote " backslash \\ slash / tab \t newline \n nul \x00 del \x7f'},
    {"html": "<script>a && b</script> > <", "unicode": E_ACUTE + LINE_SEPARATOR + EMOJI},
    {"nested": [[], {}, [[{}]], {"k": []}]},
    {"edges": [{"weight": 2.5, "target": "b", "source": "a", "import": True}]},
    [{"b": 1, "a": 1}, {"d": 2, "c": 2}],
    "a bare string",
    1.25,
    None,
]


# json.loads refuses one level more, and an integer of more digits.
DEEPEST = 9998
LONGEST_INT = 4300
VALUE_DOCUMENTS = [
    "NaN",
    "Infinity",
    "-Infinity",
    "[NaN, Infinity, -Infinity, -0.0, 0.0, 1e400, -1e400, 1E2, 1.0, 5e-324, 1e16, 1e-05]",
    "[-0, 0, -1, 12345678901234567890123]",
    "9" * LONGEST_INT,
    "-" + "1" * LONGEST_INT,
    '"\\ud800"',
    '"\\udfff x \\udc00\\ud800"',
    '"\\ud83d\\ude00"',
    '{"\\ud800": "\\udbff"}',
    '"\\u0000\\u001f\\u007f\\u0080\\u00ff\\u2028\\uffff"',
    '"\\b\\f\\n\\r\\t\\"\\\\\\/"',
    '{"a": 1, "b": 2, "a": 3}',
    ' \t\n{ "a" : [ 1 , { } , [ ] ] , "b" :null }\r\n',
    '{"ts": 1760712345.6789012, "session_id": "s", "tool": {"x": [1, "y"]}, "pattern": null}',
    '{"version": 2, "reason": "orient.map", "tool": "mcp__agentless__orient"}',
    "[" * DEEPEST + "]" * DEEPEST,
    '{"a": ' * DEEPEST + "1" + "}" * DEEPEST,
]


def format_fixture() -> Fixture:
    every = values()
    documents = [json.dumps(document) for document in DOCUMENTS] + VALUE_DOCUMENTS
    return {
        "fixed": [[bits(x), places, format(x, f".{places}f")] for x in every for places in PLACES],
        "general": [[bits(x), format(x, "g")] for x in every],
        "packed": [[json.dumps(document), packed(document)] for document in DOCUMENTS],
        "value_text": [[document, json.dumps(json.loads(document))] for document in documents],
    }


BUILDERS = {"format": format_fixture}


if __name__ == "__main__":
    raise SystemExit(write_or_check((__doc__ or "").splitlines()[0], FIXTURES, BUILDERS))
