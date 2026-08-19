"""Validation: the baseline decides whether any candidate verdict means anything.

Four properties are pinned here, and each of them is a way the phase can go
quietly wrong rather than loudly:

* a red baseline short-circuits the whole run instead of producing four
  verdicts that look like measurements and are not;
* a reproduction test that already passes is reported and dropped from the
  ladder, not counted as a pass every candidate gets for free;
* a candidate whose edits do not land carries the per-block reason, so an
  agent can fix the patch rather than the code;
* ``--jobs 2`` and ``--jobs 1`` produce the same document, because completion
  order is not evidence.

The test commands are tiny scripts the tests write into the fixture repository
and invoke through ``sys.executable``. Nothing here depends on PATH, on a
listening port, on the clock or on a test that ran before it.
"""

import json

import pytest

from agentless_mcp.application.patch_service import PatchService
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.application.validate_service import (
    ApplyStatus,
    BaselineStatus,
    ReproVerdict,
    ValidateRequest,
    ValidateService,
    Verdict,
    load_candidates,
    load_verdicts,
)
from agentless_mcp.util.errors import AtlasError

PLUS = """\
### app.py
<<<<<<< SEARCH
    return a - b
=======
    return a + b
>>>>>>> REPLACE
"""

# The same structural change written differently: extra spacing and a trailing
# comment, both of which the equivalence key normalises away.
PLUS_COMMENTED = """\
### app.py
<<<<<<< SEARCH
    return a - b
=======
    return a  +  b  # restore the sign
>>>>>>> REPLACE
"""

# Also correct, also passes both suites, but a different token stream -- so a
# different equivalence class, which is the point.
SWAPPED = """\
### app.py
<<<<<<< SEARCH
    return a - b
=======
    return b + a
>>>>>>> REPLACE
"""

# Passes nothing: add(1, 0) becomes 0.
TIMES = """\
### app.py
<<<<<<< SEARCH
    return a - b
=======
    return a * b
>>>>>>> REPLACE
"""

MISSING = """\
### app.py
<<<<<<< SEARCH
    return a // b
=======
    return a + b
>>>>>>> REPLACE
"""

FOUR_CANDIDATES = {
    "01-plus.txt": PLUS,
    "02-plus-commented.txt": PLUS_COMMENTED,
    "03-swapped.txt": SWAPPED,
    "04-times.txt": TIMES,
}

ALWAYS_RED = "raise SystemExit(1)\n"


@pytest.fixture
def service(extractor):
    return ValidateService(PatchService(extractor))


@pytest.fixture
def validate(service, python_cmd):
    """Run one validation against a repository, with the fixture commands."""

    def call(repo, candidates, *, repro=None, timeout=60, test_cmd=None, **request):
        return service.validate(
            resolve_repo(repo, None),
            ValidateRequest(
                candidates=candidates,
                test_cmd=test_cmd or python_cmd("check_regression.py"),
                repro_cmd=None if repro is None else python_cmd(repro),
                timeout=timeout,
                **request,
            ),
        )

    return call


# A test command that passes and fails alternately, keyed on a counter file
# whose path it is given rather than on the clock or on a random number: a
# flaky baseline has to be reproducible for a test to say anything about it.
# The counter lives outside the repository on purpose -- every baseline run
# gets a fresh worktree of HEAD, so state kept inside one would reset between
# runs and the command would never actually disagree with itself.
ALTERNATING = """\
import sys
from pathlib import Path

counter = Path(sys.argv[1])
runs = int(counter.read_text()) if counter.exists() else 0
counter.write_text(str(runs + 1))

print(f"run {runs}")
raise SystemExit(runs % 2)
"""


@pytest.fixture
def alternating_cmd(seeded_bug_repo, python_cmd, tmp_path):
    """A repository whose test command passes, fails, passes, fails."""

    def build():
        counter = tmp_path / "baseline-runs.txt"
        repo = seeded_bug_repo(overrides={"flip.py": ALTERNATING})
        return repo, python_cmd("flip.py", str(counter)), counter

    return build


