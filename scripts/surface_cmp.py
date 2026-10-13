"""Compare two surface_dump.py output directories, answer by answer.

usage: surface_cmp.py BASE_DIR NEW_DIR [--diffs FILE] [--examples N] [--seed N]

The comparison refuses, with exit status 1 and before it reads an answer, two dumps that read
different target lists, two builds that listed different find-symbol frames, and a dump that
has no answer for a target. An answer changed when its exit code, its stdout or its stderr
differ once the receipt lines are removed. The report gives the changed answers per surface,
the refs tier transitions keyed by (path, line), the loss count, the truncated answers, and a
seeded sample of the changed refs lines with their source text. --diffs receives a unified
diff of every changed answer.
"""

import argparse
import difflib
import json
import random
import re
import sys
from collections import Counter
from collections.abc import Callable, Iterator, Mapping, Sequence
from dataclasses import dataclass, field
from itertools import chain, dropwhile
from pathlib import Path
from typing import Any

BUILD = "build.json"
TARGETS = "targets.json"
FRAME = "frame.json"
ANSWERS = "answers"
DUMP_FILES = (BUILD, TARGETS, FRAME)
TIER_ORDER = ("same_file", "imported", "unique", "ambiguous")
ABSENT = "absent"
RECEIPT_HEADER = "// agentless-mcp receipt"
RECEIPT_PREFIX = "// "
JSON_OPEN = "{"
JSON_RECEIPT_OPEN = '  "receipt": {'
JSON_RECEIPT_CLOSE = "  },"
CEILING_MARK = "... output truncated at the "
MAP_FILE_ROW = re.compile(r"\S.*  \(rank [^)]+\)")
CYCLE_COUNT = re.compile(r"(\d+) import cycles?")
NO_CYCLES = "no import cycles"
FAN_IN = re.compile(r"^referenced by \(fan-in\): (\d+|none)$", re.MULTILINE)
FAN_OUT = re.compile(r"^references \(fan-out\): (\d+|none)$", re.MULTILINE)
NO_FAN = "none"
SOURCE_CHARS = 110
DIGEST_CHARS = 12
DEFAULT_EXAMPLES = 4
EXIT_REFUSED = 1

Site = tuple[str, int]
Move = tuple[str, str]
Size = tuple[int, ...]


@dataclass(frozen=True)
class Answer:
    """One recorded call: its exit code (None after a timeout), its stdout and its stderr."""

    code: int | None
    stdout: str
    stderr: str


@dataclass(frozen=True)
class Target:
    """The queries one Loc-Bench instance receives: its checkout, seeds and sampled ids."""

    instance_id: str
    root: str
    seeds: list[str]
    name_seeds: list[str]
    sampled: list[str]


def parse_answer(raw: Mapping[str, Any]) -> Answer:
    """Return the answer that one recorded call holds, or raise on a field of the wrong type."""
    code, stdout, stderr = raw["code"], raw["stdout"], raw["stderr"]
    if code is not None and not isinstance(code, int):
        message = f"answer exit code is not an integer: {code!r}"
        raise TypeError(message)
    if not isinstance(stdout, str) or not isinstance(stderr, str):
        message = "answer stdout or stderr is not text"
        raise TypeError(message)
    return Answer(code, stdout, stderr)


def parse_targets(text: str) -> list[Target]:
    """Return the targets a targets.json file holds, or raise on a field of the wrong type."""
    targets = [Target(**entry) for entry in json.loads(text)]
    for target in targets:
        if not _well_formed(target):
            message = f"malformed target: {target.instance_id!r}"
            raise TypeError(message)
    return targets


def _well_formed(target: Target) -> bool:
    lists = (target.seeds, target.name_seeds, target.sampled)
    if not all(isinstance(value, list) for value in lists):
        return False
    words = [target.instance_id, target.root, *chain.from_iterable(lists)]
    return all(isinstance(word, str) for word in words)


def strip_receipt(stream: str) -> str:
    """Return ``stream`` without the receipt that opens it, as text lines or as a JSON object."""
    lines = stream.split("\n")
    if lines[0].startswith(RECEIPT_HEADER):
        return "\n".join(dropwhile(lambda line: line.startswith(RECEIPT_PREFIX), lines[1:]))
    if lines[:2] == [JSON_OPEN, JSON_RECEIPT_OPEN] and JSON_RECEIPT_CLOSE in lines:
        return "\n".join([JSON_OPEN, *lines[lines.index(JSON_RECEIPT_CLOSE) + 1 :]])
    return stream


def answer_changed(base: Answer, new: Answer) -> bool:
    """Return True when the exit code, stdout or stderr differ once the receipts are removed."""
    return base.code != new.code or _visible(base) != _visible(new)


