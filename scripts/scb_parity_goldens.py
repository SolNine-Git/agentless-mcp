"""Write the scb-check v0.1.3 parity goldens for the Python complexity fixtures.

Development-time only. Run it from the repository root under uvx, so scb-check
never becomes a dependency of this package:

    env -u VIRTUAL_ENV uvx --python 3.13 --from scb-check==0.1.3 \
        python -I scripts/scb_parity_goldens.py
"""

import argparse
import json
import sys
from collections import defaultdict
from importlib import metadata
from pathlib import Path
from tokenize import TokenError

from scb_check.analysis.clones import detect_clones
from scb_check.analysis.loc import sloc_line_numbers
from scb_check.analysis.parse import ParseError, parse_file
from scb_check.analysis.symbols import extract_functions
from scb_check.pipeline import analyze_files
from scb_check.reporting.score import compute_report

REPO = Path(__file__).resolve().parents[1]
FIXTURES = REPO / "tests" / "characterization" / "fixtures" / "complexity_py"
GOLDENS = REPO / "tests" / "characterization" / "goldens" / "complexity"
REFERENCE_VERSION = "0.1.3"
MIN_PYTHON = (3, 12)


def main(argv: list[str] | None = None) -> int:
    """Write one golden per fixture and return the exit status."""
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--fixtures", type=Path, default=FIXTURES)
    parser.add_argument("--out", type=Path, default=GOLDENS)
    args = parser.parse_args(argv)

    version = metadata.version("scb-check")
    if sys.version_info < MIN_PYTHON or version != REFERENCE_VERSION:
        found = f"Python {sys.version.split()[0]} and scb-check {version}"
        print(
            f"refused: the goldens need Python 3.12 or later and scb-check "
            f"{REFERENCE_VERSION}; found {found}",
            file=sys.stderr,
        )
        return 2

    args.out.mkdir(parents=True, exist_ok=True)
    for path in sorted(args.fixtures.glob("*.py")):
        target = args.out / f"{path.stem}.json"
        golden = json.dumps(reference(path), indent=2, sort_keys=True, ensure_ascii=False)
        target.write_text(golden + "\n", encoding="utf-8")
        print(f"wrote {target}")
    return 0


def reference(path: Path) -> dict[str, object]:
    """Return scb-check's units, erosion, and clone lines for one fixture file."""
    record: dict[str, object] = {
        "fixture": path.resolve().relative_to(REPO).as_posix(),
        "reference": f"scb-check {REFERENCE_VERSION}",
        "python": f"{sys.version_info.major}.{sys.version_info.minor}",
    }
    try:
        source, tree = parse_file(path)
    except ParseError as exc:
        return {**record, "status": "parse_error", "error": type(exc).__name__}
    try:
        sloc_lines = sloc_line_numbers(source, tree)
    except (TokenError, SyntaxError) as exc:
        return {**record, "status": "tokenize_error", "error": type(exc).__name__}

    units = sorted(
        (symbol.start_line, symbol.name, symbol.cyc_complexity, symbol.sloc)
        for symbol in extract_functions(path, tree, sloc_lines)
    )
    analysis = analyze_files((path.resolve(),), include_all=True, disable_sg=True)
    pipeline_units = sorted(
        (symbol.start_line, symbol.name, symbol.cyc_complexity, symbol.sloc)
        for symbol in analysis.flags.all_functions
    )
    if pipeline_units != units:
        message = f"{path}: extract_functions and the scb-check pipeline disagree on units"
        raise RuntimeError(message)
    report = compute_report(analysis.flags)

    groups: defaultdict[str, list[list[int]]] = defaultdict(list)
    for block in detect_clones(((path, source, tree),)):
        groups[block.group_hash].append([block.start_line, block.end_line])
    clone_lines = sorted(
        line for entry in analysis.flags.clone_sloc_lines_by_file for line in entry.lines
    )

    return {
        **record,
        "status": "ok",
        "units": [
            {"name": name, "start_line": start, "cc": cc, "sloc": sloc}
            for start, name, cc, sloc in units
        ],
        "erosion": report.erosion,
        "total_mass": report.total_mass,
        "high_cc_mass": report.high_cc_mass,
        "clones": {
            "groups": sorted(sorted(group) for group in groups.values()),
            "sloc_lines": clone_lines,
        },
    }


if __name__ == "__main__":
    sys.exit(main())
