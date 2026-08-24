"""The error taxonomy, and the exit code each branch of it carries.

Two rules are pinned here because nothing else can pin them. The root is a
base and nothing else, so ``except AgentlessError`` means "any error this
package raises" and never doubles as "a condition nobody classified". And
"the input you named could not be read" is one condition with one exit code,
whichever front door reached it -- it used to be 1 through a service and 2
through the CLI's own reader, for the same mistake.
"""

import pytest

from agentless_mcp.adapters.cli.formatting import EXIT_DOMAIN, EXIT_USAGE, exit_code_for
from agentless_mcp.application import lint_service, validate_service
from agentless_mcp.util.errors import (
    AgentlessError,
    CacheLocked,
    InputUnreadable,
    LanguageUnavailable,
    OperationFailed,
    RepoResolutionError,
    SecurityRefusal,
    WalkBoundExceeded,
)

LEAVES = (
    CacheLocked,
    InputUnreadable,
    LanguageUnavailable,
    OperationFailed,
    RepoResolutionError,
    SecurityRefusal,
    WalkBoundExceeded,
)


class TestTheRootIsOnlyARoot:
    @pytest.mark.parametrize("leaf", LEAVES, ids=[leaf.__name__ for leaf in LEAVES])
    def test_every_leaf_is_caught_by_the_root(self, leaf):
        message = "message"
        with pytest.raises(AgentlessError):
            raise leaf(message)

    def test_the_catch_all_leaf_is_not_the_root(self):
        assert OperationFailed is not AgentlessError
        assert issubclass(OperationFailed, AgentlessError)


class TestTheExitCodeIsKeyedOnTheError:
    """Not on the message, and not on which subcommand raised it."""

    @pytest.mark.parametrize(
        ("error", "expected"),
        [
            (SecurityRefusal("refused"), EXIT_USAGE),
            (RepoResolutionError("no root"), EXIT_USAGE),
            (InputUnreadable("gone"), EXIT_USAGE),
            (OperationFailed("did not apply"), EXIT_DOMAIN),
            (LanguageUnavailable("no grammar"), EXIT_DOMAIN),
            (WalkBoundExceeded("too many files"), EXIT_DOMAIN),
            (CacheLocked("held"), EXIT_DOMAIN),
        ],
        ids=lambda value: type(value).__name__ if isinstance(value, Exception) else str(value),
    )
    def test_each_error_carries_its_code(self, error, expected):
        assert exit_code_for(error) == expected


class TestAnInputThatCannotBeReadIsOneCondition:
    """The four load paths that name a caller's file agree on the type.

    ``vote`` and ``patch`` read theirs in the adapter and already exited 2;
    these two read theirs through a service and exited 1. The type is what
    the adapter keys on now, so the four cannot drift apart again.
    """

    def test_lint_refuses_a_candidates_path_that_is_neither(self, tmp_path):
        with pytest.raises(InputUnreadable, match="neither a patch file nor a directory"):
            lint_service.load_candidates(tmp_path / "nope")

    def test_lint_refuses_a_diff_that_does_not_exist(self, tmp_path):
        with pytest.raises(InputUnreadable, match="cannot read diff"):
            lint_service.load_diff(tmp_path / "nope.patch")

    def test_validate_refuses_a_candidates_path_that_is_not_a_directory(self, tmp_path):
        target = tmp_path / "one.patch"
        target.write_text("", encoding="utf-8")
        with pytest.raises(InputUnreadable, match="must name a directory"):
            validate_service.load_candidates(target)

    def test_validate_refuses_a_candidate_that_is_not_utf8(self, tmp_path):
        directory = tmp_path / "candidates"
        directory.mkdir()
        (directory / "one.patch").write_bytes(b"\xff\xfe not text")
        with pytest.raises(InputUnreadable, match="is not UTF-8 text"):
            validate_service.load_candidates(directory)

    def test_an_empty_candidate_set_is_a_domain_failure_not_a_usage_one(self, tmp_path):
        """The directory the caller named exists; it just holds nothing."""
        directory = tmp_path / "candidates"
        directory.mkdir()
        with pytest.raises(OperationFailed, match="no candidate files"):
            validate_service.load_candidates(directory)
