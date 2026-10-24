"""The pure comparison functions of scripts/surface_cmp.py, on small recorded answers."""

import importlib
import json
from pathlib import Path
from types import ModuleType

import pytest

SCRIPTS = Path(__file__).resolve().parents[2] / "scripts"
TEXT_RECEIPT = (
    "// agentless-mcp receipt (repository data below)\n"
    "// repo: /r   head: abc   dirty: 0 files   cache: {cache}\n"
    "// config: /r/.agentless-mcp.json\n"
)
TARGETS = json.dumps(
    [{"instance_id": "i-1", "root": "/r", "seeds": [], "name_seeds": [], "sampled": []}]
)
# The two body notes that open an answer below its receipt, in the forms the builds print.
SKIPPED_WARNING = (
    "// warning: 1 files were skipped and are absent from this answer: x.bin. A symbol in a "
    "skipped file reads as missing here even though it may exist in the repository."
)
SEEDS_NOTE = (
    "// note: these focus seeds matched no file, no symbol, and no name spelled in five or fewer "
    "non-test files of this repository, so the ranking below is not focused on them: {seeds}. "
    "Check the spelling, or seed with a file path from the tree."
)


@pytest.fixture
def cmp(monkeypatch: pytest.MonkeyPatch) -> ModuleType:
    monkeypatch.syspath_prepend(SCRIPTS)
    return importlib.import_module("surface_cmp")


def text_answer(body: str, cache: str = "none") -> str:
    return TEXT_RECEIPT.format(cache=cache) + body


def refs_answer(groups: list[dict[str, object]], cache: str = "none", **extra: object) -> str:
    document = {"receipt": {"repo": "/r", "cache": cache}, "notice": "data", **extra}
    return json.dumps({**document, "groups": groups}, indent=2)


def group(path: str, tier: str, *lines: int) -> dict[str, object]:
    sites = [{"line": line, "enclosing": "f", "stable_id": None} for line in lines]
    return {"path": path, "count": len(lines), "tier": tier, "sites": sites}


def write_dump(directory: Path, targets: str, frame: str | None, answered: list[str]) -> Path:
    (directory / "answers").mkdir(parents=True)
    (directory / "build.json").write_text("{}", encoding="utf-8")
    (directory / "targets.json").write_text(targets, encoding="utf-8")
    if frame is not None:
        (directory / "frame.json").write_text(frame, encoding="utf-8")
    for instance in answered:
        (directory / "answers" / f"{instance}.json").write_text("{}", encoding="utf-8")
    return directory


def test_a_text_receipt_and_its_config_line_are_removed(cmp: ModuleType) -> None:
    assert cmp.strip_receipt(text_answer("a.py  (rank 0.5)\n")) == "a.py  (rank 0.5)\n"


def test_a_json_receipt_object_is_removed_and_the_rest_is_kept(cmp: ModuleType) -> None:
    stripped = cmp.strip_receipt(refs_answer([], cache="g:1 fresh"))
    assert stripped == cmp.strip_receipt(refs_answer([], cache="none"))
    assert json.loads(stripped) == {"notice": "data", "groups": []}


def test_a_receipt_on_stderr_with_its_summary_line_is_removed(cmp: ModuleType) -> None:
    stderr = text_answer("// 12 modules, 30 edges\nagentless-mcp: caveat\n")
    assert cmp.strip_receipt(stderr) == "agentless-mcp: caveat\n"


def test_a_stream_without_a_receipt_is_unchanged(cmp: ModuleType) -> None:
    assert cmp.strip_receipt("// not a receipt\nbody\n") == "// not a receipt\nbody\n"
    assert cmp.strip_receipt("") == ""


def test_answers_that_differ_only_in_the_receipt_are_unchanged(cmp: ModuleType) -> None:
    base = cmp.Answer(0, text_answer("body\n", cache="none"), "")
    new = cmp.Answer(0, text_answer("body\n", cache="g:9 fresh"), "")
    assert not cmp.answer_changed(base, new)


def test_a_body_note_below_the_receipt_is_part_of_the_answer(cmp: ModuleType) -> None:
    rows = "a.py  (rank 0.5)\n"
    seeds = [text_answer(f"{SEEDS_NOTE.format(seeds=seed)}\n{rows}") for seed in ("zz", "yy")]
    assert cmp.answer_changed(cmp.Answer(0, seeds[0], ""), cmp.Answer(0, seeds[1], ""))
    skipped = cmp.Answer(0, text_answer(f"{SKIPPED_WARNING}\n\n1 import cycle\n"), "")
    whole = cmp.Answer(0, text_answer("1 import cycle\n"), "")
    assert cmp.answer_changed(skipped, whole)