def _visible(answer: Answer) -> tuple[str, str]:
    return strip_receipt(answer.stdout), strip_receipt(answer.stderr)


def refs_sites(stdout: str) -> dict[Site, str] | None:
    """Return the best tier of each (path, line) in a refs JSON answer, None if unparsable."""
    try:
        payload = json.loads(stdout)
    except json.JSONDecodeError:
        return None
    if not isinstance(payload, dict) or not isinstance(payload.get("groups"), list):
        return None
    best: dict[Site, str] = {}
    for group in payload["groups"]:
        rows = _group_rows(group)
        if rows is None:
            return None
        for site, tier in rows:
            if site not in best or _tier_rank(tier) < _tier_rank(best[site]):
                best[site] = tier
    return best


def _group_rows(group: object) -> list[tuple[Site, str]] | None:
    if not isinstance(group, dict):
        return None
    path, tier, lines = group.get("path"), group.get("tier"), _site_lines(group.get("sites"))
    if not isinstance(path, str) or not isinstance(tier, str) or lines is None:
        return None
    return [((path, line), tier) for line in lines]


def _site_lines(sites: object) -> list[int] | None:
    if not isinstance(sites, list):
        return None
    lines = [site.get("line") if isinstance(site, dict) else None for site in sites]
    numbers = [line for line in lines if isinstance(line, int)]
    return numbers if len(numbers) == len(lines) else None


def _tier_rank(tier: str) -> int:
    return TIER_ORDER.index(tier) if tier in TIER_ORDER else len(TIER_ORDER)


def site_moves(base: Mapping[Site, str], new: Mapping[Site, str]) -> list[tuple[Site, Move]]:
    """Return each (path, line) of either answer with its tier in base and its tier in new."""
    return [
        (site, (base.get(site, ABSENT), new.get(site, ABSENT))) for site in sorted({*base, *new})
    ]


def answer_size(surface: str, stdout: str) -> Size | None:
    """Return the size that the loss count compares for one answer, None if it has none."""
    sizer = SIZERS.get(surface)
    return None if sizer is None else sizer(stdout)


def _refs_size(stdout: str) -> Size | None:
    sites = refs_sites(stdout)
    return None if sites is None else (len(sites),)


def _map_size(stdout: str) -> Size | None:
    return (sum(1 for line in stdout.split("\n") if MAP_FILE_ROW.fullmatch(line)),)


def _cycles_size(stdout: str) -> Size | None:
    for line in stdout.split("\n"):
        if line == NO_CYCLES:
            return (0,)
        match = CYCLE_COUNT.fullmatch(line)
        if match:
            return (int(match.group(1)),)
    return None


def _explain_size(stdout: str) -> Size | None:
    fan_in, fan_out = _fan(FAN_IN, stdout), _fan(FAN_OUT, stdout)
    if fan_in is None or fan_out is None:
        return None
    return (fan_in, fan_out)


def _fan(pattern: re.Pattern[str], stdout: str) -> int | None:
    match = pattern.search(stdout)
    if match is None:
        return None
    return 0 if match.group(1) == NO_FAN else int(match.group(1))


SIZERS: dict[str, Callable[[str], Size | None]] = {
    "refs": _refs_size,
    "map": _map_size,
    "cycles": _cycles_size,
    "explain": _explain_size,
}


def shrank(base: Size | None, new: Size | None) -> bool:
    """Return True when both sizes are known and a part of ``new`` is below ``base``."""
    if base is None or new is None:
        return False
    return any(after < before for before, after in zip(base, new, strict=True))


def exit_lost(base: int | None, new: int | None) -> bool:
    """Return True when a call that exited 0 in base failed or timed out in new."""
    return base == 0 and new != 0


def is_truncated(stdout: str) -> bool:
    """Return True when an answer says that the token ceiling cut it."""
    try:
        payload = json.loads(stdout)
    except json.JSONDecodeError:
        return any(line.startswith(CEILING_MARK) for line in stdout.split("\n"))
    return isinstance(payload, dict) and "truncated" in payload


def frame_differences(base: Mapping[str, Any], new: Mapping[str, Any]) -> list[str]:
    """Return each instance and kind whose find-symbol listing differs between two frames."""
    found = [f"{instance}: only in one frame" for instance in sorted(set(base) ^ set(new))]
    for instance in sorted(set(base) & set(new)):
        kinds = sorted(set(base[instance]) | set(new[instance]))
        found.extend(
            f"{instance} {kind}"
            for kind in kinds
            if base[instance].get(kind) != new[instance].get(kind)
        )
    return found


