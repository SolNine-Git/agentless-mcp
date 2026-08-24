"""What a view needs to answer, and what it only used to insist on.

Three of the four views here need no grammar. The audit found the grammar
guard standing in front of all of them, so the ``read`` tool's line primitive
could not answer for a README, and an environment failure that ``skeleton``
degrades into a reported error came out of the other two as an exception.
"""

import pytest

from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.util.errors import LanguageUnavailable, SecurityRefusal

READ_ME = "# Ledger\n\nHow to post an entry.\nRun the reconciler nightly.\n"
LEDGER = "class Ledger:\n    def post(self, item):\n        return item\n"


@pytest.fixture
def repo(tmp_path):
    """A repository holding one parsed file and one the grammars do not claim."""
    (tmp_path / "README.md").write_text(READ_ME, encoding="utf-8")
    (tmp_path / "ledger.py").write_text(LEDGER, encoding="utf-8")
    return resolve_repo(tmp_path, None)


class TestFilesWithNoGrammar:
    """A line view asks for text, not for a parse."""

    def test_a_slice_of_a_readme_returns_its_lines(self, repo, extractor):
        view = ViewService(extractor).read_slice(repo, "README.md", intervals=[(1, 3)])

        assert view.error == ""
        assert "How to post an entry." in view.text
        assert view.language == ""

    def test_a_whole_readme_reads_without_intervals(self, repo, extractor):
        view = ViewService(extractor).read_slice(repo, "README.md")

        assert view.error == ""
        assert "Run the reconciler nightly." in view.text

    def test_a_line_location_resolves_in_a_readme(self, repo, extractor):
        view = ViewService(extractor).resolve_locations(repo, "README.md", ["line:3"])

        assert view.resolution.spans == ((3, 3),)
        assert "How to post an entry." in view.text

    def test_a_symbol_location_in_a_readme_is_unrecognized_not_fatal(self, repo, extractor):
        """No grammar means no symbols, and no symbols is an answer.

        The loc resolver already reports a name it cannot find; a file with
        nothing to find in it is the same answer for a different reason.
        """
        view = ViewService(extractor).resolve_locations(repo, "README.md", ["class:Ledger"])

        assert [entry.loc for entry in view.resolution.unrecognized] == ["class:Ledger"]

    def test_a_skeleton_still_refuses_it(self, repo, extractor):
        """The one view that is a parse keeps the hard refusal."""
        views = ViewService(extractor).skeleton(repo, ["README.md"])

        assert views[0].error == "README.md: no grammar for this file type"
        assert views[0].text == ""


class TestAnUnwarmedGrammar:
    """One environment failure, one channel: a view, not an exception."""

    @pytest.fixture
    def unwarmed(self, extractor, monkeypatch):
        """An extractor whose grammar refuses to load."""

        def refuse(*_args, **_kwargs):
            message = "language 'python' is not warmed in this process"
            raise LanguageUnavailable(message)

        monkeypatch.setattr(extractor, "extract_from_source", refuse)
        return extractor

    def test_a_slice_keeps_its_lines_without_the_scope_headers(self, repo, unwarmed):
        view = ViewService(unwarmed).read_slice(repo, "ledger.py", intervals=[(3, 3)])

        assert view.error == ""
        assert "return item" in view.text

    def test_a_location_resolution_survives_it(self, repo, unwarmed):
        view = ViewService(unwarmed).resolve_locations(repo, "ledger.py", ["line:3"])

        assert view.resolution.spans == ((3, 3),)


class TestAPathThatLeavesTheRepository:
    """The caller's own argument, so the refusal is the answer."""

    def test_a_slice_raises(self, repo, extractor):
        with pytest.raises(SecurityRefusal):
            ViewService(extractor).read_slice(repo, "../outside.py")

    def test_a_location_resolution_raises(self, repo, extractor):
        with pytest.raises(SecurityRefusal):
            ViewService(extractor).resolve_locations(repo, "../outside.py", ["line:1"])