def by_id(report):
    return {verdict.id: verdict for verdict in report.verdicts}


class TestLoadCandidates:
    def test_ids_are_stems_and_order_is_sorted(self, candidates_dir):
        directory = candidates_dir(FOUR_CANDIDATES)
        candidates = load_candidates(directory)

        assert [candidate.id for candidate in candidates] == [
            "01-plus",
            "02-plus-commented",
            "03-swapped",
            "04-times",
        ]
        assert [candidate.index for candidate in candidates] == [0, 1, 2, 3]

    def test_an_empty_directory_is_refused(self, candidates_dir):
        with pytest.raises(AtlasError, match="no candidate files"):
            load_candidates(candidates_dir({}))

    def test_a_missing_directory_is_refused(self, tmp_path):
        with pytest.raises(AtlasError, match="not one"):
            load_candidates(tmp_path / "nowhere")

    def test_two_candidates_sharing_a_stem_are_refused(self, candidates_dir):
        directory = candidates_dir({"fix.txt": PLUS, "fix.json": '{"edits": []}'})
        with pytest.raises(AtlasError, match="share the id"):
            load_candidates(directory)


class TestBaseline:
    def test_a_red_baseline_short_circuits_the_whole_run(
        self, seeded_bug_repo, candidates_dir, validate
    ):
        repo = seeded_bug_repo(overrides={"check_regression.py": ALWAYS_RED})
        report = validate(repo, candidates_dir(FOUR_CANDIDATES))

        assert report.header.baseline is BaselineStatus.UNVERIFIED
        assert "exited 1" in report.header.baseline_detail
        assert all(verdict.regression is Verdict.NOT_EVALUATED for verdict in report.verdicts)
        assert all(verdict.equivalence_key is None for verdict in report.verdicts)
        assert not report.any_passed
        assert any("UNVERIFIED" in warning for warning in report.warnings())

    def test_a_baseline_that_hangs_is_unverified_too(
        self, seeded_bug_repo, candidates_dir, validate
    ):
        repo = seeded_bug_repo(overrides={"check_regression.py": "import time\ntime.sleep(600)\n"})
        report = validate(repo, candidates_dir({"01-plus.txt": PLUS}), timeout=1)

        assert report.header.baseline is BaselineStatus.UNVERIFIED
        assert "timed out" in report.header.baseline_detail
        assert not report.any_passed

    def test_a_repro_that_passes_on_the_baseline_is_reported_and_dropped(
        self, seeded_bug_repo, candidates_dir, validate
    ):
        repo = seeded_bug_repo()
        # check_regression.py passes on unpatched HEAD, so it cannot possibly
        # be a reproduction of the bug.
        report = validate(repo, candidates_dir(FOUR_CANDIDATES), repro="check_regression.py")

        assert report.header.repro_verdict is ReproVerdict.DOES_NOT_REPRODUCE
        assert not report.header.repro_valid
        assert any("does_not_reproduce" in warning for warning in report.warnings())
        # Excluded from the ladder means it is not run per candidate at all.
        assert all(verdict.reproduction is None for verdict in report.verdicts)

    def test_a_repro_that_fails_on_the_baseline_counts(
        self, seeded_bug_repo, candidates_dir, validate
    ):
        report = validate(
            seeded_bug_repo(), candidates_dir({"01-plus.txt": PLUS}), repro="check_repro.py"
        )

        assert report.header.repro_verdict is ReproVerdict.REPRODUCES
        assert report.header.repro_valid

    def test_a_repro_that_cannot_be_started_is_unrunnable(
        self, seeded_bug_repo, candidates_dir, service, python_cmd
    ):
        report = service.validate(
            resolve_repo(seeded_bug_repo(), None),
            ValidateRequest(
                candidates=candidates_dir({"01-plus.txt": PLUS}),
                test_cmd=python_cmd("check_regression.py"),
                repro_cmd="./no-such-runner-4c1a",
                timeout=60,
            ),
        )

        assert report.header.repro_verdict is ReproVerdict.UNRUNNABLE
        assert not report.header.repro_valid
        assert any("unrunnable" in warning for warning in report.warnings())

    def test_no_repro_command_is_its_own_verdict(self, seeded_bug_repo, candidates_dir, validate):
        report = validate(seeded_bug_repo(), candidates_dir({"01-plus.txt": PLUS}))

        assert report.header.repro_verdict is ReproVerdict.NOT_GIVEN
        assert report.warnings() == ()