def refusals(base_dir: Path, new_dir: Path) -> list[str]:
    """Return why the two dumps cannot be compared, or an empty list when they can."""
    dumps = (base_dir, new_dir)
    absent = [f"{d} has no {f}" for d in dumps for f in DUMP_FILES if not (d / f).is_file()]
    if absent:
        return absent
    if (base_dir / TARGETS).read_bytes() != (new_dir / TARGETS).read_bytes():
        return ["the two dumps read different targets.json files"]
    return [*_frame_refusals(base_dir, new_dir), *_answer_refusals(base_dir, new_dir)]


def _frame_refusals(base_dir: Path, new_dir: Path) -> list[str]:
    if (base_dir / FRAME).read_bytes() == (new_dir / FRAME).read_bytes():
        return []
    frames = [json.loads((d / FRAME).read_text(encoding="utf-8")) for d in (base_dir, new_dir)]
    differ = frame_differences(*frames)
    return [f"the two builds listed different find-symbol frames: {', '.join(differ)}"]


def _answer_refusals(base_dir: Path, new_dir: Path) -> list[str]:
    targets = parse_targets((base_dir / TARGETS).read_text(encoding="utf-8"))
    found = []
    for directory in (base_dir, new_dir):
        missing = [t.instance_id for t in targets if not _answers_path(directory, t).is_file()]
        if missing:
            found.append(f"{directory} has no answers for {len(missing)} targets: {missing[:5]}")
    return found


def _answers_path(directory: Path, target: Target) -> Path:
    return directory / ANSWERS / f"{target.instance_id}.json"


@dataclass
class Tally:
    """The counts and listings that the comparison of every answer collects."""

    labels: tuple[str, str]
    calls: Counter[str] = field(default_factory=Counter)
    changed: Counter[str] = field(default_factory=Counter)
    stderr_changed: Counter[str] = field(default_factory=Counter)
    exit_changes: list[str] = field(default_factory=list)
    losses: list[str] = field(default_factory=list)
    truncated: Counter[str] = field(default_factory=Counter)
    unparsable: Counter[str] = field(default_factory=Counter)
    moves: Counter[Move] = field(default_factory=Counter)
    examples: dict[Move, list[tuple[str, str, Site]]] = field(default_factory=dict)
    diffs: list[str] = field(default_factory=list)


def compare_answer(tally: Tally, instance: str, key: str, base: Answer, new: Answer) -> None:
    """Add one answer pair to ``tally``: changes, losses, truncation and refs tier moves."""
    surface = key.split(":", 1)[0]
    where = f"{instance} {key}"
    tally.calls[surface] += 1
    tally.truncated["base"] += is_truncated(base.stdout)
    tally.truncated["new"] += is_truncated(new.stdout)
    if base.code != new.code:
        tally.exit_changes.append(f"{where}: {_code(base.code)} -> {_code(new.code)}")
    if exit_lost(base.code, new.code):
        tally.losses.append(f"{where}: exit 0 -> {_code(new.code)}")
    sizes = answer_size(surface, base.stdout), answer_size(surface, new.stdout)
    if shrank(*sizes):
        tally.losses.append(f"{where}: {surface} size {sizes[0]} -> {sizes[1]}")
    if surface == "refs":
        _compare_refs(tally, instance, key, base, new)
    if strip_receipt(base.stderr) != strip_receipt(new.stderr):
        tally.stderr_changed[surface] += 1
    if answer_changed(base, new):
        tally.changed[surface] += 1
        tally.diffs.append(render_diff(where, base, new, tally.labels))


def _code(code: int | None) -> str:
    return "timeout" if code is None else str(code)


def _compare_refs(tally: Tally, instance: str, key: str, base: Answer, new: Answer) -> None:
    base_sites, new_sites = refs_sites(base.stdout), refs_sites(new.stdout)
    tally.unparsable["base"] += base_sites is None
    tally.unparsable["new"] += new_sites is None
    if base_sites is None or new_sites is None:
        return
    for site, move in site_moves(base_sites, new_sites):
        tally.moves[move] += 1
        if move[0] != move[1]:
            tally.examples.setdefault(move, []).append((instance, key, site))


def render_diff(where: str, base: Answer, new: Answer, labels: tuple[str, str]) -> str:
    """Return a header line and the unified diff of each stream, receipts removed."""
    parts = [f"===== {where}  exit {_code(base.code)} -> {_code(new.code)}"]
    streams = zip(("stdout", "stderr"), _visible(base), _visible(new), strict=True)
    for stream, before, after in streams:
        parts.extend(
            difflib.unified_diff(
                before.splitlines(),
                after.splitlines(),
                f"{labels[0]} {stream}",
                f"{labels[1]} {stream}",
                lineterm="",
                n=1,
            )
        )
    return "\n".join(parts)