def test_the_cache_field_is_masked_in_both_receipt_forms(cmp: ModuleType) -> None:
    text = text_answer("body\n", cache="g:1 fresh")
    assert "// repo: /r   head: abc   dirty: 0 files   cache: <CACHE>" in cmp.mask_cache(text)
    masked = cmp.mask_cache(refs_answer([], cache="g:1 fresh"))
    assert masked == cmp.mask_cache(refs_answer([], cache="none"))
    assert '    "cache": <CACHE>' in masked.split("\n")


def test_a_pair_that_timed_out_in_both_builds_is_counted_and_not_compared(
    cmp: ModuleType,
) -> None:
    tally = cmp.Tally(("base", "new"))
    cmp.compare_answer(tally, "i-1", "map:a", cmp.Answer(None, "", ""), cmp.Answer(None, "", ""))
    cmp.compare_answer(tally, "i-1", "map:b", cmp.Answer(0, "x\n", ""), cmp.Answer(None, "", ""))
    assert tally.not_compared == ["i-1 map:a"]
    assert tally.timeouts == {"base": 1, "new": 2}
    report = list(cmp.report_lines(tally))
    assert "timeouts: base 1, new 2" in report
    assert "not compared, a timeout in both builds: 1" in report
    assert "  i-1 map:a" in report


def test_a_different_exit_code_or_stderr_is_a_change(cmp: ModuleType) -> None:
    base = cmp.Answer(0, "body\n", "")
    assert cmp.answer_changed(base, cmp.Answer(1, "body\n", ""))
    assert cmp.answer_changed(base, cmp.Answer(0, "body\n", "agentless-mcp: warning\n"))


def test_refs_sites_keep_the_best_tier_of_a_line_found_in_two_groups(cmp: ModuleType) -> None:
    answer = refs_answer([group("a.py", "unique", 3, 7), group("a.py", "imported", 3)])
    assert cmp.refs_sites(answer) == {("a.py", 3): "imported", ("a.py", 7): "unique"}


@pytest.mark.parametrize(
    "stdout",
    [
        "agentless-mcp: no such symbol\n",
        json.dumps({"receipt": {}, "target": "f"}),
        json.dumps({"groups": [{"path": "a.py", "tier": "unique", "sites": [{"line": "3"}]}]}),
        json.dumps({"groups": ["a.py"]}),
    ],
)
def test_an_unparsable_refs_answer_has_no_sites(cmp: ModuleType, stdout: str) -> None:
    assert cmp.refs_sites(stdout) is None
    assert cmp.answer_size("refs", stdout) is None


def test_site_moves_name_a_line_that_only_one_answer_holds_absent(cmp: ModuleType) -> None:
    base = {("a.py", 1): "unique", ("a.py", 2): "same_file"}
    new = {("a.py", 1): "imported", ("b.py", 5): "ambiguous"}
    assert cmp.site_moves(base, new) == [
        (("a.py", 1), ("unique", "imported")),
        (("a.py", 2), ("same_file", "absent")),
        (("b.py", 5), ("absent", "ambiguous")),
    ]


def test_a_refs_answer_with_fewer_lines_is_a_loss(cmp: ModuleType) -> None:
    base = cmp.answer_size("refs", refs_answer([group("a.py", "unique", 1, 2)]))
    new = cmp.answer_size("refs", refs_answer([group("a.py", "imported", 1)]))
    assert (base, new) == ((2,), (1,))
    assert cmp.shrank(base, new)


def test_the_map_size_counts_the_file_rows_only(cmp: ModuleType) -> None:
    body = "a.py  (rank 0.2199)\n  def f()  [f] @3\n\nb/c.py  (rank 0.1)\n  class C  [C] @1\n"
    assert cmp.answer_size("map", text_answer(body)) == (2,)


@pytest.mark.parametrize(
    ("body", "size"),
    [
        ("no import cycles\n", (0,)),
        ("1 import cycle\n    1. (2 files) a.py -> b.py -> a.py\n", (1,)),
        (f"{SKIPPED_WARNING}\n\n3 import cycles\n", (3,)),
        ("agentless-mcp: no repository\n", None),
    ],
)
def test_the_cycles_size_is_the_count_its_header_states(
    cmp: ModuleType, body: str, size: tuple[int, ...] | None
) -> None:
    assert cmp.answer_size("cycles", text_answer(body)) == size


def test_the_explain_size_is_the_fan_in_and_the_fan_out(cmp: ModuleType) -> None:
    body = "references (fan-out): 2\n  imported (2)\n\nreferenced by (fan-in): none\n"
    assert cmp.answer_size("explain", text_answer(body)) == (0, 2)
    assert cmp.answer_size("explain", "agentless-mcp: no symbol named f\n") is None