class TestRepeatBaseline:
    """``--repeat-baseline N``: three outcomes, and the mixed one is the point."""

    def test_a_deterministic_green_baseline_repeated_still_proceeds(
        self, seeded_bug_repo, candidates_dir, validate
    ):
        report = validate(
            seeded_bug_repo(), candidates_dir({"01-plus.txt": PLUS}), repeat_baseline=3
        )

        assert report.header.baseline is BaselineStatus.OK
        assert report.header.repeat_baseline == 3
        assert report.header.baseline_failures == 0
        assert not report.header.flaky_baseline
        assert "all 3 baseline runs" in report.header.baseline_detail
        assert report.any_passed

    def test_a_deterministic_red_baseline_repeated_is_broken_not_flaky(
        self, seeded_bug_repo, candidates_dir, validate
    ):
        repo = seeded_bug_repo(overrides={"check_regression.py": ALWAYS_RED})
        report = validate(repo, candidates_dir(FOUR_CANDIDATES), repeat_baseline=3)

        assert report.header.baseline is BaselineStatus.UNVERIFIED
        assert report.header.baseline_failures == 3
        assert not report.header.flaky_baseline
        assert "exited 1" in report.header.baseline_detail
        assert all(verdict.regression is Verdict.NOT_EVALUATED for verdict in report.verdicts)

    def test_a_disagreeing_baseline_is_unverified_and_names_the_count(
        self, alternating_cmd, candidates_dir, validate
    ):
        repo, command, _ = alternating_cmd()
        report = validate(
            repo, candidates_dir(FOUR_CANDIDATES), test_cmd=command, repeat_baseline=4
        )

        assert report.header.baseline is BaselineStatus.UNVERIFIED
        assert report.header.flaky_baseline
        assert report.header.baseline_failures == 2
        assert "FLAKY BASELINE" in report.header.baseline_detail
        assert "2 failed and 2 passed" in report.header.baseline_detail
        assert all(verdict.regression is Verdict.NOT_EVALUATED for verdict in report.verdicts)
        assert not report.any_passed

    def test_the_flaky_warning_says_what_to_do_about_it(
        self, alternating_cmd, candidates_dir, validate
    ):
        repo, command, _ = alternating_cmd()
        report = validate(
            repo, candidates_dir({"01-plus.txt": PLUS}), test_cmd=command, repeat_baseline=2
        )

        (warning,) = report.warnings()
        assert "UNVERIFIED" in warning
        assert "answers differently" in warning

    def test_the_run_record_carries_the_repeat_count(
        self, seeded_bug_repo, candidates_dir, validate
    ):
        report = validate(
            seeded_bug_repo(), candidates_dir({"01-plus.txt": PLUS}), repeat_baseline=2
        )
        record = json.loads(report.jsonl().splitlines()[0])

        assert record["repeat_baseline"] == 2
        assert record["baseline_failures"] == 0
        assert record["flaky_baseline"] is False

    def test_the_default_runs_the_baseline_exactly_once(
        self, alternating_cmd, candidates_dir, validate
    ):
        repo, command, counter = alternating_cmd()
        # A candidate that cannot apply costs no test run, so the counter ends
        # up holding the number of BASELINE runs and nothing else.
        report = validate(repo, candidates_dir({"01-noop.txt": "not a patch"}), test_cmd=command)

        # The default is one run, and the alternating command passes on its
        # first: the flag existing must not have changed what happens without
        # it. The counter is what proves the command ran once, not twice.
        assert report.header.repeat_baseline == 1
        assert report.header.baseline is BaselineStatus.OK
        assert not report.header.flaky_baseline
        assert counter.read_text() == "1"

    def test_the_baseline_runs_exactly_as_many_times_as_asked(
        self, alternating_cmd, candidates_dir, validate
    ):
        repo, command, counter = alternating_cmd()
        validate(
            repo,
            candidates_dir({"01-noop.txt": "not a patch"}),
            test_cmd=command,
            repeat_baseline=3,
        )

        assert counter.read_text() == "3"


