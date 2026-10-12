"""Write the test fixtures whose results the CPython 3.13 standard library computes.

Development-time only. Run it from the repository root:

    uv run --python 3.13 --no-project python scripts/stdlib_fixtures.py

The script writes fixtures for the text, json, metrics, checks, io and store crates. With --check,
the script compares its output with the committed fixtures and writes nothing.
"""

import json
import math
import os
import random
import re
import shlex
import struct
import sys
import tokenize
from collections.abc import Callable
from difflib import SequenceMatcher
from pathlib import Path

from fixture_files import Fixture, render, write_or_check_files

CRATES = Path(__file__).resolve().parents[1] / "crates"
INF = math.inf
NAN = math.nan
WORD = re.compile(r"\w")

SPLITLINES = [
    "",
    "a",
    "a\n",
    "\n",
    "a\r\nb",
    "\r\n\r\n",
    "a\rb\n\nc",
    "x\x1cy\x1dz\x1ew",
    "x\u2028y\x85z\u2029",
    "a\x1fb",
    "a\x0bb\x0cc",
    "a\n\r",
]
SPLIT_WHITESPACE = ["", "   ", " a  b ", "a\x1fb\u3000c", "a\u200bb", "\x85x\xa0y\t", "\u2028a\x1c"]
STRIP = ["", "  ", " a b ", "\x1fa\x1c", "\u3000a\u2029", "\u200ba\u200b", "\xa0\x85a"]

FLOAT_REPR = [
    0.0,
    -0.0,
    1.0,
    -1.0,
    0.1,
    0.5,
    0.3333333333333333,
    0.6666666666666666,
    100.0,
    123456.789,
    1000000000000000.0,
    1e16,
    9999999999999998.0,
    1.0000000000000002e16,
    1e22,
    1.5e300,
    5e-324,
    2.2250738585072014e-308,
    0.0001,
    0.0001,
    0.00012345,
    1e-05,
    1.5e-05,
    0.3,
    2.675,
    1.7976931348623157e308,
    INF,
    -INF,
    NAN,
    12.0,
    1234567890123456.0,
    0.30000000000000004,
    3.14159,
    -2.5e-10,
    -352334.4703336753,
    -698301.6521509961,
    301868.9460797075,
    -855127.4266649145,
    71764.00861337851,
    -268622.1661748289,
    -884002.1504505863,
    14871.466378840501,
    0.0011717393263120275,
    3.894531646471093e-10,
    0.0014173908334978915,
    242.11663289953958,
    2.8824890300083344e-11,
    10332.17500334878,
    7763.63165660779,
    3.2504078464356366e-17,
]
ROUND_VALUES = [
    2.675,
    0.125,
    0.375,
    2.5,
    3.5,
    -2.5,
    0.5,
    1.5,
    -0.5,
    -0.4,
    0.0,
    -0.0,
    1234.5678,
    1e300,
    5e-324,
    0.285,
    1.005,
    0.0005,
    0.0015,
    0.0025,
    5e-07,
    1.5e-06,
    2.5e-06,
    0.1234565,
    0.1234575,
    1.0000005,
    123.4565,
    -0.0004999,
    1e16,
    1.7976931348623157e308,
    INF,
    -INF,
    NAN,
    -95.24089298036279,
    119.5447721609919,
    848.4211680474587,
    -68.6998598004534,
    15.682546124542228,
    174.76965769979392,
    -630.6793122902468,
    23.817278083610972,
    259.76544043360377,
    585.9537450399052,
    9.412345622921847e-05,
    0.0003034012626245255,
    9.06705374918394e-05,
    0.0008096445343671776,
    0.0006934384825412391,
]
ROUND_LARGE_VALUES = [0.1, 2.675, 1e-320, 123.456]
ROUND_HALF_EVEN = [(ROUND_VALUES, (0, 1, 2, 3, 6, 7)), (ROUND_LARGE_VALUES, (17, 323, 324, 400))]
SUM_BUILTIN = [
    [],
    [0.1] * 10,
    [1e100, 1.0, -1e100],
    [-0.0],
    [-0.0, -0.0],
    [1e308, 1e308],
    [1e308, 1e308, -1e308],
    [INF, 1.0],
    [INF, -INF],
    [0.1, 0.2, 0.3],
    [
        9.525102111858402e-06,
        0.00011332979587418524,
        -0.00042078142733664747,
        813.7177106428495,
        -38303.63517961313,
        -6385.4724015212505,
        0.001424087828235845,
        -255.20491454853754,
        4242.215314923073,
        -0.008807976600675347,
        -7.171009773016435,
        -0.14481538866119426,
        -6.8796268320652,
        -0.0027683528811086734,
        0.005887589630449823,
        -0.08362899784084604,
        0.050393007622902886,
        0.045889057887843525,
        2.179180380728072e-05,
        -7.638684434900758,
        -0.6700757927128536,
        -69.60309306789905,
        -15660.329104651139,
        -844.7590356384089,
        0.14605188055476792,
        -0.31975527561760897,
        1887.3975421003686,
        5.937839516431887e-05,
        0.06799355610250828,
        -5180.3325160711065,
        -0.08700000485678104,
        29425.770905533755,
        0.06438495732194298,
        43325.55887966072,
        -30.59894886230987,
        -2890.7178091930796,
        -7.658084110365362e-06,
        -0.05635844503606411,
        -0.007413195559626316,
        -20.420464290753458,
        -83.88373975997227,
        -0.019671148733139177,
        7.667676528830249,
        0.07279689393970304,
        0.4127934189930038,
        3.654461187749032,
        0.0009154624079279825,
        -0.0008340306106773359,
        -0.005360862663609285,
        -9758.738803124023,
    ],
    [1.0, 1e-16, 1e-16, 1e-16, -1.0],
]