def report_lines(tally: Tally) -> Iterator[str]:
    """Yield the report's counts: per surface, exit codes, losses, truncation and tier moves."""
    yield f"base: {tally.labels[0]}"
    yield f"new:  {tally.labels[1]}"
    yield "surface   calls  changed  stderr changed"
    for surface in sorted(tally.calls):
        counts = tally.calls[surface], tally.changed[surface], tally.stderr_changed[surface]
        yield f"{surface:<9} {counts[0]:>5}  {counts[1]:>7}  {counts[2]:>14}"
    yield f"exit-code changes: {len(tally.exit_changes)}"
    yield from (f"  {line}" for line in tally.exit_changes)
    yield f"loss count: {len(tally.losses)}"
    yield from (f"  {line}" for line in tally.losses)
    yield f"truncated answers: base {tally.truncated['base']}, new {tally.truncated['new']}"
    unparsable = tally.unparsable
    yield f"refs answers that are not JSON: base {unparsable['base']}, new {unparsable['new']}"
    yield "refs lines by tier transition (base -> new):"
    for move, count in sorted(tally.moves.items(), key=lambda item: (-item[1], item[0])):
        yield f"  {move[0]:>10} -> {move[1]:<10} {count}"


def example_lines(
    tally: Tally, roots: Mapping[str, str], per_move: int, seed: int
) -> Iterator[str]:
    """Yield a seeded sample of the refs lines that changed tier, each with its source text."""
    rng = random.Random(seed)
    for move, found in sorted(tally.examples.items()):
        yield f"examples {move[0]} -> {move[1]} ({len(found)}):"
        for instance, key, (path, line) in rng.sample(found, min(per_move, len(found))):
            yield f"  {instance} {key}"
            yield f"      {path}:{line}  {source_line(roots[instance], path, line)}"


def source_line(root: str, path: str, line: int) -> str:
    """Return line ``line`` of ``root``/``path``, stripped and cut to a report width."""
    try:
        text = (Path(root) / path).read_text(encoding="utf-8", errors="replace")
    except OSError as error:
        return f"<unreadable: {error.strerror}>"
    lines = text.splitlines()
    return lines[line - 1].strip()[:SOURCE_CHARS] if 0 < line <= len(lines) else "<out of range>"


def build_label(directory: Path) -> str:
    """Return the label of a dump's build: its identity kind and digest, and its argv prefix."""
    build = json.loads((directory / BUILD).read_text(encoding="utf-8"))
    identity = build["identity"]
    prefix = " ".join(build["argv_prefix"])
    return f"{identity['kind']}:{identity['sha256'][:DIGEST_CHARS]} ({prefix})"


def compare_dumps(base_dir: Path, new_dir: Path, targets: Sequence[Target]) -> Tally:
    """Compare every answer of the two dumps, target by target."""
    tally = Tally((build_label(base_dir), build_label(new_dir)))
    for target in targets:
        base, new = (_load_answers(_answers_path(d, target)) for d in (base_dir, new_dir))
        if base.keys() != new.keys():
            sys.exit(f"refused: {target.instance_id} holds different queries in the two dumps")
        for key in base:
            compare_answer(tally, target.instance_id, key, base[key], new[key])
    return tally


def _load_answers(path: Path) -> dict[str, Answer]:
    raw = json.loads(path.read_text(encoding="utf-8"))
    return {key: parse_answer(record) for key, record in raw["answers"].items()}


def main() -> int:
    """Compare two dumps and print the report, or print the refusals and return 1."""
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    parser.add_argument("base", type=Path, help="the surface_dump.py output of the base build")
    parser.add_argument("new", type=Path, help="the surface_dump.py output of the new build")
    parser.add_argument("--diffs", type=Path, help="write the unified diffs to this file")
    parser.add_argument("--examples", type=int, default=DEFAULT_EXAMPLES, help="lines per move")
    parser.add_argument("--seed", type=int, default=0, help="seed of the example sample")
    args = parser.parse_args()
    found = refusals(args.base, args.new)
    if found:
        print("\n".join(f"refused: {reason}" for reason in found), file=sys.stderr)
        return EXIT_REFUSED
    targets = parse_targets((args.base / TARGETS).read_text(encoding="utf-8"))
    tally = compare_dumps(args.base, args.new, targets)
    roots = {target.instance_id: target.root for target in targets}
    print(f"targets: {len(targets)} instances, one targets.json, identical find-symbol frames")
    print("\n".join(report_lines(tally)))
    print("\n".join(example_lines(tally, roots, args.examples, args.seed)))
    if args.diffs is not None:
        args.diffs.write_text("\n\n".join(tally.diffs) + "\n", encoding="utf-8")
        print(f"unified diffs of {len(tally.diffs)} changed answers in {args.diffs}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