def test_a_lower_fan_in_is_a_loss_and_an_unknown_size_is_not(cmp: ModuleType) -> None:
    assert cmp.shrank((3, 2), (2, 2))
    assert not cmp.shrank((3, 2), (3, 4))
    assert not cmp.shrank((3,), None)
    assert not cmp.shrank(None, (0,))


@pytest.mark.parametrize(
    ("base", "new", "lost"),
    [(0, 1, True), (0, None, True), (1, 0, False), (2, 1, False), (0, 0, False)],
)
def test_an_exit_code_is_lost_when_a_success_fails_or_times_out(
    cmp: ModuleType, base: int, new: int | None, lost: bool
) -> None:
    assert cmp.exit_lost(base, new) is lost


def test_truncation_is_read_from_the_json_key_or_the_ceiling_line(cmp: ModuleType) -> None:
    assert cmp.is_truncated(refs_answer([], truncated={"token_ceiling": 8000}))
    assert cmp.is_truncated("a\n... output truncated at the 8000-token ceiling: 4 of 9 lines\n")
    assert not cmp.is_truncated(refs_answer([]))
    assert not cmp.is_truncated("a.py  (rank 0.1)\n")


def test_compare_answer_counts_tier_moves_losses_and_one_diff(cmp: ModuleType) -> None:
    tally = cmp.Tally(("base b:1", "new b:2"))
    base = cmp.Answer(0, refs_answer([group("a.py", "unique", 1, 2)]), "")
    new = cmp.Answer(0, refs_answer([group("a.py", "imported", 1)], cache="g:2 fresh"), "")
    cmp.compare_answer(tally, "i-1", "refs:f", base, new)
    assert tally.moves == {("unique", "imported"): 1, ("unique", "absent"): 1}
    assert tally.losses == ["i-1 refs:f: refs size (2,) -> (1,)"]
    assert tally.changed == {"refs": 1}
    assert len(tally.diffs) == 1
    assert '"cache"' not in tally.diffs[0]


def test_an_unparsable_refs_answer_is_counted_and_has_no_moves(cmp: ModuleType) -> None:
    tally = cmp.Tally(("base", "new"))
    base = cmp.Answer(0, refs_answer([group("a.py", "unique", 1)]), "")
    cmp.compare_answer(tally, "i-1", "refs:f", base, cmp.Answer(None, "", ""))
    assert tally.unparsable == {"base": 0, "new": 1}
    assert not tally.moves
    assert tally.losses == ["i-1 refs:f: exit 0 -> timeout"]


def test_frame_differences_name_each_instance_and_kind(cmp: ModuleType) -> None:
    listing = {"code": 0, "stdout": "{}", "stderr": ""}
    base = {"i-1": {"class": listing, "method": listing}, "i-2": {"class": listing}}
    new = {"i-1": {"class": listing, "method": {**listing, "code": 1}}, "i-3": {}}
    assert cmp.frame_differences(base, new) == [
        "i-2: only in one frame",
        "i-3: only in one frame",
        "i-1 method",
    ]


def test_two_dumps_of_one_target_list_and_one_frame_can_be_compared(
    cmp: ModuleType, tmp_path: Path
) -> None:
    base = write_dump(tmp_path / "base", TARGETS, "{}", ["i-1"])
    new = write_dump(tmp_path / "new", TARGETS, "{}", ["i-1"])
    assert cmp.refusals(base, new) == []


def test_two_dumps_of_different_target_lists_are_refused(cmp: ModuleType, tmp_path: Path) -> None:
    base = write_dump(tmp_path / "base", TARGETS, "{}", ["i-1"])
    new = write_dump(tmp_path / "new", TARGETS + "\n", "{}", ["i-1"])
    assert cmp.refusals(base, new) == ["the two dumps read different targets.json files"]


def test_different_frames_and_a_missing_answer_are_refused(cmp: ModuleType, tmp_path: Path) -> None:
    base = write_dump(tmp_path / "base", TARGETS, '{"i-1": {}}', ["i-1"])
    new = write_dump(tmp_path / "new", TARGETS, '{"i-1": {"class": {}}}', [])
    found = cmp.refusals(base, new)
    assert found[0] == "the two builds listed different find-symbol frames: i-1 class"
    assert found[1] == f"{new} has no answers for 1 targets: ['i-1']"
    assert len(found) == 2


def test_a_dump_that_a_failed_worker_left_without_its_frame_is_refused(
    cmp: ModuleType, tmp_path: Path
) -> None:
    base = write_dump(tmp_path / "base", TARGETS, "{}", ["i-1"])
    new = write_dump(tmp_path / "new", TARGETS, None, [])
    assert cmp.refusals(base, new) == [f"{new} has no frame.json"]