DUMPS = [
    "{}",
    "[]",
    '{"a": []}',
    '{"a": {}}',
    "[[], {}]",
    '{"a": 1, "b": [1, 2, 3], "c": {"d": null, "e": true, "f": false}}',
    '{"floats": [0.5, 1.25, 0.1, 1e+16, 1.5e-05, -0.0, 3.0, 123456.789, 1e-07, 2.5e+20]}',
    '{"ints": [0, -1, 9007199254740993, -9223372036854775808, 18446744073709551615]}',
    (
        '{"s": "plain", "t": "quote \\" backslash \\\\ slash /", "u": "\\b\\f\\n\\r\\t", '
        '"v": "\\u0000\\u0001\\u001f\x7f"}'
    ),
    (
        '{"w": "café \u2028 \u4e2d", "x": "\U0001f600 \U00010000 \U0010ffff", '
        '"y": "\ud7ff\ue000\uffff"}'
    ),
    '{"nested": [{"a": [1, {"b": [[], [2]]}]}, "z"]}',
    '["only", 1, 2.0, null]',
    '"top-level string \xe9"',
    "42",
    "0.1",
    "null",
    "true",
]
LOADS = [
    "null",
    "true",
    "false",
    " \t\n\r 42 \r\n",
    "0",
    "-0",
    "-12",
    "99999999999999999999",
    "-99999999999999999999999999",
    "1.5",
    "-2.5e-3",
    "1E5",
    "1e+2",
    "1e400",
    "-1e400",
    "1e-400",
    "0.1",
    "NaN",
    "Infinity",
    "-Infinity",
    "[NaN, Infinity, -Infinity]",
    '""',
    '"plain"',
    '"quote \\" backslash \\\\ slash \\/"',
    '"\\b\\f\\n\\r\\t"',
    '"\\u0041\\u00e9\\u4E2D"',
    '"\\ud83d\\ude00"',
    '"\\ud800"',
    '"\\udc00"',
    '"\\ud800\\u0041"',
    '"\\ud800\\ud800"',
    '"\\udc00\\ud800"',
    '"a\\ud800"',
    '"\\ud800\\udbff\\udc00"',
    '"\\ud800\\n"',
    '"\xe9t\xe9 \U00002028 \U00004e2d"',
    '"\U0001f600"',
    '"\x7f"',
    "[]",
    "{}",
    "[ ]",
    "{ }",
    "[1, [2, [3, []]], {}]",
    '{"a": 1, "b": 2, "a": 3}',
    '{"b": 1, "a": {"c": [true, null]}}',
    '{"\\u00e9": 1, "\xe9": 2}',
    '{"": 0}',
    '{ "a" : 1 , "b" : [ 1 , 2 ] }',
    "",
    " ",
    "\n\n",
    "{",
    "[",
    "]",
    "}",
    '"abc',
    '"a\\',
    '"\\x"',
    '"\\u12"',
    '"\\u12',
    '"\\u123"',
    '"\\u1234',
    '"\\uZZZZ"',
    '"\\ud800\\u12"',
    '"\\ud800\\uZZZZ"',
    '"\\ud800\\udc00',
    '"\\ud800\\',
    '"\\ud800\\u"',
    '"a\x01b"',
    '"a\nb"',
    '"\x1f"',
    '{"a" 1}',
    "{1:2}",
    '{"a":}',
    '{"a":1,}',
    '{"a":1 , }',
    "[1,]",
    "[1,  ]",
    "[1 2]",
    '{"a":1 "b":2}',
    "{,}",
    "[,]",
    '{"a":1,,}',
    "[1,,]",
    "1 2",
    '{"a":1}x',
    "-",
    "-x",
    "1.",
    "1e",
    "1e+",
    "01",
    "1.5e",
    "-Infinityx",
    "nu",
    "t",
    "fa",
    "nan",
    "infinity",
    "+1",
    ".5",
    "'a'",
    "\U0000feff{}",
    "\U0000feff",
    '{"a":1\n\n,"b"}',
    "[\n1,\n2\n3]",
    '"\xe9" x',
    '"\U00004e2d\x0a"',
    '["\xe9", ]',
    '{"\xe9":1 "x"}',
    '{\n  "a": 1,\n  "b": \n}',
    "[[[[",
    '{"a": [1, {"b": }]}',
]
REPR = [
    "",
    "a",
    "it's",
    'say "hi"',
    "both ' and \"",
    "back\\slash",
    "\t\n\r",
    "\x00\x01\x1f\x7f",
    " ~",
    "\x80\x85\x9f\xa0\xad\xe9\xff",
    "\U00000378\U0000200b\U00002028\U00002029\U00003000\U0000e000\U0000feff\U0000ffff",
    "\U0001f600\U000e0001\U0010ffff\U00020000",
    "\U0000d800\U0000dcff\U0000dfff",
    "\U00004e2d\U00006587",
    "line\nbreak's",
]
SHLEX_QUOTE = [
    "",
    "abc",
    "a b",
    "it's",
    "\xe9t\xe9",
    "a$b",
    "_@%+=:,./-",
    "AZaz09",
    "\t",
    "a\nb",
    "-",
    "x'y'z",
    '"',
    "*",
    "~",
    "!",
    "a;b",
    "''",
]
FSDECODE = [
    b"",
    b"abc",
    b"\xff",
    b"\x80abc",
    b"\xe2\x82",
    b"\xe2\x82A",
    b"\xf0\xe2\x82\xac",
    b"\xed\xa0\x80",
    b"\xc0\x80",
    b"\xc3\xa9t\xc3\xa9",
    b"\xf4\x90\x80\x80",
    b" \xa0 ",
    b"\xc2\xa0x\xc2\x85",
]