class TestCandidates:
    @pytest.fixture
    def report(self, seeded_bug_repo, candidates_dir, validate):
        return validate(seeded_bug_repo(), candidates_dir(FOUR_CANDIDATES), repro="check_repro.py")

    def test_the_good_fixes_pass_both_suites(self, report):
        verdicts = by_id(report)
        for name in ("01-plus", "02-plus-commented", "03-swapped"):
            assert verdicts[name].apply_status is ApplyStatus.OK
            assert verdicts[name].regression is Verdict.PASSED
            assert verdicts[name].reproduction is Verdict.PASSED

    def test_the_bad_fix_fails_the_regression_suite(self, report):
        bad = by_id(report)["04-times"]

        assert bad.apply_status is ApplyStatus.OK
        assert bad.regression is Verdict.FAILED
        assert bad.reproduction is Verdict.FAILED
        assert "add(1, 0) == 0" in bad.regression_run.stderr_tail

    def test_the_two_spellings_of_one_fix_share_an_equivalence_key(self, report):
        verdicts = by_id(report)

        assert verdicts["01-plus"].equivalence_key == verdicts["02-plus-commented"].equivalence_key
        assert verdicts["01-plus"].equivalence_key != verdicts["03-swapped"].equivalence_key

    def test_the_run_reports_that_something_passed(self, report):
        assert report.any_passed

    def test_the_checkout_is_untouched_by_the_whole_run(
        self, seeded_bug_repo, candidates_dir, validate
    ):
        repo = seeded_bug_repo()
        before = (repo / "app.py").read_text(encoding="utf-8")
        validate(repo, candidates_dir(FOUR_CANDIDATES))

        assert (repo / "app.py").read_text(encoding="utf-8") == before

    def test_a_candidate_that_does_not_apply_carries_the_block_reason(
        self, seeded_bug_repo, candidates_dir, validate
    ):
        report = validate(seeded_bug_repo(), candidates_dir({"05-missing.txt": MISSING}))
        verdict = by_id(report)["05-missing"]

        assert verdict.apply_status is ApplyStatus.FAILED
        assert verdict.regression is Verdict.NOT_EVALUATED
        assert verdict.equivalence_key is None
        assert len(verdict.apply_reasons) == 1
        assert "not_found" in verdict.apply_reasons[0]
        assert "app.py" in verdict.apply_reasons[0]
        assert not report.any_passed

    def test_a_malformed_candidate_fails_that_candidate_only(
        self, seeded_bug_repo, candidates_dir, validate
    ):
        report = validate(
            seeded_bug_repo(),
            candidates_dir({"01-plus.txt": PLUS, "02-truncated.txt": PLUS.split("=======", 1)[0]}),
        )
        verdicts = by_id(report)

        assert verdicts["02-truncated"].apply_status is ApplyStatus.FAILED
        assert "not terminated" in verdicts["02-truncated"].apply_reasons[0]
        assert verdicts["01-plus"].regression is Verdict.PASSED

    def test_an_edits_json_candidate_is_read_too(self, seeded_bug_repo, candidates_dir, validate):
        document = json.dumps(
            {
                "edits": [
                    {
                        "index": 0,
                        "path": "app.py",
                        "search": "    return a - b",
                        "replace": "    return a + b",
                    }
                ]
            }
        )
        report = validate(seeded_bug_repo(), candidates_dir({"01-json.json": document}))

        assert by_id(report)["01-json"].regression is Verdict.PASSED


