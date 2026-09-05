"""History: the commits that touched one symbol's lines, bodies included, fitted to a budget."""

import re
import subprocess

import pytest

from agentless_mcp.application.history_service import (
    DEFAULT_HISTORY_LIMIT,
    HistoryService,
    render_history,
)
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.core import gitinfo
from agentless_mcp.core.projectconfig import MIN_BUDGET
from agentless_mcp.util.errors import OperationFailed

SOURCE = """\
def quote(sku):
    return 1


class PriceBook:
    def cost_of(self, sku):
        return quote(sku)
"""

EDITED_QUOTE = SOURCE.replace("    return 1\n", "    rate = 2\n    return rate\n")
EDITED_COST = EDITED_QUOTE.replace("        return quote(sku)\n", "        return quote(sku) * 2\n")

WHY = "Return the rate\n\nBecause the loader reads it.\nAnd the sink owns the line grammar."
OUTSIDE = "Tidy cost_of\n\nOutside the quote span."
RECORD = "a" * 40 + "\x00" + "2026-08-23T16:43:57-04:00" + "\x00" + "s" + "\x00" + "b" + "\x00"


def stage_all(root):
    """Stage everything, so commit_all records the working tree as it stands."""
    subprocess.run(
        ["git", "-C", str(root), "add", "-A"], check=True, capture_output=True, timeout=30
    )


def commit_file(root, commit_all, text, message):
    """Write core.py, stage it, and commit it with the pinned identity."""
    (root / "core.py").write_text(text, encoding="utf-8")
    stage_all(root)
    commit_all(root, message)


@pytest.fixture
def repo(make_git_repo, commit_all):
    """Three commits: the fixture, an edit inside quote, an edit outside it."""
    root = make_git_repo({"core.py": SOURCE})
    commit_file(root, commit_all, EDITED_QUOTE, WHY)
    commit_file(root, commit_all, EDITED_COST, OUTSIDE)
    return root


@pytest.fixture
def service(extractor, counter):
    return HistoryService(extractor, counter)


def context(root):
    return resolve_repo(root, None)


def injected(extractor, counter, outcomes):
    """A service whose git is a table of subcommand -> outcome, recording each call."""
    calls = []

    def runner(cwd, arguments, *, timeout, max_output_bytes):
        calls.append((cwd, list(arguments), timeout, max_output_bytes))
        return outcomes[arguments[0]]

    return HistoryService(extractor, counter, runner=runner), calls


class TestHistory:
    def test_lists_the_commits_that_touched_the_span_newest_first(self, service, repo):
        result = service.history(context(repo), "py:core.py::quote")

        assert result.target == "py:core.py::quote"
        assert (result.path, result.start_line, result.end_line) == ("core.py", 1, 3)
        assert [entry.subject for entry in result.entries] == ["Return the rate", "fixture"]
        assert result.entries[0].body_lines == (
            "Because the loader reads it.",
            "And the sink owns the line grammar.",
        )
        assert result.entries[1].body_lines == ()
        assert re.match(r"\d{4}-\d{2}-\d{2}T", result.entries[0].authored)
        assert result.more_commits is False
        assert result.output_capped is False
        assert result.dirty is False

    def test_a_limit_below_the_count_marks_the_rest(self, service, repo):
        result = service.history(context(repo), "py:core.py::quote", limit=1)

        assert [entry.subject for entry in result.entries] == ["Return the rate"]
        assert result.more_commits is True
        assert "... 1 newest commits shown; older commits touch this span" in render_history(result)

    def test_a_small_budget_cuts_the_long_body_and_marks_it(
        self, service, make_git_repo, commit_all
    ):
        root = make_git_repo({"core.py": SOURCE})
        body = "\n".join(f"line {number} of a long explanation" for number in range(40))
        commit_file(root, commit_all, EDITED_QUOTE, f"Long why\n\n{body}")

        result = service.history(context(root), "py:core.py::quote", budget=MIN_BUDGET)

        cut = result.entries[0]
        assert 1 <= cut.body_shown < cut.body_total == 40
        text = render_history(result)
        assert f"... {cut.body_shown} of 40 body lines shown for {cut.short_sha}" in text
        assert f"git show -s --format=%B {cut.short_sha}" in text
        assert cut.as_dict()["body_truncated"] == {"lines_shown": cut.body_shown, "lines": 40}
        assert "body_truncated" not in result.entries[1].as_dict()

    def test_an_uncommitted_edit_to_the_file_is_noted(self, service, repo):
        (repo / "core.py").write_text(EDITED_COST + "\n# trailing\n", encoding="utf-8")

        result = service.history(context(repo), "py:core.py::quote")

        assert result.dirty is True
        assert "note: core.py has uncommitted changes" in render_history(result)

    def test_the_render_indents_rows_and_escapes_body_lines(
        self, service, make_git_repo, commit_all
    ):
        root = make_git_repo({"core.py": SOURCE})
        commit_file(root, commit_all, EDITED_QUOTE, "Escape\n\nfirst \x1b[0m second\n\nlast")

        text = render_history(service.history(context(root), "py:core.py::quote"))

        lines = text.splitlines()
        assert lines[0] == "py:core.py::quote  core.py:1-3  (2 commits, newest first)"
        assert lines[1].startswith("  ")
        assert lines[1].endswith("  Escape")
        assert lines[2].startswith("    first ")
        assert lines[3] == ""
        assert lines[4] == "    last"
        assert "\x1b" not in text
        assert not any(line.startswith("...") for line in lines)

    def test_a_filename_holding_a_colon_is_traced(self, service, make_git_repo):
        root = make_git_repo({"odd:name.py": SOURCE})

        result = service.history(context(root), "py:odd:name.py::quote")

        assert result.path == "odd:name.py"
        assert [entry.subject for entry in result.entries] == ["fixture"]