IGNORED_TOKENS = frozenset(
    {
        tokenize.COMMENT,
        tokenize.DEDENT,
        tokenize.ENDMARKER,
        tokenize.INDENT,
        tokenize.NEWLINE,
        tokenize.NL,
    }
)


def bits(x: float) -> str:
    # The hex digits of the IEEE-754 bits keep every float exact, NaN and -0.0 included.
    (raw,) = struct.unpack("<Q", struct.pack("<d", x))
    return f"0x{raw:016x}"


def text_fixture() -> Fixture:
    return {
        "is_space": [code for code in range(sys.maxunicode + 1) if chr(code).isspace()],
        "splitlines": [
            [text, keepends, text.splitlines(keepends)]
            for text in SPLITLINES
            for keepends in (False, True)
        ],
        "split_whitespace": [[text, text.split()] for text in SPLIT_WHITESPACE],
        "strip": [[text, text.strip()] for text in STRIP],
        "not_printable": code_ranges(lambda char: not char.isprintable()),
        "repr": [[code_points(text), repr(text)] for text in REPR],
        "fsdecode": [
            [list(raw), code_points(raw.decode("utf-8", "surrogateescape"))] for raw in FSDECODE
        ],
    }


def word_fixture() -> Fixture:
    return {
        "word": code_ranges(lambda char: WORD.fullmatch(char) is not None),
    }