class TestParallelism:
    def test_two_jobs_produce_the_same_verdicts_as_one(
        self, seeded_bug_repo, candidates_dir, validate
    ):
        """Durations differ between runs; nothing else may."""
        repo = seeded_bug_repo()
        directory = candidates_dir(FOUR_CANDIDATES)

        serial = validate(repo, directory, repro="check_repro.py", jobs=1)
        parallel = validate(repo, directory, repro="check_repro.py", jobs=2)

        assert _comparable(serial) == _comparable(parallel)
        assert [verdict.id for verdict in parallel.verdicts] == [
            "01-plus",
            "02-plus-commented",
            "03-swapped",
            "04-times",
        ]


def _comparable(report):
    """Strip the fields that legitimately vary between two identical runs."""
    records = [json.loads(line) for line in report.jsonl().splitlines()]
    for record in records:
        record.pop("duration", None)
        record.pop("tails", None)
        record.pop("baseline_run", None)
        record.pop("repro_baseline_run", None)
        record.pop("jobs", None)
    return records


class TestVerdictDocument:
    @pytest.fixture
    def document(self, seeded_bug_repo, candidates_dir, validate):
        return validate(
            seeded_bug_repo(), candidates_dir(FOUR_CANDIDATES), repro="check_repro.py"
        ).jsonl()

    def test_the_first_record_is_the_run_header(self, document):
        header = json.loads(document.splitlines()[0])

        assert header["record"] == "run"
        assert header["repro_valid"] is True
        assert header["baseline"] == "ok"
        assert header["candidates"] == 4
        assert header["receipt"]["head"]
        assert "check_regression.py" in header["test_cmd"]

    def test_every_candidate_gets_one_record(self, document):
        records = [json.loads(line) for line in document.splitlines()[1:]]

        assert [record["id"] for record in records] == [
            "01-plus",
            "02-plus-commented",
            "03-swapped",
            "04-times",
        ]
        assert all(record["record"] == "candidate" for record in records)

    def test_tails_ride_along_only_on_a_failure(self, document):
        records = {json.loads(line)["id"]: json.loads(line) for line in document.splitlines()[1:]}

        assert "tails" not in records["01-plus"]
        assert "the suite is red" not in document
        assert "add(1, 0) == 0" in records["04-times"]["tails"]["regression"]["stderr_tail"]

    def test_it_round_trips_through_the_reader(self, document):
        loaded = load_verdicts(document)

        assert loaded.repro_valid is True
        assert loaded.baseline is BaselineStatus.OK
        assert [candidate.id for candidate in loaded.candidates] == [
            "01-plus",
            "02-plus-commented",
            "03-swapped",
            "04-times",
        ]
        assert [candidate.regression_passed for candidate in loaded.candidates] == [
            True,
            True,
            True,
            False,
        ]


class TestLoadVerdicts:
    def test_an_empty_document_is_refused(self):
        with pytest.raises(AtlasError, match="empty"):
            load_verdicts("\n \n")

    def test_a_non_json_line_is_refused_with_its_number(self):
        with pytest.raises(AtlasError, match=r"line 1 .* not valid JSON"):
            load_verdicts("not json at all\n")

    def test_a_document_that_does_not_start_with_a_run_record_is_refused(self):
        with pytest.raises(AtlasError, match=r"must be a 'run' record"):
            load_verdicts(json.dumps({"record": "candidate", "id": "a"}) + "\n")

    def test_a_header_missing_repro_valid_is_refused_rather_than_defaulted(self):
        header = {"record": "run", "baseline": "ok", "test_cmd": "x", "repro_cmd": None}
        with pytest.raises(AtlasError, match="repro_valid"):
            load_verdicts(json.dumps(header) + "\n")

    def test_a_candidate_missing_its_apply_object_is_refused(self):
        header = {
            "record": "run",
            "baseline": "ok",
            "repro_valid": False,
            "test_cmd": "x",
            "repro_cmd": None,
        }
        candidate = {"record": "candidate", "id": "a", "index": 0, "regression": "passed"}
        text = json.dumps(header) + "\n" + json.dumps(candidate) + "\n"

        with pytest.raises(AtlasError, match=r"no 'apply' object"):
            load_verdicts(text)
