"""Write the app crate's guide fixtures: the packaged guide, its sections, and the refusal texts.

Development-time only. Run it from the repository root, with scratch cache directories:

    XDG_CACHE_HOME=<scratch> TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=<scratch> \
        .venv/bin/python scripts/guide_fixtures.py

The synthetic section runs the Python section rules over small guide texts, for the generator's
own tests. With --check, the script compares its output with the committed fixtures and writes
nothing.
"""

import argparse
import ast
from pathlib import Path
from unittest import mock

from fixture_files import Fixture, refuse_real_caches, write_or_check

from agentless_mcp.adapters.cli import main
from agentless_mcp.core import guide

FIXTURES = Path(__file__).resolve().parents[1] / "crates" / "app" / "fixtures" / "guide"
NBSP = chr(0xA0)
IDEOGRAPHIC_SPACE = chr(0x3000)
EM_DASH = chr(0x2014)
SURROGATE = chr(0xDCFF)

UNKNOWN = ["nope", "", "it's", 'say "x"', "a\nb", "café", SURROGATE, "REFS", " refs"]

SYNTHETIC = [
    "",
    "# Title\n\n## A\ntext\n### B\nmore\n## C\nend\n",
    "## A\n```\n## not\n```\n## B\n",
    "## A\n```\n## x\n````\n## B\n",
    "## A\n````\n## x\n```\n## y\n````\n## B\n",
    "## A\n~~~\n```\n## x\n~~~\n## B\n",
    "## A\n```\n## x\n``` text\n## y\n```\n## B\n",
    "## A\n```python\n## x\n```\n## B\n",
    "## A\n    ```\n## B\n   ```\n## x\n   ```\n    ## y\n## C\n",
    "## A\n```\n## never closed\n",
    "# one\n## two\n### three\n#### four\n##### five\n###### six\n####### seven\n## eight\n",
    "## Name ##\n## Other #\n## C#\n##\n## \n",
    "## # \n## Keep\n",
    f"##{NBSP}Name\n## {IDEOGRAPHIC_SPACE}Spaced{IDEOGRAPHIC_SPACE} ##\n",
    "## `refs` (`find_referencing_symbols`) -- fan-in\n## a (b (c) d) e\n## unclosed (paren\n",
    "## Mixed_Case-Name 2\n## --lead\n## x -- y -- z\n## (only)x\n",
    "## Mixed_Case-Name 2\n## --lead\n## x -- y -- z\n## (only)y\n## a(b)c\n",
    "## A\ntext\n\n\n## B\n\n",
    "## A\nlast",
    "##\tTab\n\t## not heading\n## B\ttab\n",
    "# T\nbody\n",
    f"## Café {EM_DASH} menu\n### Sub\n#### Para\ntext\n### Sub two\n",
    "## Same\n## same\n### SAME!\n",
    "## A\n## B\n## a\n### b\n",
]


def unknown_refusal(name: str) -> str:
    messages: list[str] = []

    def capture(message: str, code: int) -> int:
        messages.append(message)
        return code

    with mock.patch.object(main, "fail", side_effect=capture):
        main._cmd_guide(argparse.Namespace(section=name), services=None)
    return messages[0]


def duplicate_names(error: guide.GuideDataError) -> list[str]:
    listed = str(error).split(" section name ", maxsplit=1)[1].split("]", maxsplit=1)[0] + "]"
    return ast.literal_eval(listed)


def synthetic_sections(text: str) -> object:
    guide._sections.cache_clear()
    try:
        with mock.patch.object(guide, "_resource_text", return_value=text):
            names = guide.section_names()
            return [[name, guide.section_text(name)] for name in names]
    except guide.GuideDataError as error:
        return {"duplicate": duplicate_names(error)}
    finally:
        guide._sections.cache_clear()


def guide_fixture() -> Fixture:
    names = list(guide.section_names())
    return {
        "text": [guide.guide_text()],
        "names": [names],
        "sections": [[name, guide.section_text(name)] for name in names],
        "unknown": [[[ord(c) for c in name], unknown_refusal(name)] for name in UNKNOWN],
        "synthetic": [[text, synthetic_sections(text)] for text in SYNTHETIC],
    }


BUILDERS = {"guide": guide_fixture}


if __name__ == "__main__":
    refuse_real_caches()
    raise SystemExit(write_or_check((__doc__ or "").splitlines()[0], FIXTURES, BUILDERS))