def shell_fixture() -> Fixture:
    return {
        "shlex_quote": [[text, shlex.quote(text)] for text in SHLEX_QUOTE],
    }


def os_fixture() -> Fixture:
    # The messages are the C library's, so the test that reads these runs on Linux only.
    return {
        "os_error_repr": [[code, repr(OSError(code, os.strerror(code)))] for code in range(141)],
    }


def difflib_fixture() -> Fixture:
    return {
        "matching_blocks": [
            [a, b, [list(block) for block in SequenceMatcher(None, a, b).get_matching_blocks()]]
            for a, b in matching_pairs()
        ],
    }


def matching_pairs() -> list[tuple[list[str], list[str]]]:
    lines = [f"line {index}" for index in range(197)]
    pairs = [
        ([], []),
        (["a"], []),
        ([], ["a"]),
        (["a", "b", "c"], ["a", "b", "c"]),
        (["a", "b", "x", "c"], ["a", "c", "b"]),
        (["x"] * 5, ["x"] * 3),
        (["a", "b", "a", "b"], ["b", "a", "b", "a"]),
        (["def f():", "    return 1", ""], ["def f():", "    return 2", ""]),
        # Four copies of "p" in 200 lines pass the 200 // 100 + 1 limit; in 199 lines nothing is
        # popular; three copies in 200 lines stay below the limit.
        (["p", *lines[:5], "p"], ["p"] * 4 + lines[:196]),
        (["p", *lines[:5], "p"], ["p"] * 4 + lines[:195]),
        (["p", *lines[:5], "p"], ["p"] * 3 + lines),
        # A popular line starts no match, so a lone "p" matches only below the limit.
        (["p"], ["x"] + ["p"] * 4 + lines[:195]),
        (["p"], ["x"] + ["p"] * 4 + lines[:194]),
        (["p"], ["x"] + ["p"] * 3 + lines[:196]),
        (["p"] * 6, ["p"] * 250),
        (["", "x", "", "y", ""] * 50, ["", "y", "", "x", ""] * 50),
    ]
    rng = random.Random(1313)
    for length, alphabet in [(30, 3), (120, 8), (240, 4), (260, 40), (400, 6)]:
        a = [f"t{rng.randrange(alphabet)}" for _ in range(length)]
        b = [f"t{rng.randrange(alphabet)}" for _ in range(length + rng.randrange(-10, 10))]
        pairs.append((a, b))
    return pairs


def code_ranges(accept: Callable[[str], bool]) -> list[list[int]]:
    # Ranges keep the fixture small: the 1,114,112 code points fall in a few hundred runs.
    ranges: list[list[int]] = []
    for code in range(sys.maxunicode + 1):
        if not accept(chr(code)):
            continue
        if ranges and ranges[-1][1] == code - 1:
            ranges[-1][1] = code
        else:
            ranges.append([code, code])
    return ranges


def code_points(text: str) -> list[int]:
    # A str can hold a lone surrogate, and JSON text cannot, so the fixtures store code points.
    return [ord(char) for char in text]


def float_fixture() -> Fixture:
    return {
        "float_repr": [[bits(x), repr(x)] for x in FLOAT_REPR],
        "round_half_even": [
            [bits(x), ndigits, bits(round(x, ndigits))]
            for values, digits in ROUND_HALF_EVEN
            for x in values
            for ndigits in digits
        ],
    }