class TestRefusals:
    def test_a_bare_name_is_refused_with_the_fix(self, service, repo):
        with pytest.raises(OperationFailed, match="Pass a stable id"):
            service.history(context(repo), "quote")

    def test_an_unknown_symbol_is_refused(self, service, repo):
        with pytest.raises(OperationFailed, match="no longer defines"):
            service.history(context(repo), "py:core.py::missing")

    def test_an_untracked_file_is_refused_as_not_in_head(self, service, repo):
        (repo / "new.py").write_text("def fresh():\n    return 0\n", encoding="utf-8")
        with pytest.raises(OperationFailed, match="not in HEAD"):
            service.history(context(repo), "py:new.py::fresh")

    def test_a_directory_without_git_is_refused_before_any_spawn(
        self, extractor, counter, tmp_path
    ):
        (tmp_path / "core.py").write_text(SOURCE, encoding="utf-8")
        service, calls = injected(extractor, counter, {})

        with pytest.raises(OperationFailed, match="history needs git"):
            service.history(context(tmp_path), "py:core.py::quote")
        assert calls == []

    def test_a_timed_out_git_is_reported_as_git_failing(self, extractor, counter, repo):
        service, _ = injected(
            extractor,
            counter,
            {"log": gitinfo.GitOutcome(None, "git log timed out after 30.0s")},
        )
        with pytest.raises(OperationFailed, match="git could not answer: git log timed out"):
            service.history(context(repo), "py:core.py::quote")

    def test_no_records_is_a_refusal_not_an_empty_answer(self, extractor, counter, repo):
        service, _ = injected(extractor, counter, {"log": gitinfo.GitOutcome("", "", returncode=0)})
        with pytest.raises(
            OperationFailed, match=re.escape("no commit touches lines 1-3 of core.py")
        ):
            service.history(context(repo), "py:core.py::quote")

    def test_bounds_are_checked_before_git_is_asked(self, extractor, counter, repo):
        service, calls = injected(extractor, counter, {})
        with pytest.raises(OperationFailed, match="limit"):
            service.history(context(repo), "py:core.py::quote", limit=0)
        with pytest.raises(OperationFailed, match="budget"):
            service.history(context(repo), "py:core.py::quote", budget=MIN_BUDGET - 1)
        assert calls == []


class TestGitContract:
    def test_the_argv_carries_the_span_and_the_overflow_probe(self, extractor, counter, repo):
        service, calls = injected(
            extractor,
            counter,
            {
                "log": gitinfo.GitOutcome(RECORD, "", returncode=0),
                "diff": gitinfo.GitOutcome(None, "git diff exited 1: no detail", returncode=1),
            },
        )

        result = service.history(context(repo), "py:core.py::quote", limit=DEFAULT_HISTORY_LIMIT)

        assert calls[0][1] == [
            "log",
            "-L1,3:core.py",
            "--no-patch",
            "-z",
            "--format=%H%x00%aI%x00%s%x00%b",
            "--max-count=11",
            "--",
        ]
        assert calls[0][2] == 30.0
        assert calls[1][1] == ["diff", "--quiet", "HEAD", "--", "./core.py"]
        assert result.dirty is True
        assert result.entries[0].short_sha == "a" * 8

    def test_a_capped_output_keeps_whole_records_and_says_so(self, extractor, counter, repo):
        service, _ = injected(
            extractor,
            counter,
            {
                "log": gitinfo.GitOutcome(
                    RECORD + "b" * 40 + "\x00date", "", truncated=True, returncode=-9
                ),
                "diff": gitinfo.GitOutcome(None, "", returncode=0),
            },
        )

        result = service.history(context(repo), "py:core.py::quote")

        assert [entry.sha for entry in result.entries] == ["a" * 40]
        assert result.output_capped is True
        assert "git output was cut at 2000000 bytes; the 1 complete commits" in render_history(
            result
        )
