"""Fail when pre-commit would skip a cargo hook on a file its tool reads, or at its stage.

Usage: python -I scripts/hook_triggers.py CONFIG ROOT. pre-commit's own code decides which files a
hook runs on, so the top-level files and exclude keys, a hook's exclude, and its type filters count.
That code is internal API: run the pre-commit that uv.lock holds.
"""

import os
import re
import sys
import tempfile
from pathlib import Path
from typing import Any

from pre_commit.clientlib import load_config
from pre_commit.commands.run import Classifier
from pre_commit.hook import Hook
from pre_commit.prefix import Prefix
from pre_commit.repository import _hook

INSTALLED = ("pre-commit", "pre-push")
PUSH = ("pre-push",)
# Of the stages that default_install_hook_types installs, the ones each cargo hook runs at.
STAGES = {
    "cargo-fmt": INSTALLED,
    "cargo-clippy": INSTALLED,
    "cargo-gates": INSTALLED,
    "cargo-deps": INSTALLED,
    "cargo-test": PUSH,
    "cargo-public-api": PUSH,
    "cargo-ground-truth": PUSH,
    "cargo-erosion": PUSH,
    "cargo-policy": PUSH,
}
# Their tools can read any repository file.
EVERY_FILE = ("cargo-ground-truth", "cargo-erosion")
# cargo and rustup read these for every cargo hook.
CARGO = [
    "Cargo.toml",
    "Cargo.lock",
    "crates/kernel/Cargo.toml",
    "rust-toolchain.toml",
    ".cargo/config.toml",
]
SOURCES = ["crates/kernel/src/lib.rs", "xtask/src/main.rs"]
# Each hook's inputs beyond CARGO. tool_config refuses a control or non-ASCII character in any path,
# and the Kelvin sign folds to an ASCII letter under ignore case. The policy hook runs on xtask.
INPUTS = dict.fromkeys(STAGES, SOURCES) | {
    "cargo-gates": [
        *SOURCES,
        "crates/dependencies.toml",
        "docs/a\x01.md",
        "docs/a\x7f.md",
        f"docs/{chr(0xE9)}.md",
        f"docs/{chr(0x212A)}.md",
    ],
    "cargo-policy": ["xtask/src/policy.rs"],
}
# The hook, the Rust file and constant that list file names, and whether the gate reads a name in
# any directory.
LISTS = [
    ("cargo-gates", "xtask/src/tool_config.rs", "TOOL_FILES", True),
    ("cargo-policy", "xtask/src/policy.rs", "FILES", False),
    ("cargo-policy", "xtask/src/tool_config.rs", "CLIPPY_NAMES", True),
]
# A file that no list names: only the every-file hooks must run on it.
OTHER = "README.md"


def main() -> int:
    """Print each cargo hook trigger or stage that misses its gate, and fail when one does."""
    config = load_config(sys.argv[1])
    inputs = _inputs(Path(sys.argv[2]))
    hooks = _cargo_hooks(config)
    with tempfile.TemporaryDirectory() as scratch:
        os.chdir(scratch)
        # The classifier reads each file's type from disk.
        paths = sorted({path for hook_paths in inputs.values() for path in hook_paths})
        for path in paths:
            Path(path).parent.mkdir(parents=True, exist_ok=True)
            Path(path).write_text("x = 1\n", encoding="utf-8")
        classifier = Classifier.from_config(paths, config["files"], config["exclude"])
        problems = _install_problems(config) + _problems(hooks, classifier, inputs)
    if problems:
        print("\n".join(problems))
    return 1 if problems else 0


def _inputs(root: Path) -> dict[str, list[str]]:
    listed = _listed(root)
    inputs = {hook: CARGO + INPUTS[hook] + listed.get(hook, []) for hook in STAGES}
    every = sorted({path for paths in inputs.values() for path in paths} | {OTHER})
    inputs.update(dict.fromkeys(EVERY_FILE, every))
    # A case-insensitive file system serves CLIPPY.TOML as clippy.toml: a trigger must ignore case.
    return {
        hook: [spelling for path in paths for spelling in (path, path.upper())]
        for hook, paths in inputs.items()
    }


def _listed(root: Path) -> dict[str, list[str]]:
    listed: dict[str, list[str]] = {}
    for hook, source, constant, anywhere in LISTS:
        for name in _names(root / source, constant):
            paths = [name, f"crates/kernel/{name}"] if anywhere else [name]
            listed.setdefault(hook, []).extend(paths)
    return listed


def _names(source: Path, constant: str) -> list[str]:
    text = source.read_text(encoding="utf-8")
    body = re.search(rf"const {constant}\b[^=]*=\s*\[(.*?)\];", text, re.DOTALL)
    items = re.findall(r'"([^"]+)"|\b([A-Z][A-Z0-9_]*)\b', body.group(1)) if body else []
    found = [literal or _constant(text, name, source) for literal, name in items]
    if not found:
        message = f"{source}: {constant} lists no file names"
        raise SystemExit(message)
    return found


def _constant(text: str, name: str, source: Path) -> str:
    value = re.search(rf'const {name}: &str = "([^"]+)";', text)
    if value is None:
        message = f"{source}: {name} is not a string constant in this file"
        raise SystemExit(message)
    return value.group(1)


def _cargo_hooks(config: dict[str, Any]) -> dict[str, Hook]:
    # _hook applies default_stages, as pre-commit does before it runs a hook.
    prefix = Prefix(str(Path.cwd()))
    return {
        raw["id"]: Hook.create(repo["repo"], prefix, _hook(raw, root_config=config))
        for repo in config["repos"]
        if repo["repo"] == "local"
        for raw in repo["hooks"]
        if raw["id"].startswith("cargo-")
    }


def _install_problems(config: dict[str, Any]) -> list[str]:
    installed = config["default_install_hook_types"]
    used = sorted({stage for stages in STAGES.values() for stage in stages})
    return [
        f"default_install_hook_types does not hold {stage}: pre-commit install sets no {stage}"
        " git hook, so the cargo hooks at that stage do not run"
        for stage in used
        if stage not in installed
    ]


def _problems(
    hooks: dict[str, Hook], classifier: Classifier, inputs: dict[str, list[str]]
) -> list[str]:
    unknown = [hook for hook in hooks if hook not in STAGES]
    problems = [f"{hook}: no stages are set for this cargo hook" for hook in unknown]
    for hook_id, want in STAGES.items():
        hook = hooks.get(hook_id)
        if hook is None:
            problems.append(f"no {hook_id} hook")
        else:
            problems += _hook_problems(hook, want, classifier, inputs[hook_id])
    return problems


def _hook_problems(
    hook: Hook, want: tuple[str, ...], classifier: Classifier, inputs: list[str]
) -> list[str]:
    stages = tuple(stage for stage in INSTALLED if stage in hook.stages)
    wrong = stages != want
    problems = [f"the {hook.id} hook runs at {list(stages)}, not {list(want)}"] if wrong else []
    matched = set(classifier.filenames_for_hook(hook))
    missed = [path for path in inputs if path not in matched]
    return problems + [f"the {hook.id} trigger misses {path}" for path in missed]


if __name__ == "__main__":
    sys.exit(main())
