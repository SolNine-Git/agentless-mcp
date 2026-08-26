"""Fan-in rows keep every name match and now say how much each one is worth.

The deliberate over-reporting is the property under test as much as the labels
are: a file that shadows the target's name still appears, because a missed
caller is the expensive error -- it just appears labelled
``name-only-ambiguous``, which is what tells a reader the reference binds
somewhere else.
"""

import pytest

from agentless_mcp.application import render
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.util.tokens import Chars4Counter

CORE = """\
def helper(value):
    return value


def only_once(value):
    return value


def caller(value):
    return helper(value)
"""

USER = """\
from core import helper


def use(value):
    return helper(value)
"""

SHADOW = """\
from core import helper


def helper(value):
    return value * 2


def use_shadow(value):
    return helper(value)
"""

STRANGER = """\
def stray(value):
    return only_once(value)
"""

# A method reached through a receiver, defined once in the repository and in
# the calling file itself. A bare `refresh` cannot reach a class member, so
# the module-scope tier refuses it and `unique` was all that was left (#42).
SERVICE = """\
class Handlers:
    def refresh(self, value):
        return value


def build(handlers, value):
    return handlers.refresh(value)
"""

# The same shape with the name defined twice, where co-location narrows
# nothing without the receiver's type.
TWICE_HERE = """\
class Local:
    def sync(self, value):
        return value


def drive(worker, value):
    return worker.sync(value)
"""

TWICE_ELSEWHERE = """\
class Remote:
    def sync(self, value):
        return value * 2
"""

FILES = {
    "core.py": CORE,
    "user.py": USER,
    "shadow.py": SHADOW,
    "stranger.py": STRANGER,
    "service.py": SERVICE,
    "twice_here.py": TWICE_HERE,
    "twice_elsewhere.py": TWICE_ELSEWHERE,
}


@pytest.fixture
def repo(tmp_path):
    """A repository carrying an import, a shadow and an unconnected caller."""
    for relative, text in FILES.items():
        (tmp_path / relative).write_text(text, encoding="utf-8")
    return resolve_repo(tmp_path, None)


def tier_by_path(result):
    """Map each group's file to the tier it was labelled with."""
    return {group.path: group.tier for group in result.groups}


class TestTierLabels:
    def test_the_defining_file_is_labelled_same_file(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(
            repo, "py:core.py::helper"
        )
        assert tier_by_path(result)["core.py"] == "same_file"

    def test_an_importing_file_is_labelled_resolved_via_import(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(
            repo, "py:core.py::helper"
        )
        assert tier_by_path(result)["user.py"] == "imported"

    def test_a_shadowing_file_is_labelled_name_only(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(
            repo, "py:core.py::helper"
        )
        assert tier_by_path(result)["shadow.py"] == "ambiguous"

    def test_a_shadowing_file_is_still_reported(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(
            repo, "py:core.py::helper"
        )
        assert "shadow.py" in tier_by_path(result)

    def test_the_repository_s_only_definition_is_labelled_unique(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(
            repo, "py:core.py::only_once"
        )
        assert tier_by_path(result)["stranger.py"] == "unique"

    def test_a_co_located_member_is_labelled_same_file_member(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(
            repo, "py:service.py::Handlers.refresh"
        )
        assert tier_by_path(result)["service.py"] == "same_file_member"

    def test_a_member_defined_twice_stays_ambiguous(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(
            repo, "py:twice_here.py::Local.sync"
        )
        assert tier_by_path(result)["twice_here.py"] == "ambiguous"

    def test_a_module_scope_definition_still_outranks_the_member_tier(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(
            repo, "py:core.py::only_once"
        )
        assert tier_by_path(result)["stranger.py"] == "unique"

    def test_the_label_reaches_the_rendered_rows(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(
            repo, "py:core.py::helper"
        )
        rendered = render.render_ref_groups(result.groups, "helper")
        assert "resolved-via-import\n  user.py  (2 references)" in rendered
        assert "name-only-ambiguous" in rendered

    def test_the_json_form_carries_the_tier(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(
            repo, "py:core.py::helper"
        )
        document = result.as_dict()
        assert {group["tier"] for group in document["groups"]} == {
            "same_file",
            "imported",
            "ambiguous",
        }
