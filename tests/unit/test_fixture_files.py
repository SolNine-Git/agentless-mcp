"""The shared fixture-generator helpers in scripts/fixture_files.py."""

import importlib
import os
import pwd
import subprocess
import sys
from pathlib import Path
from types import ModuleType

import pytest

SCRIPTS = Path(__file__).resolve().parents[2] / "scripts"
IDENTITY = ("fixture tests", "fixture@example.invalid", "2026-02-01T00:00:00Z")


@pytest.fixture
def helpers(monkeypatch: pytest.MonkeyPatch) -> ModuleType:
    monkeypatch.syspath_prepend(SCRIPTS)
    return importlib.import_module("fixture_files")


def _write(path: Path, text: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def test_a_file_that_no_builder_produced_is_extra(helpers: ModuleType, tmp_path: Path):
    produced = _write(tmp_path / "kept.json", "{}")
    orphan = _write(tmp_path / "orphan.json", "{}")
    _write(tmp_path / "tags.db", "")
    _write(tmp_path / "other" / "deep.json", "{}")
    assert helpers.extra_files([produced], ()) == [orphan]


def test_a_shared_file_or_directory_is_never_extra(helpers: ModuleType, tmp_path: Path):
    produced = _write(tmp_path / "a" / "kept.json", "{}")
    shared = _write(tmp_path / "a" / "theirs.json", "{}")
    other = _write(tmp_path / "b" / "kept.json", "{}")
    _write(tmp_path / "b" / "theirs.json", "{}")
    assert helpers.extra_files([produced], [shared]) == []
    assert helpers.extra_files([produced, other], [shared, tmp_path / "b"]) == []


def test_the_differences_name_stale_then_extra_files(helpers: ModuleType, tmp_path: Path):
    fresh = _write(tmp_path / "fresh.json", "same")
    stale = _write(tmp_path / "stale.json", "old")
    missing = tmp_path / "missing.json"
    orphan = _write(tmp_path / "orphan.json", "{}")
    contents = {fresh: b"same", stale: b"new", missing: b"new"}
    assert helpers.differences(contents) == [
        f"stale: {stale}",
        f"stale: {missing}",
        f"extra: {orphan}",
    ]


def test_check_reports_an_extra_file_and_writes_nothing(
    helpers: ModuleType, tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys
):
    _write(tmp_path / "orphan.json", "{}")
    contents = {tmp_path / "made.json": b"{}\n"}
    monkeypatch.setattr(sys, "argv", ["generator", "--check"])
    monkeypatch.setattr(helpers, "PYTHON", sys.version_info[:2])
    assert helpers.write_or_check_files("test", tmp_path, lambda: contents) == 1
    assert not (tmp_path / "made.json").exists()
    assert capsys.readouterr().err.splitlines() == [
        f"stale: {tmp_path / 'made.json'}",
        f"extra: {tmp_path / 'orphan.json'}",
    ]


def test_git_env_drops_every_git_name_and_pins_the_identity(helpers: ModuleType):
    base = {
        "PATH": "/bin",
        "GIT_DIR": "/elsewhere",
        "GIT_INDEX_FILE": "/x",
        "GIT_CONFIG_GLOBAL": "/y",
    }
    env = helpers.git_env(helpers.Identity(*IDENTITY), base)
    assert env == {
        "PATH": "/bin",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_AUTHOR_NAME": IDENTITY[0],
        "GIT_AUTHOR_EMAIL": IDENTITY[1],
        "GIT_AUTHOR_DATE": IDENTITY[2],
        "GIT_COMMITTER_NAME": IDENTITY[0],
        "GIT_COMMITTER_EMAIL": IDENTITY[1],
        "GIT_COMMITTER_DATE": IDENTITY[2],
    }


def test_run_git_commits_in_its_directory_despite_a_git_dir(helpers: ModuleType, tmp_path: Path):
    repo = tmp_path / "repo"
    repo.mkdir()
    env = {**helpers.hermetic_env(tmp_path / "home"), "GIT_DIR": str(tmp_path / "elsewhere")}
    identity = helpers.Identity(*IDENTITY)
    helpers.run_git(repo, identity, "init", "--quiet", "-b", "main", env=env)
    helpers.run_git(repo, identity, "commit", "--quiet", "--allow-empty", "-m", "x", env=env)
    shown = helpers.run_git(repo, identity, "log", "-1", "--format=%an <%ae> %aI", env=env)
    assert shown.decode().strip() == "fixture tests <fixture@example.invalid> 2026-02-01T00:00:00Z"
    assert not (tmp_path / "elsewhere").exists()


def test_a_failed_git_call_raises(helpers: ModuleType, tmp_path: Path):
    env = helpers.hermetic_env(tmp_path / "home")
    identity = helpers.Identity(*IDENTITY)
    with pytest.raises(subprocess.CalledProcessError):
        helpers.run_git(tmp_path, identity, "rev-parse", "HEAD", env=env)


def test_scratch_caches_pass_the_guard(helpers: ModuleType, tmp_path: Path):
    environ = {
        "HOME": str(tmp_path / "home"),
        "XDG_CACHE_HOME": str(tmp_path / "cache"),
        helpers.GRAMMAR_OVERRIDE: str(tmp_path / "libs"),
    }
    helpers.refuse_real_caches(environ)
    helpers.refuse_real_caches({"HOME": str(tmp_path / "home")})


@pytest.mark.parametrize(
    ("name", "place"),
    [
        ("HOME", ""),
        ("XDG_CACHE_HOME", ".cache"),
        ("TREE_SITTER_LANGUAGE_PACK_CACHE_DIR", ".cache/tree-sitter-language-pack/v1.14.3/libs"),
    ],
)
def test_a_name_that_reaches_the_real_cache_is_refused(
    helpers: ModuleType, tmp_path: Path, name: str, place: str
):
    real_home = Path(pwd.getpwuid(os.getuid()).pw_dir)
    environ = {"HOME": str(tmp_path / "home"), name: str(real_home / place)}
    with pytest.raises(SystemExit, match=f"refused: {name} puts a cache in"):
        helpers.refuse_real_caches(environ)


def test_an_unset_home_is_the_real_home_and_is_refused(helpers: ModuleType, tmp_path: Path):
    with pytest.raises(SystemExit, match="refused: HOME puts a cache in"):
        helpers.refuse_real_caches({"XDG_CACHE_HOME": str(tmp_path / "cache")})