def sum_fixture() -> Fixture:
    return {
        "sum_builtin": [
            [[bits(x) for x in values], bits(float(sum(values)))] for values in SUM_BUILTIN
        ],
    }


def json_fixture() -> Fixture:
    return {
        "dumps": [
            [doc, json.dumps(json.loads(doc), indent=2), json.dumps(json.loads(doc))]
            for doc in DUMPS
        ],
        "loads": [[doc, loaded(doc)] for doc in LOADS],
        "max_depth": [max_depth()],
    }


def loaded(doc: str) -> list[object]:
    try:
        value = json.loads(doc)
    except json.JSONDecodeError as error:
        return ["error", error.msg, error.pos, error.lineno, error.colno]
    return ["ok", tagged(value)]


def tagged(value: object) -> list[object]:
    # Tags keep what JSON fixture text cannot: NaN, big integers, and lone surrogates.
    if isinstance(value, list):
        return ["array", [tagged(item) for item in value]]
    if isinstance(value, dict):
        return ["object", [[code_points(key), tagged(item)] for key, item in value.items()]]
    return tagged_scalar(value)


def tagged_scalar(value: object) -> list[object]:
    if value is None:
        return ["null"]
    if isinstance(value, bool):
        return ["bool", value]
    if isinstance(value, int):
        return ["int", str(value)]
    if isinstance(value, float):
        return ["float", bits(value)]
    if isinstance(value, str):
        return ["str", code_points(value)]
    message = f"json.loads returned a {type(value).__name__}"
    raise TypeError(message)


def max_depth() -> int:
    # The deepest nesting json.loads reads before RecursionError, the same for arrays and objects.
    depths = {opener: deepest(opener) for opener in "[{"}
    if len(set(depths.values())) != 1:
        message = f"json.loads nests arrays and objects to different depths: {depths}"
        raise RuntimeError(message)
    return depths["["]


def deepest(opener: str) -> int:
    low, high = 1, 100_000
    while low < high:
        middle = (low + high + 1) // 2
        if reads_nested(opener, middle):
            low = middle
        else:
            high = middle - 1
    return low


def reads_nested(opener: str, depth: int) -> bool:
    doc = "[" * depth + "]" * depth if opener == "[" else '{"a": ' * depth + "0" + "}" * depth
    try:
        json.loads(doc)
    except RecursionError:
        return False
    return True


def sloc_fixture() -> Fixture:
    results = [(source, sloc(source)) for source in sloc_sources()]
    return {
        "lines": [[source, result] for source, result in results if isinstance(result, list)],
        "errors": [[source, *result] for source, result in results if isinstance(result, tuple)],
    }


def sloc(source: str) -> list[int] | tuple[str, str]:
    readline = iter(source.splitlines(keepends=True)).__next__
    try:
        tokens = list(tokenize.generate_tokens(readline))
    except (tokenize.TokenError, SyntaxError) as error:
        return type(error).__name__, error.args[0] if error.args else ""
    return sorted({token.start[0] for token in tokens if token.type not in IGNORED_TOKENS})


def sloc_sources() -> list[str]:
    # Plain code, blank lines, comments and indentation.
    sources = [
        "",
        "\n",
        "x\n",
        "x",
        "  \n\n",
        "# c\n",
        "x = 1  # c\n",
        "x = 1\n\n\ny = 2\n",
        "if a:\n    b\n\n    c\nd\n",
        "def f():\n    pass\n  # c\n",
        "class A:\n    def f(self):\n        return 1\n",
        "\n\n\nx = 1\n\n",
        "x=1;y=2\n",
        "if a:\n    b\n  # dedented comment\n    c\n",
        "    \n",
        "if a:\n    b\n   \n",
    ]
    # Strings and their prefixes.
    for p in ["", "r", "b", "u", "R", "B", "U", "rb", "Rb", "bR", "BR", "br"]:
        sources += [
            f"x = {p}'a'\n",
            f'x = {p}"a\\"b"\n',
            f"x = {p}'''a\nb\n'''\n",
            f'x = {p}"""a\nb"""\ny\n',
        ]
    sources += [
        'x = "a\\\nb"\n',
        "x = 'a' 'b'\n",
        'x = ("a"\n     "b")\n',
        '"""doc"""\n',
        '"""\ndoc\n"""\ndef f(): pass\n',
        "x = '''a\\''''\n",
        'x = """a\\"""b"""\n',
        "s = 'it''s'\n",
        'x = "#not comment"\n',
        "x = '\\\\'\n",
    ]
    # f-strings.
    for p in ["f", "F", "rf", "fr", "Rf", "fR", "RF"]:
        sources += [
            f'x = {p}"a{{b}}c"\n',
            f'x = {p}"""a\n{{b}}\nc\n"""\n',
            f"x = {p}'''\n{{b\n}}\n'''\n",
        ]
    sources += [
        'x = f"a{\nb\n}c"\n',
        'f"{x:{\ny}}"\n',
        'f"{x!r}"\n',
        'f"{x=}"\n',
        'f"{{x}}"\n',
        'f"{x:>10}"\n',
        'f"{ f"{y}" }"\n',
        'f"{\n# comment\nx}"\n',
        'f"""\n{x\n}\n{y}\n"""\n',
        'f"{x}\\\ny"\n',
        'f"a{b}c{d}e"\nz = 1\n',
        'f"""{a}\n\n{b}"""\n',
        "f'{a[\"k\"]}'\n",
        'f"{lambda: 1}"\n',
        'f"{x:{y}.{z}}"\n',
        'f""\n',
        'f"""\n"""\n',
        'f"{x}"f"{y}"\n',
    ]
    # Continuation lines, brackets, form feeds and Unicode line breaks.
    sources += [
        "a = 1\\\n+ 2\n",
        "a = (1,\n     2)\n",
        "a = [\n]\n",
        "a = {\n  1: 2,\n}\n",
        "x = (\n\n  1\n)\n",
        "a\x0cb = 1\n",
        "\x0cx = 1\n",
        "x = 1\u2028y = 2\n",
        "x = 1\x1cy = 2\n",
        "x = 1\x85y\n",
        "x = 1\r\ny = 2\r\n",
        "x = 1\ry = 2\r",
        "if a:\\\n  b\n",
        "x = 1 \\\n\n",
        "def f(a,\n      b):\n    pass\n",
        "x = [1,\n# c\n2]\n",
    ]
    # Unusual characters and tokens.
    sources += [
        "a $ b\n",
        "a ? b\n",
        "x = 1 ! 2\n",
        "a = `b`\n",
        "x = 0x1F + 0o7 + 0b1 + 1_000 + 1.5e-3j\n",
        "x = ...\n",
        "@dec\ndef f(): pass\n",
        "x := 1\n",
        "x -> y\n",
        "café = 1\n",
        "x = 'café'\n",
        "\ufeffx = 1\n",
        "x = 1\t# tab\n",
        "\tx = 1\n",
        "if a:\n\tb\n\tc\n",
        "x = 1\x00\n",
        "x = \xa0 1\n",
    ]
    # Errors.
    sources += [
        '"abc\n',
        "'abc",
        '"""abc\n',
        "'''abc",
        "(1,\n",
        "[1,\n2\n",
        "x = (\n",
        "if x:\n  a\n b\n",
        "if x:\n\ta\n        b\n",
        "  x = 1\n",
        "x = 1\n  y = 2\n",
        'f"{x\n',
        'f"{x"\n',
        'f"abc\n',
        'f"""abc\n',
        'f"}"\n',
        'f"{"\n',
        'f"{x!}"\n',
        "x = 1 \\ y\n",
        "x = 1\\",
        ")\n",
        "(]\n",
        "x = 'a\\\n",
        "def f():\nreturn 1\n",
        'x = "\u2028"\n',
        "x = '\x0c'\n",
        "if a:\n    b\n   c\n",
        "x = f'{a!r:{b!r}}'\n",
    ]
    # Nesting limits.
    sources += [
        "(" * 199 + ")" * 199 + "\n",
        "(" * 201 + "\n",
        "if a:\n" + "".join(" " * (i + 1) + f"if a{i}:\n" for i in range(101)),
    ]
    return sources + sloc_edge_sources()


def sloc_edge_sources() -> list[str]:
    # The byte order mark and break characters other than \n and \r are ordinary characters.
    sources = [
        "\ufeff\n",
        "x\n\ufeff\n",
        "\ufeff  x\ny\n",
        "x\n\u2028y\n",
        "x\n\x85\n",
        "x\n\x0b\n",
        "# c\x0b\nx\n",
        "  \x0b\n",
    ]
    # Number literals.
    sources += [
        "0x\n",
        "0x_\n",
        "0x1__2\n",
        "0o\n",
        "0o8\n",
        "0o17\n",
        "0b\n",
        "0b12\n",
        "0b1_\n",
        "1__0\n",
        "1_\n",
        "0_x\n",
        "1e+_\n",
        "1e_\n",
        ".5_\n",
        "1abc\n",
        "0777\n",
        "1_000.5_5e1_0j\n",
        "1f'{x}'\n",
        "0x1f'{x}'\n",
        "1.f'{x}'\n",
    ]
    # String errors: the detection line and the escaped-quote hint.
    sources += [
        "x = 'a\\\nb\n",
        "x = 'a\\'\n",
        "x = 'a\\\\'\n",
        "x = '''a\\'''\n",
        "'abc\n\x00\n",
        "'''abc\n\x00\n",
    ]
    # Null bytes and non-printable characters.
    sources += [
        "x = (1\n\x00\n",
        "x\n# \x00\n",
        "x = '\x01'\n",
        "# \x01\n",
        "x \x7f\n",
        "f'{x}\x01'\n",
        "\x01\n",
    ]
    # f-string literal runs: {{, }} and \N{...} split a run.
    sources += [
        "f'''a\n{{\nb'''\n",
        "f'''a\n}}\nb'''\n",
        "f'''a\n\\N{BULLET}b'''\n",
        "f'''a\n\\N{\nBULLET}b'''\n",
        "rf'''a\n\\N{\nb}'''\n",
        "f'{{}}'\n",
        "f'\\N{x'\n",
        "f'{x:{y}}}}'\n",
    ]
    # f-string format specs and fields.
    sources += [
        "f'{x:\n}'\n",
        "f'''{x:\nabc}'''\n",
        "f'''{x:\n{y}\nz}'''\n",
        "f'{x:'\n",
        "f'{x:{{y}}'\n",
        "f'{x\n}'\n",
        "f'''{\n\n}'''\n",
        "f'''{x\n'''\n",
        "f\"{x'\n",
        "f'{x' + 'y'\n",
        "f'{x}' f'''\n{y}'''\n",
    ]
    # f-string nesting and unmatched closers.
    sources += ["f'{x:{y:{z:{w}}}}'\n", "f'{x:{y:{z}}}'\n", "f'{)}'\n", "f'{))}'\n", "f'{)\n"]
    # Indentation through backslash continuations.
    sources += [
        "if a:\n    b\n  \\\n    c\n",
        "if a:\n    \\\n  b\n",
        "  \\\n  x\n",
        "\\\n    x\n",
        "x = 'a' \\\n",
        "x = 'abc' \\\n\n",
        "if a:\n        b\n\tc\n",
    ]
    return sources


# Each fixture file, by its path below crates/, with the builder of its sections.
BUILDERS: dict[str, Callable[[], Fixture]] = {
    "text/fixtures/text.json": text_fixture,
    "json/fixtures/json.json": json_fixture,
    "json/fixtures/float.json": float_fixture,
    "metrics/fixtures/tokenize.json": sloc_fixture,
    "metrics/fixtures/word.json": word_fixture,
    "metrics/fixtures/sum.json": sum_fixture,
    "checks/fixtures/diff.json": difflib_fixture,
    "io/fixtures/oserror.json": os_fixture,
    "store/fixtures/shell.json": shell_fixture,
}


def fixtures() -> dict[Path, bytes]:
    return {CRATES / path: render(build()) for path, build in BUILDERS.items()}


if __name__ == "__main__":
    sys.exit(write_or_check_files(__doc__.splitlines()[0], CRATES, fixtures))
