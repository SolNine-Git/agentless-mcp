#!/usr/bin/env bash
# Prove that every workspace gate fails on its own violation.
# Copies the tree into a scratch git repository with no remote, checks that the
# base passes every gate, then applies one violation at a time and checks that
# the matching gate fails with the matching message. It needs network access:
# cargo deny checks advisories and yanked crates, violations 11 to 13 and 25f
# add crates that cargo fetches, and the hook trigger check installs the dev
# group that uv.lock holds.
set -euo pipefail

gate_bin=${1:?usage: scripts/gate-selftest.sh PATH-TO-PINNED-AGENTLESS-MCP}
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(git rev-parse --show-toplevel)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
work=$scratch/work
outside=$scratch/outside
mkdir "$work"
export CARGO_TARGET_DIR="$root/target/selftest"
# Seconds for each gate command, so that a stalled fetch or build fails its row and does not hang the job.
step_deadline=120
timed_out=124
failures=0
bases=0
checks=0
# Characters for file names that the gates refuse: the long s in UTF-8, and two control characters.
long_s=$(printf '\305\277')
start_of_heading=$'\001'
line_feed=$'\n'

git -C "$root" ls-files -z --cached --others --exclude-standard \
    | tar -C "$root" --null -T - -cf - | tar -C "$work" -xf -
cd "$work"
# From here on, no hook or setting of the owner's applies: no GIT_ variable, no global or system git
# config, and an empty XDG_CONFIG_HOME, where git, uv and nextest find their user files.
while IFS= read -r name; do unset "$name"; done < <(compgen -e | grep '^GIT_' || true)
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 XDG_CONFIG_HOME=$scratch/config
mkdir "$XDG_CONFIG_HOME"
git init -q
# A commit starts git maintenance in the background, which can still write to .git when the EXIT trap
# removes the scratch directory.
git config maintenance.auto false
git config gc.auto 0
# A scratch identity, because the isolated config holds none.
git config user.name selftest
git config user.email selftest@invalid
git add -A
git commit -q -m base -m "The tree every violation below starts from."
base=$(git rev-parse HEAD)

# The erosion gate. Append the base revision. An array, because the deadline runs a program, not a function.
erosion=(cargo xtask erosion --lint-bin "$gate_bin" --health-bin "$gate_bin" --base)
# The hook trigger check runs the pre-commit that uv.lock holds, because it calls pre-commit's
# internal API. The dev group goes into a scratch environment.
triggers=(env UV_PROJECT_ENVIRONMENT="$scratch/venv" uv run --frozen --only-group dev
    python -I "$here/hook_triggers.py" .pre-commit-config.yaml .)
# As CI and the cargo-deps hook run it: an ignore file, a hidden directory or a directory named target
# hides no manifest under the roots.
machete=(cargo machete --no-ignore crates xtask)

deadline() {
    perl "$here/deadline.pl" "$step_deadline" "$@" </dev/null
}

passes() {
    local name=$1 status=0
    shift
    bases=$((bases + 1))
    deadline "$@" >/dev/null 2>&1 || status=$?
    if ((status == 0)); then
        echo "ok   base passes: $name"
    elif ((status == timed_out)); then
        echo "FAIL base fails: $name: it ran past its $step_deadline s deadline"
        failures=$((failures + 1))
    else
        echo "FAIL base fails: $name"
        failures=$((failures + 1))
    fi
}

# fails NAME TEXT COMMAND...: the gate must exit non-zero and print TEXT, a fixed string.
fails() {
    local name=$1 text=$2 output status=0
    shift 2
    checks=$((checks + 1))
    output=$(deadline "$@" 2>&1) || status=$?
    if ((status == 0)); then
        echo "FAIL $name: the gate passed"
        failures=$((failures + 1))
    elif ((status == timed_out)); then
        echo "FAIL $name: the gate ran past its $step_deadline s deadline"
        failures=$((failures + 1))
    elif grep -qF -- "$text" <<<"$output"; then
        echo "ok   $name"
    else
        echo "FAIL $name: the gate failed without \"$text\""
        failures=$((failures + 1))
    fi
}

violate() {
    git reset -q --hard "$base"
    # Only the root target/, which ground-truth builds in: a row can add a directory crates/target.
    git clean -q -fdx -e /target
}

# commit [-m PARAGRAPH]...: commit every change. A last paragraph can carry a trailer.
commit() {
    git add -A
    git commit -q -m violation -m "One violation, which one gate must catch." "$@"
}

# lines N: set clippy.toml's too-many-lines-threshold, a policy value, to N.
lines() {
    sed -i.bak "s/^too-many-lines-threshold = .*/too-many-lines-threshold = $1/" clippy.toml && rm clippy.toml.bak
}

# allow_entry FILE LINT COUNT: list one suppression in crates/suppressions.toml.
allow_entry() {
    sed -i.bak '/^allow = \[\]$/d' crates/suppressions.toml && rm crates/suppressions.toml.bak
    printf '\n[[allow]]\nfile = "%s"\nlint = "%s"\ncount = %s\nreason = "selftest"\n' "$1" "$2" "$3" \
        >>crates/suppressions.toml
}

raise_baseline() {
    printf '{\n  "erosion": 1.0,\n  "over_threshold": 99,\n  "clone_share": 1.0\n}\n' >crates/erosion-baseline.json
}

# running PID...: print each PID whose process has not ended. A zombie has ended.
running() {
    local pid stat
    for pid in "$@"; do
        stat=$(ps -o stat= -p "$pid" || true)
        stat=${stat// /}
        if [[ -n $stat && $stat != Z* ]]; then
            printf '%s ' "$pid"
        fi
    done
}

# leader_problem: start deadline.pl on a shell with a background sleep, then SIGKILL perl. Print a
# problem if either process still runs 2 s later, and kill both.
leader_problem() {
    local pids=$scratch/leader-pids helper tries=0 started=() left
    perl "$here/deadline.pl" "$step_deadline" sh -c 'sleep 60 & echo "$$ $!" >"$1"; wait' probe "$pids" \
        </dev/null >/dev/null 2>&1 &
    helper=$!
    until [[ -s $pids ]] || ((tries == 50)); do
        sleep 0.1
        tries=$((tries + 1))
    done
    kill -KILL "$helper"
    wait "$helper" 2>/dev/null || true
    if [[ ! -s $pids ]]; then
        echo "the command did not start in 5 s"
        return
    fi
    read -r -a started <"$pids"
    left=$(running "${started[@]}")
    tries=0
    while [[ -n $left ]] && ((tries < 20)); do
        sleep 0.1
        tries=$((tries + 1))
        left=$(running "${started[@]}")
    done
    if [[ -n $left ]]; then
        echo "processes ${left}still run 2 s after perl died"
        kill -KILL "${started[@]}" 2>/dev/null || true
    fi
}

# deadline.pl's leader must kill the command when perl itself dies, or a killed run leaves it running.
bases=$((bases + 1))
leader=$(leader_problem)
if [[ -z $leader ]]; then
    echo "ok   base passes: deadline.pl kills its command when perl dies"
else
    echo "FAIL base fails: deadline.pl kills its command when perl dies: $leader"
    failures=$((failures + 1))
fi

passes "cargo fmt" cargo fmt --all --check
passes "cargo clippy" cargo clippy --workspace --all-targets -- -D warnings
passes "tests and snapshots" cargo insta test --check --workspace --test-runner nextest
passes "cargo deny" cargo deny check
passes "cargo machete" "${machete[@]}"
passes "xtask layers" cargo xtask layers
passes "xtask suppressions" cargo xtask suppressions
passes "xtask lint-tables" cargo xtask lint-tables
passes "xtask static-gates" cargo xtask static-gates
passes "xtask ground-truth" cargo xtask ground-truth
passes "xtask public-api" cargo xtask public-api
passes "xtask erosion" "${erosion[@]}" "$base"
passes "xtask policy" cargo xtask policy --base "$base"
passes "hook triggers and stages match the gates" "${triggers[@]}"

# 1. A function above complexity 10.
violate
{
    echo 'pub fn branchy(x: u32) -> u32 {'
    for i in $(seq 1 12); do echo "    if x == $i { return $i; }"; done
    echo '    0'
    echo '}'
} >crates/kernel/src/branchy.rs
commit
fails "1 complexity_delta" "] complexity_delta  crates/kernel/src/branchy.rs:1" "${erosion[@]}" "$base"

# 2. A copied block of 40 or more tokens.
violate
sed -n '/fn from_str/,/^    }/p' crates/kernel/src/key.rs \
    | sed 's/fn from_str/fn parse_again/' >crates/kernel/src/copied.rs
commit
fails "2 clone_delta" "] clone_delta  crates/kernel/src/copied.rs:1" "${erosion[@]}" "$base"

# 3. unwrap_or_default() on a parsed value in a boundary crate.
violate
mkdir -p crates/io/src
printf '[package]\nname = "io"\nversion.workspace = true\nedition.workspace = true\npublish.workspace = true\n\n[lints]\nworkspace = true\n' \
    >crates/io/Cargo.toml
printf 'pub fn port(text: &str) -> u32 {\n    text.parse().unwrap_or_default()\n}\n' >crates/io/src/lib.rs
commit
fails "3 contract_drift" "] contract_drift  crates/io/src/lib.rs:2" "${erosion[@]}" "$base"

# 4. A file that does not parse: lint cannot check it, and health cannot measure it.
violate
printf 'pub fn broken( {\n' >crates/kernel/src/broken.rs
commit
fails "4 not_checked" "[not_checked] complexity_delta  crates/kernel/src/broken.rs" "${erosion[@]}" "$base"
fails "4b health cannot measure a file" "health --json cannot judge the change: it could not measure 1 files" \
    "${erosion[@]}" "$base"

# 5. An expect attribute, an inner allow attribute, each other attribute that
# suppresses a lint or hides a file, and suppressions.toml entries that do not match.
# Then an ignore attribute, which turns a test off.
violate
printf '\n#[expect(clippy::too_many_lines)]\nfn long() {}\n' >>crates/kernel/src/error.rs
fails "5a expect attribute" "clippy::too_many_lines" cargo xtask suppressions
violate
printf '#![allow(dead_code)]\n' >crates/kernel/src/inner.rs
fails "5b inner allow attribute" "dead_code" cargo xtask suppressions
violate
printf '\n#[expect/*c*/(dead_code)]\nfn unused_c() {}\n' >>crates/kernel/src/error.rs
fails "5c expect with a comment inside" "the dead_code suppression has no entry" \
    cargo xtask suppressions
violate
printf '#![expect(dead_code)]\n' >crates/kernel/src/b.txt
printf '\n#[path = "b.txt"]\nmod b;\n' >>crates/kernel/src/lib.rs
fails "5d path attribute" "#[path] is refused" cargo xtask suppressions
violate
printf '#[expect(dead_code)]\nfn unused_e() {}\n' >crates/kernel/src/c.txt
printf '\ninclude!("c.txt");\n' >>crates/kernel/src/lib.rs
fails "5e include!" "include! is refused" cargo xtask suppressions
violate
printf '#[expect(dead_code)]\nfn unused_f() {}\n' >crates/kernel/src/c.txt
printf '\nuse std::include as i;\ni!("c.txt");\n' >>crates/kernel/src/lib.rs
fails "5f include! renamed by a use item" "a use item that names include is refused" \
    cargo xtask suppressions
violate
printf '\nmacro_rules! quiet {\n    ($a:meta) => {\n        #[$a]\n        fn unused_g() {}\n    };\n}\nquiet!(expect(dead_code));\n' \
    >>crates/kernel/src/error.rs
fails "5g attribute from a macro fragment" "the gates cannot read" cargo xtask suppressions
violate
printf '#![warn(warnings)]\n' >crates/kernel/src/warn.rs
fails "5h warn attribute" "the warnings suppression has no entry" cargo xtask suppressions
violate
printf '#![clippy::msrv = "1.0"]\n' >crates/kernel/src/msrv.rs
fails "5i clippy tool attribute" "the clippy::msrv suppression has no entry" cargo xtask suppressions
violate
printf '\n#[rustfmt::skip]\n/// Skip.\npub const SKIP: [u8; 2] = [1,2];\n' >>crates/kernel/src/lib.rs
fails "5j rustfmt::skip" "the rustfmt::skip suppression has no entry" cargo xtask suppressions
violate
printf '#[expect(dead_code)]\nfn unused_k() {}\n' >xtask/src/stray.rs
fails "5k expect in xtask" "xtask/src/stray.rs:1: the dead_code suppression has no entry" \
    cargo xtask suppressions
violate
printf '\n#[expect(dead_code)]\nfn unused_l() {}\n\n#[expect(dead_code)]\nfn unused_m() {}\n' \
    >>crates/kernel/src/error.rs
allow_entry crates/kernel/src/error.rs dead_code 1
fails "5l suppression count mismatch" "dead_code is suppressed 2 times" cargo xtask suppressions
violate
allow_entry crates/kernel/src/error.rs dead_code 1
fails "5m stale suppression entry" "is stale" cargo xtask suppressions
violate
printf '\n#[expect(dead_code)]\nfn unused_n() {}\n' >>crates/kernel/src/error.rs
allow_entry crates/kernel/src/error.rs dead_code 0
fails "5n entry with a count of 0" "has a count of 0" cargo xtask suppressions
violate
printf '\n#[expect(dead_code)]\nfn unused_o() {}\n' >>crates/kernel/src/error.rs
allow_entry crates/kernel/src/error.rs dead_code 1
allow_entry crates/kernel/src/error.rs dead_code 1
fails "5o entry listed twice" "is listed twice" cargo xtask suppressions
violate
cat >crates/kernel/src/probe.rs <<'R'
#[cfg(test)]
mod ignored {
    #[test]
    #[ignore = "skipped"]
    fn always_fails() {
        assert_eq!(1, 2);
    }
}
R
fails "5p ignore attribute" "crates/kernel/src/probe.rs:4: the ignore suppression has no entry" \
    cargo xtask suppressions

# 6. An upward crate edge: kernel (layer 0) depends on prompts (layer 1). Then no
# package under crates/, a path dependency outside the workspace, and a third-party
# dependency that crates/dependencies.toml does not list.
violate
mkdir -p crates/prompts/src
printf '[package]\nname = "prompts"\nversion.workspace = true\nedition.workspace = true\npublish.workspace = true\n\n[lints]\nworkspace = true\n' \
    >crates/prompts/Cargo.toml
: >crates/prompts/src/lib.rs
printf '\n[dependencies.prompts]\npath = "../prompts"\n' >>crates/kernel/Cargo.toml
fails "6 xtask layers" "depends on prompts" cargo xtask layers
violate
rm -rf crates/kernel
fails "6b no package under crates" "layers has nothing to check" cargo xtask layers
violate
mkdir -p "$outside/src"
printf '[package]\nname = "outside"\nversion = "0.1.0"\nedition = "2024"\n\n[workspace]\n' \
    >"$outside/Cargo.toml"
printf '#![expect(dead_code)]\nfn unused() {}\n/// One.\npub fn one() {}\n' >"$outside/src/lib.rs"
printf '\n[dependencies.outside]\npath = "%s"\n' "$outside" >>crates/kernel/Cargo.toml
printf 'outside = 0\n' >>crates/layers.toml
sed -i.bak 's/^kernel = 0$/kernel = 1/' crates/layers.toml && rm crates/layers.toml.bak
fails "6c path dependency outside the workspace" "which is not a workspace package" \
    cargo xtask layers
violate
printf '\n[dependencies.itoa]\nversion = "1"\n' >>crates/kernel/Cargo.toml
fails "6d an unlisted direct dependency" \
    "itoa is a direct third-party dependency of a workspace package, and crates/dependencies.toml does not list it in \`direct\`" \
    cargo xtask layers

# 7. A new public function in kernel, the same in a lib target at a custom path, and a
# snapshot in a package with no lib target or in a source directory.
violate
printf '\n/// Return one.\n#[must_use]\npub const fn one() -> u32 {\n    1\n}\n' >>crates/kernel/src/lib.rs
fails "7 xtask public-api" "public-api.txt" cargo xtask public-api
violate
mv crates/kernel/src/lib.rs crates/kernel/src/root.rs
printf '\n[lib]\npath = "src/root.rs"\n' >>crates/kernel/Cargo.toml
printf '\n/// Return one.\n#[must_use]\npub const fn one() -> u32 {\n    1\n}\n' >>crates/kernel/src/root.rs
fails "7b public-api reads the lib target" "public-api.txt does not match" cargo xtask public-api
violate
cp crates/kernel/public-api.txt xtask/public-api.txt
fails "7c public-api.txt with no lib target" "xtask/public-api.txt: this file is refused" cargo xtask static-gates
violate
cp crates/kernel/public-api.txt crates/kernel/src/public-api.txt
fails "7d public-api.txt in a source directory" "crates/kernel/src/public-api.txt: this file is refused" \
    cargo xtask static-gates

# 8. A health figure above the base's baseline. Then a tree baseline above the lowest
# committed one or deleted, and a --base that is missing, an option, not a commit, or
# given twice. Then a base with no baseline, health that counts at another threshold,
# health that walks part of the tree, and an edit that a skip-worktree mark hides. Then
# repository config that hides a committed edit from git diff: an external diff driver
# that git trusts, and a clean filter. diff.trustExitCode alone must not break the gate.
violate
printf '{\n  "erosion": 0.0,\n  "over_threshold": 0,\n  "clone_share": 0.0\n}\n' >crates/erosion-baseline.json
commit
fails "8 xtask erosion ratchet" "clone share rose" "${erosion[@]}" HEAD
violate
raise_baseline
commit
fails "8b tree baseline above the base's" "crates/erosion-baseline.json is above its lowest committed version" \
    "${erosion[@]}" "$base"
violate
rm crates/erosion-baseline.json
commit
fails "8c tree baseline deleted" "crates/erosion-baseline.json is missing, but a committed version exists" \
    "${erosion[@]}" "$base"
violate
fails "8d --base with no value" "usage: cargo xtask" \
    cargo xtask erosion --lint-bin "$gate_bin" --health-bin "$gate_bin" --base
violate
fails "8e --base with an option for a value" "usage: cargo xtask" \
    cargo xtask erosion --lint-bin "$gate_bin" --health-bin "$gate_bin" --base "--output=$scratch/injected"
violate
fails "8f --base names a tree" "expected commit type" "${erosion[@]}" "HEAD^{tree}"
violate
fails "8g --base given twice" "usage: cargo xtask" "${erosion[@]}" "$base" --base "$base"
violate
git rm -q crates/erosion-baseline.json
commit
no_baseline=$(git rev-parse HEAD)
git show "$base:crates/erosion-baseline.json" >crates/erosion-baseline.json
commit
raise_baseline
commit
fails "8h a raised baseline on a branch whose base has none" \
    "crates/erosion-baseline.json is above its lowest committed version" "${erosion[@]}" "$no_baseline"
violate
git rm -q crates/erosion-baseline.json
commit
no_baseline=$(git rev-parse HEAD)
raise_baseline
fails "8i no committed baseline, and the tree's above health" \
    "crates/erosion-baseline.json is above what health measures now" "${erosion[@]}" "$no_baseline"
violate
printf '{"complexity_threshold": 100}\n' >crates/.agentless-mcp.json
fails "8j health counts at another threshold" "it counted units over complexity 100, not the pinned 10" \
    "${erosion[@]}" "$base"

# fake_health FILES WALKED STOPPED: a health binary that reports a walk of WALKED of FILES files.
fake_health() {
    cat >"$scratch/health" <<EOF
#!/bin/sh
echo '{"complexity": {"threshold": 10, "files": $1, "walked": $2, "stopped_by_budget": $3,
    "not_measured": {"total": 0}, "erosion": 0.0, "over_threshold": 0, "clone_share": 0.0}}'
EOF
    chmod +x "$scratch/health"
}
violate
fake_health 5 5 true
fails "8k health stopped by its time budget" "health --json cannot judge the change: its time budget stopped the walk" \
    cargo xtask erosion --lint-bin "$gate_bin" --health-bin "$scratch/health" --base "$base"
violate
fake_health 5 3 false
fails "8l health walked part of the tree" "health --json cannot judge the change: it walked 3 of 5 files" \
    cargo xtask erosion --lint-bin "$gate_bin" --health-bin "$scratch/health" --base "$base"
violate
printf '\n/// Added.\npub const ADDED: u8 = 1;\n' >>crates/kernel/src/id.rs
git update-index --skip-worktree crates/kernel/src/id.rs
fails "8m an edit that a skip-worktree mark hides" "tracked files with an assume-unchanged or skip-worktree mark: 1" \
    "${erosion[@]}" "$base"
git update-index --no-skip-worktree crates/kernel/src/id.rs

# io_drift: commit a boundary crate crates/io as io_base, then an edit that lint reports as a
# contract drift. The base holds the crate's lock entry: an uncommitted one is a change that lint reads.
io_drift() {
    mkdir -p crates/io/src
    printf '[package]\nname = "io"\nversion.workspace = true\nedition.workspace = true\npublish.workspace = true\n\n[lints]\nworkspace = true\n' \
        >crates/io/Cargo.toml
    printf 'pub fn port(text: &str) -> Option<u32> {\n    text.parse().ok()\n}\n' >crates/io/src/lib.rs
    deadline cargo metadata --format-version 1 --offline >/dev/null
    commit
    io_base=$(git rev-parse HEAD)
    printf 'pub fn port(text: &str) -> u32 {\n    text.parse().unwrap_or_default()\n}\n' >crates/io/src/lib.rs
    commit
}
violate
io_drift
mkdir -p .git/info
printf '*.rs diff=quiet\n' >.git/info/attributes
git config diff.quiet.command true
git config diff.quiet.trustExitCode true
fails "8n an external diff driver that git trusts" "] contract_drift  crates/io/src/lib.rs:2" \
    "${erosion[@]}" "$io_base"
rm .git/info/attributes
git config --unset diff.quiet.command
git config --unset diff.quiet.trustExitCode
violate
printf '\n// A remark.\n' >>crates/kernel/src/lib.rs
commit
git config diff.trustExitCode true
passes "xtask erosion with diff.trustExitCode set" "${erosion[@]}" "$base"
git config --unset diff.trustExitCode
violate
io_drift
git show "$io_base:crates/io/src/lib.rs" >"$scratch/io-lib.rs"
mkdir -p .git/info
printf 'crates/io/src/lib.rs filter=hide\n' >.git/info/attributes
git config filter.hide.clean "cat '$scratch/io-lib.rs'"
fails "8o a clean filter that hides a committed edit" "lint found erosion since $io_base" "${erosion[@]}" "$io_base"
rm .git/info/attributes
git config --unset filter.hide.clean

# 9. A file write outside the io crate, and a macro-built allow of a lint at forbid.
violate
printf '\n/// Write a file.\npub fn save() {\n    let _ = std::fs::write("x", "y");\n}\n' >>crates/kernel/src/lib.rs
fails "9 clippy disallowed_methods" "disallowed" cargo clippy -p kernel -- -D warnings
violate
printf '\nmod probe;\n' >>crates/kernel/src/lib.rs
cat >crates/kernel/src/probe.rs <<'R'
macro_rules! hash_attr {
    ($h:tt $_d:tt) => {
        $h[allow(clippy::dbg_macro)]
        #[cfg(test)]
        fn print_one() -> u8 {
            dbg!(1)
        }
    };
}

hash_attr!(
    /// harmless words
);

#[cfg(test)]
mod tests {
    #[test]
    fn prints() {
        assert_eq!(super::print_one(), 1);
    }
}
R
fails "9b allow of a forbidden lint" "allow(clippy::dbg_macro) incompatible with previous forbid" \
    cargo clippy -p kernel --all-targets -- -D warnings

# 10. unsafe outside syntax's ffi.rs, and a syntax lint table that drifts. Then
# a macro in ffi.rs, a nested crate with no lint table, and a hidden file. Then
# global_asm!, which needs no unsafe token, and a target or a package that leaves the
# workspace edition or rust-version.
syntax_crate() {
    mkdir -p crates/syntax/src
    cat >crates/syntax/Cargo.toml <<'EOF'
[package]
name = "syntax"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
publish.workspace = true

[lints.rust]
unexpected_cfgs = "forbid"
unsafe_op_in_unsafe_fn = "deny"

[lints.clippy]
pedantic = { level = "warn", priority = -1 }
allow_attributes = "forbid"
dbg_macro = "forbid"
todo = "forbid"
unimplemented = "forbid"
undocumented_unsafe_blocks = "deny"
EOF
    : >crates/syntax/src/lib.rs
}
# The control: without it, a helper table that drifts would make 10b pass on its own.
violate
syntax_crate
passes "xtask lint-tables with a syntax crate" cargo xtask lint-tables
violate
syntax_crate
printf 'pub fn f() {\n    unsafe {}\n}\n' >crates/syntax/src/lib.rs
fails "10a unsafe outside ffi.rs" "ffi.rs only" cargo xtask lint-tables
violate
syntax_crate
sed -i.bak '/allow_attributes/d' crates/syntax/Cargo.toml && rm crates/syntax/Cargo.toml.bak
fails "10b syntax lint table drift" "crates/syntax/Cargo.toml" cargo xtask lint-tables
violate
syntax_crate
printf 'fn main() {\n    let x = 1;\n    // SAFETY: a test.\n    let _ = unsafe { std::ptr::read(&raw const x) };\n}\n' \
    >crates/syntax/build.rs
fails "10c unsafe in syntax's build.rs" "ffi.rs only" cargo xtask lint-tables
violate
syntax_crate
mkdir -p crates/syntax/src/sub
printf 'pub mod sub;\n' >crates/syntax/src/lib.rs
printf 'pub mod ffi;\n' >crates/syntax/src/sub.rs
printf '/// Read.\n#[must_use]\npub fn f() -> u8 {\n    let x = 1;\n    // SAFETY: a test.\n    unsafe { std::ptr::read(&raw const x) }\n}\n' \
    >crates/syntax/src/sub/ffi.rs
fails "10d unsafe in a nested ffi.rs" "ffi.rs only" cargo xtask lint-tables
violate
syntax_crate
printf 'macro_rules! raw {\n    ($x:expr) => {\n        // SAFETY: a test.\n        unsafe { std::ptr::read(&raw const $x) }\n    };\n}\n' \
    >crates/syntax/src/ffi.rs
printf '#[macro_use]\nmod ffi;\n\n/// Read.\n#[must_use]\npub fn f() -> u8 {\n    let x = 1;\n    raw!(x)\n}\n' \
    >crates/syntax/src/lib.rs
fails "10e macro_rules in ffi.rs" "macro_rules! is refused here" cargo xtask lint-tables
violate
mkdir -p crates/kernel/nested/foo/src
printf '[package]\nname = "foo"\nversion.workspace = true\nedition.workspace = true\npublish.workspace = true\n' \
    >crates/kernel/nested/foo/Cargo.toml
printf '//! Foo.\n' >crates/kernel/nested/foo/src/lib.rs
printf '\n[dependencies.foo]\npath = "nested/foo"\n' >>crates/kernel/Cargo.toml
fails "10f nested crate with no lint table" "nested/foo/Cargo.toml: [lints] must be" \
    cargo xtask lint-tables
violate
printf '#![expect(dead_code)]\n' >crates/kernel/src/b.txt
printf '\n#[path = "b.txt"]\nmod b;\n' >>crates/kernel/src/lib.rs
fails "10g lint-tables refuses a path attribute" "#[path] is refused" cargo xtask lint-tables
violate
printf 'core::arch::global_asm!("");\n' >crates/kernel/src/probe.rs
fails "10h global_asm! outside ffi.rs" "crates/kernel/src/probe.rs:1: \`global_asm\` is unsafe code" \
    cargo xtask lint-tables
violate
printf '\n[lib]\nedition = "2021"\n' >>crates/kernel/Cargo.toml
fails "10i a lib target on an older edition" \
    "crates/kernel: target kernel builds with edition 2021, not the workspace edition" cargo xtask lint-tables
violate
sed -i.bak 's/^rust-version.workspace = true$/rust-version = "1.85"/' crates/kernel/Cargo.toml
rm crates/kernel/Cargo.toml.bak
fails "10j a package off the workspace rust-version" \
    "crates/kernel: rust-version is 1.85, not the workspace rust-version" cargo xtask lint-tables

# 11. A license outside the allow-list, and separately a git source.
violate
printf '\n[dependencies.option-ext]\nversion = "0.2"\n' >>crates/kernel/Cargo.toml
fails "11a cargo deny licenses" "MPL-2.0" cargo deny check licenses
violate
printf '\n[dependencies.itoa]\ngit = "https://github.com/dtolnay/itoa"\n' >>crates/kernel/Cargo.toml
fails "11b cargo deny sources" "source-not-allowed" cargo deny check sources

# 12. An unused dependency, then the same in a manifest that .gitignore names, and in a
# package in a directory named target.
violate
printf '\n[dependencies.itoa]\nversion = "1"\n' >>crates/kernel/Cargo.toml
fails "12 cargo machete" "itoa" "${machete[@]}"
violate
printf '\n[dependencies.itoa]\nversion = "1"\n' >>crates/kernel/Cargo.toml
printf '/crates/kernel/Cargo.toml\n' >>.gitignore
fails "12b cargo machete reads an ignored manifest" "itoa" "${machete[@]}"
violate
mkdir -p crates/target/src
printf '[package]\nname = "tgt"\nversion = "0.1.0"\nedition = "2024"\n\n[dependencies.itoa]\nversion = "1"\n' \
    >crates/target/Cargo.toml
: >crates/target/src/lib.rs
fails "12c cargo machete reads a package in crates/target" "crates/target/Cargo.toml" "${machete[@]}"

# 13. A snapshot that was never accepted.
violate
printf '\n[dev-dependencies.insta]\nversion = "1.49"\n' >>crates/kernel/Cargo.toml
mkdir -p crates/kernel/tests
printf '#[test]\nfn pending() {\n    insta::assert_snapshot!("value");\n}\n' >crates/kernel/tests/selftest.rs
fails "13 cargo insta test --check" "snapshot assertion for 'pending' failed" \
    cargo insta test --check -p kernel --test-runner nextest

# 14 is reserved for the golden files (P1).

# 15. A tool config file outside the allowed set, and an extra root cargo alias. Then
# ignore files that a walker reads, tool config files that git ignores, an exempt package
# that reads another clippy config, a non-ASCII name, and a control character in a name.
violate
sed '/disallowed-methods/,$d' clippy.toml >crates/clippy.toml
fails "15a crates/clippy.toml" "this tool config file is refused" cargo xtask lint-tables
violate
sed '/disallowed-methods/,$d' clippy.toml >crates/kernel/.clippy.toml
fails "15b crates/kernel/.clippy.toml" "this tool config file is refused" cargo xtask lint-tables
violate
mkdir -p crates/kernel/.cargo
printf '[build]\nrustflags = ["--cap-lints", "allow"]\n' >crates/kernel/.cargo/config.toml
fails "15c nested .cargo/config.toml" "this tool config file is refused" cargo xtask lint-tables
violate
printf 'max_width = 200\n' >crates/kernel/rustfmt.toml
fails "15d nested rustfmt.toml" "this tool config file is refused" cargo xtask lint-tables
violate
printf 'machete = "version"\n' >>.cargo/config.toml
fails "15e an extra cargo alias" ".cargo/config.toml must hold only the xtask alias" \
    cargo xtask lint-tables
violate
printf 'exceptions = [{ allow = ["MPL-2.0"], crate = "option-ext" }]\n' >deny.exceptions.toml
fails "15f deny.exceptions.toml" "deny.exceptions.toml: this tool config file is refused" cargo xtask lint-tables
violate
cp deny.toml .deny.toml
fails "15g .deny.toml" ".deny.toml: this tool config file is refused" cargo xtask lint-tables
violate
printf '{"complexity_threshold": 100}\n' >crates/.agentless-mcp.json
fails "15h crates/.agentless-mcp.json" "crates/.agentless-mcp.json: this tool config file is refused" \
    cargo xtask lint-tables
violate
printf 'crates/kernel/Cargo.toml\n' >.ignore
fails "15i .ignore" ".ignore: this tool config file is refused" cargo xtask lint-tables
violate
printf 'crates/kernel/Cargo.toml\n' >.rgignore
fails "15j .rgignore" ".rgignore: this tool config file is refused" cargo xtask lint-tables
violate
printf 'kernel/Cargo.toml\n' >crates/.ignore
fails "15k crates/.ignore" "crates/.ignore: this tool config file is refused" cargo xtask lint-tables
violate
printf 'kernel/Cargo.toml\n' >crates/.gitignore
fails "15l crates/.gitignore" "crates/.gitignore: this file is refused" cargo xtask lint-tables
violate
printf 'max_width = 200\n' >.rustfmt.toml
printf '/.rustfmt.toml\n' >>.gitignore
fails "15m an ignored .rustfmt.toml" ".rustfmt.toml: this tool config file is refused" cargo xtask lint-tables
violate
mkdir -p docs/.cargo
printf '[build]\nrustflags = ["--cap-lints", "allow"]\n' >docs/.cargo/config.toml
printf 'docs/.cargo/\n' >>.gitignore
fails "15n an ignored docs/.cargo/config.toml" "docs/.cargo/config.toml: this tool config file is refused" \
    cargo xtask lint-tables
violate
sed 's/^too-many-lines-threshold = 60$/too-many-lines-threshold = 1000/' xtask/clippy.toml >xtask/.clippy.toml
printf 'xtask/.clippy.toml\n' >>.gitignore
fails "15o an ignored xtask/.clippy.toml" \
    "xtask: clippy reads xtask/.clippy.toml for this package: it must read xtask/clippy.toml" cargo xtask lint-tables
violate
mkdir -p crates/io/src
printf '[package]\nname = "io"\nversion.workspace = true\nedition.workspace = true\nrust-version.workspace = true\npublish.workspace = true\n\n[lints]\nworkspace = true\n' \
    >crates/io/Cargo.toml
printf '//! Io.\n' >crates/io/src/lib.rs
fails "15p crates/io without its own clippy.toml" \
    "crates/io: clippy reads clippy.toml for this package: it must read crates/io/clippy.toml" cargo xtask lint-tables
violate
# APFS folds this name to rustfmt.toml, so it lives in a directory that holds no rustfmt.toml.
mkdir -p docs/fold-probe
cp rustfmt.toml "docs/fold-probe/ru${long_s}tfmt.toml"
fails "15q a non-ASCII tool config name" "docs/fold-probe/ru${long_s}tfmt.toml: a non-ASCII path is refused" \
    cargo xtask lint-tables
violate
printf 'notes\n' >"docs/a${start_of_heading}b.md"
fails "15r a control character in a path" 'docs/a\u{1}b.md: a path with a control character is refused' \
    cargo xtask lint-tables

# 16. A symlink under crates/.
violate
ln -s lib.rs crates/kernel/src/alias.rs
fails "16 symlink under crates" "a symlink under crates/ or xtask/ is refused" cargo xtask suppressions

# 17. A crate root or a package outside the files the gates scan. Then a FIFO that would
# block the scan, and files under crates/ that no rule allows: a note, the manifest of an
# excluded crate, and a source whose name is not ASCII.
violate
mv crates/kernel/src/lib.rs crates/kernel/src/lib.txt
printf '\n[lib]\npath = "src/lib.txt"\n' >>crates/kernel/Cargo.toml
fails "17a a .txt crate root" "a target's source must be a .rs file inside its package" \
    cargo xtask suppressions
violate
printf '//! Kernel.\n' >docs/kernel.rs
printf '\n[lib]\npath = "../../docs/kernel.rs"\n' >>crates/kernel/Cargo.toml
fails "17b a crate root path with .." "a target's source must be a .rs file inside its package" \
    cargo xtask suppressions
violate
mkdir -p tools/foo/src
printf '[package]\nname = "foo"\nversion.workspace = true\nedition.workspace = true\npublish.workspace = true\n\n[lints]\nworkspace = true\n' \
    >tools/foo/Cargo.toml
printf '//! Foo.\n' >tools/foo/src/lib.rs
printf '\n[dependencies.foo]\npath = "../../tools/foo"\n' >>crates/kernel/Cargo.toml
fails "17c package outside crates and xtask" "a workspace package must be xtask or live under crates/" \
    cargo xtask suppressions
violate
printf '//! Kernel.\n' >docs/kernel.rs
printf '\n[lib]\npath = "%s/docs/kernel.rs"\n' "$work" >>crates/kernel/Cargo.toml
fails "17d an absolute crate root outside the package" \
    "a target's source must be a .rs file inside its package" cargo xtask suppressions
violate
mkfifo crates/kernel/src/fifo.rs
fails "17e a FIFO named .rs" \
    "crates/kernel/src/fifo.rs: an entry that is not a regular file, a directory or a symlink is refused" \
    cargo xtask suppressions
# git clean leaves a FIFO in place.
rm crates/kernel/src/fifo.rs
violate
printf 'notes\n' >crates/kernel/notes.txt
fails "17f a file that no rule allows" "crates/kernel/notes.txt: this file is refused" cargo xtask suppressions
violate
mkdir -p crates/old/src
sed 's/^name = "kernel"$/name = "old"/' crates/kernel/Cargo.toml >crates/old/Cargo.toml
printf '//! Old.\n' >crates/old/src/lib.rs
sed -i.bak 's|^members = \["crates/\*", "xtask"\]$|&\
exclude = ["crates/old"]|' Cargo.toml && rm Cargo.toml.bak
fails "17g an excluded crate" "crates/old/Cargo.toml: this file is refused" cargo xtask suppressions
violate
printf 'fn hidden() {}\n' >"crates/kernel/src/probe.r${long_s}"
fails "17h a non-ASCII source name" "crates/kernel/src/probe.r${long_s}: a non-ASCII path is refused" \
    cargo xtask lint-tables

# 18. static-gates fails when one of its gates fails.
violate
printf '\n#[expect(clippy::too_many_lines)]\nfn long() {}\n' >>crates/kernel/src/error.rs
fails "18 xtask static-gates" "xtask static-gates: suppressions failed" cargo xtask static-gates

# 19. Code that hides a suppression or a file read from the gates: a # that starts no
# attribute, a tt fragment, a macro call named by a fragment, a fragment in a use item,
# the identifier include, a shebang line, a nested macro_rules!, a .RS extension, and a
# macro_rules! named by a fragment.
violate
cat >crates/kernel/src/probe.rs <<'R'
macro_rules! hash_attr {
    ($h:tt) => {
        $h[allow(dead_code)]
        fn never_used() {}
    };
}

hash_attr!(#);
R
fails "19a a # passed to a macro" \
    "crates/kernel/src/probe.rs:8: a # that does not start #[..] or #![..] is refused" cargo xtask suppressions
violate
cat >crates/kernel/src/probe.rs <<'R'
macro_rules! group_attr {
    ($b:tt) => {
        #$b
        fn never_used() {}
    };
}

group_attr!([allow(dead_code)]);
R
fails "19b a # before a fragment" \
    "crates/kernel/src/probe.rs:3: a # that does not start #[..] or #![..] is refused" cargo xtask suppressions
violate
cat >crates/kernel/src/probe.rs <<'R'
macro_rules! join {
    ($h:tt, $b:tt) => {
        $h $b
        fn never_used() {}
    };
}

join!(#, [allow(dead_code)]);
R
fails "19c a # joined to a group" \
    "crates/kernel/src/probe.rs:8: a # that does not start #[..] or #![..] is refused" cargo xtask suppressions
violate
cat >crates/kernel/src/probe.rs <<'R'
macro_rules! hash_attr {
    ($h:tt $_d:tt) => {
        $h[allow(dead_code)]
        fn never_used() {}
    };
}

hash_attr!(
    /// harmless words
);
R
fails "19d a doc comment split by tt fragments" "crates/kernel/src/probe.rs:2: the tt fragment is refused" \
    cargo xtask suppressions
violate
cat >crates/kernel/src/probe.rs <<'R'
macro_rules! hash_attr {
    ($h:tt) => {
        $h[path = "payload.txt"]
        mod hidden;
    };
}

hash_attr!(#);
R
printf '#![allow(dead_code)]\nfn never_used() {}\n' >crates/kernel/src/payload.txt
fails "19e a path attribute built by a macro" \
    "crates/kernel/src/probe.rs:8: a # that does not start #[..] or #![..] is refused" cargo xtask suppressions
violate
cat >crates/kernel/src/probe.rs <<'R'
macro_rules! read_in {
    ($name:ident) => {
        $name!("payload.txt");
    };
}

read_in!(include);
R
printf '#[expect(dead_code)]\nfn never_used() {}\n' >crates/kernel/src/payload.txt
fails "19f include! named by a fragment" \
    "crates/kernel/src/probe.rs:3: a macro call named by a fragment is refused" cargo xtask suppressions
violate
cat >crates/kernel/src/probe.rs <<'R'
macro_rules! read_in {
    ($($p:ident)*) => {
        $($p)*!("payload.txt");
    };
}

read_in!(include);
R
printf '#[expect(dead_code)]\nfn never_used() {}\n' >crates/kernel/src/payload.txt
fails "19g include! named by a repetition" \
    "crates/kernel/src/probe.rs:3: a macro call named by a fragment is refused" cargo xtask suppressions
violate
cat >crates/kernel/src/probe.rs <<'R'
macro_rules! bring {
    ($name:ident) => {
        use std::$name as inc;
    };
}

bring!(include);
inc!("payload.txt");
R
printf '#[expect(dead_code)]\nfn never_used() {}\n' >crates/kernel/src/payload.txt
fails "19h include named by a fragment in a use item" \
    "crates/kernel/src/probe.rs:3: a fragment other than \$crate in a use item is refused" cargo xtask suppressions
violate
cat >crates/kernel/src/probe.rs <<'R'
macro_rules! bring {
    ($u:ident) => {
        $u std::include as inc;
    };
}

bring!(use);
inc!("payload.txt");
R
printf '#[expect(dead_code)]\nfn never_used() {}\n' >crates/kernel/src/payload.txt
fails "19i the identifier include" "crates/kernel/src/probe.rs:3: the identifier include is refused" \
    cargo xtask suppressions
violate
printf '#!/usr/bin/env /*\n#![expect(dead_code)]\n// */\nfn never_used() {}\n' >crates/kernel/src/probe.rs
fails "19j a shebang line" "crates/kernel/src/probe.rs:1: a first line that starts with #! and not #![ is refused" \
    cargo xtask suppressions
violate
cat >crates/kernel/src/probe.rs <<'R'
macro_rules! define {
    ($name:ident, $($body:tt)*) => {
        macro_rules! $name {
            () => { $($body)* };
        }
    };
}
R
fails "19k macro_rules! inside a macro_rules! body" \
    "crates/kernel/src/probe.rs:1: macro_rules! inside a macro_rules! body is refused" cargo xtask suppressions
violate
printf 'fn never_used() {}\n' >crates/kernel/src/probe.RS
fails "19l a .RS file" "crates/kernel/src/probe.RS: a .rs extension in another case is refused" \
    cargo xtask suppressions
violate
cat >crates/kernel/src/probe.rs <<'R'
macro_rules! define {
    ($define:ident, $name:ident) => {
        $define! $name {
            () => {};
        }
    };
}

define!(macro_rules, defined);
R
fails "19m a macro_rules! named by a fragment" \
    "crates/kernel/src/probe.rs:3: a macro call named by a fragment is refused" cargo xtask suppressions

# 20. Code hidden from clippy: a cfg predicate that names clippy or holds a fragment,
# cfg!(clippy), and a build script outside crates/syntax.
violate
cat >crates/kernel/src/probe.rs <<'R'
#[cfg(all(test, not(clippy)))]
mod hidden {
    #[test]
    fn hidden_from_clippy() {
        let value = dbg!(1);
        assert_eq!(value, 1);
    }
}
R
fails "20a cfg(not(clippy))" \
    "crates/kernel/src/probe.rs:1: a cfg predicate that names clippy or holds a fragment is refused" \
    cargo xtask suppressions
violate
cat >crates/kernel/src/probe.rs <<'R'
macro_rules! gate {
    ($p:ident) => {
        #[cfg(all(test, not($p)))]
        mod hidden {
            #[test]
            fn hidden_from_clippy() {
                let value = dbg!(1);
                assert_eq!(value, 1);
            }
        }
    };
}

gate!(clippy);
R
fails "20b a fragment in a cfg predicate" \
    "crates/kernel/src/probe.rs:3: a cfg predicate that names clippy or holds a fragment is refused" \
    cargo xtask suppressions
violate
cat >crates/kernel/src/probe.rs <<'R'
#[cfg(test)]
mod tests {
    #[test]
    fn linted() {
        assert!(!cfg!(clippy));
    }
}
R
fails "20c cfg!(clippy)" \
    "crates/kernel/src/probe.rs:5: a cfg predicate that names clippy or holds a fragment is refused" \
    cargo xtask suppressions
violate
cat >crates/kernel/build.rs <<'R'
//! Build script.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(real)");
    let wrapper = std::env::var("RUSTC_WORKSPACE_WRAPPER").unwrap_or_default();
    if !wrapper.contains("clippy") {
        println!("cargo::rustc-cfg=real");
    }
}
R
fails "20d a build script in kernel" "crates/kernel: build script build-script-build is refused" \
    cargo xtask lint-tables

# 21. A package no gate reads: a [patch] or [replace] path package, and a package nested
# under crates/io, which would read crates/io's clippy config. Then a package manifest
# with a profile, which cargo ignores, or a list of dependencies that machete skips.
fake_package() {
    mkdir -p vendor/fake/src
    printf '[package]\nname = "fake"\nversion = "1.0.0"\nedition = "2024"\n' >vendor/fake/Cargo.toml
    : >vendor/fake/src/lib.rs
}
violate
fake_package
printf '\n[patch.crates-io]\nfake = { path = "vendor/fake" }\n' >>Cargo.toml
fails "21a [patch]" "Cargo.toml: [patch] is refused" cargo xtask lint-tables
violate
fake_package
printf '\n[replace]\n"fake:1.0.0" = { path = "vendor/fake" }\n' >>Cargo.toml
fails "21b [replace]" "Cargo.toml: [replace] is refused" cargo xtask lint-tables
violate
mkdir -p crates/io/src crates/io/evil/src
printf '[package]\nname = "io"\nversion.workspace = true\nedition.workspace = true\npublish.workspace = true\n\n[lints]\nworkspace = true\n\n[dependencies]\nevil = { path = "evil" }\n' \
    >crates/io/Cargo.toml
printf '[package]\nname = "evil"\nversion.workspace = true\nedition.workspace = true\npublish.workspace = true\n\n[lints]\nworkspace = true\n' \
    >crates/io/evil/Cargo.toml
printf '//! Io.\n\npub use evil::write;\n' >crates/io/src/lib.rs
printf '//! Evil.\n\n/// Write a file.\npub fn write() {\n    let _ = std::fs::write("x", "y");\n}\n' \
    >crates/io/evil/src/lib.rs
grep -v 'std::fs' clippy.toml >crates/io/clippy.toml
fails "21c a package nested under crates/io" "crates/io/evil: clippy reads crates/io/clippy.toml for this package" \
    cargo xtask lint-tables
violate
printf '\n[profile.dev]\ndebug-assertions = false\n' >>crates/kernel/Cargo.toml
fails "21d [profile] in a package" "crates/kernel/Cargo.toml: [profile] is refused" cargo xtask lint-tables
violate
printf '\n[package.metadata.cargo-machete]\nignored = ["serde"]\n' >>crates/kernel/Cargo.toml
fails "21e cargo-machete metadata in a package" "crates/kernel/Cargo.toml: [package.metadata.cargo-machete] is refused" \
    cargo xtask lint-tables

# 22. A policy file loosened in a commit with no Policy-Change trailer, the same commit
# with the trailer, and a policy file loosened in the working tree. Then a reference on a
# sibling commit, a merge that brings back a value from before the reference, HEAD judged
# when the tree is back at the reference, a deletion in the index, the root manifest's
# keys, a NaN, and log.follow set where xtask's git calls would read it. Then a second
# commit that creates a policy file the base lacks, git config, a replace ref or a graft
# that would make a commit seem approved, a malformed trailer, and a change to the
# dependency list.
# unapproved FILE: what policy prints when HEAD changes FILE with no trailer.
unapproved() {
    echo "$1: commit $(git rev-parse HEAD) changes this policy file without a"
}
# merge_with COMMIT: commit the working tree as a merge of HEAD and COMMIT.
merge_with() {
    git add -A
    git reset -q --hard "$(git commit-tree "$(git write-tree)" -p HEAD -p "$1" -m merge -m "A merge for one violation.")"
}
# loosening_merge: an approved tightening of clippy.toml, then a merge that loosens it with no trailer.
loosening_merge() {
    local side
    printf 'side\n' >side.txt
    commit
    side=$(git rev-parse HEAD)
    git reset -q --hard "$base"
    lines 59
    commit -m "Policy-Change: clippy.toml"
    lines 1000
    merge_with "$side"
}
violate
sed -i.bak 's/^too-many-lines-threshold = 60$/too-many-lines-threshold = 1000/' clippy.toml && rm clippy.toml.bak
commit
fails "22 a policy change with no trailer" \
    "clippy.toml: commit $(git rev-parse HEAD) changes this policy file without a" cargo xtask policy --base "$base"
git commit -q --amend -m violation -m "One violation, which one gate must catch." -m "Policy-Change: clippy.toml"
passes "xtask policy with a Policy-Change trailer" cargo xtask policy --base "$base"
violate
sed -i.bak 's/^yanked = "deny"$/yanked = "allow"/' deny.toml && rm deny.toml.bak
fails "22b an uncommitted policy change" "deny.toml: the working tree changes this policy file" \
    cargo xtask policy --base "$base"
violate
lines 1000
commit -m "Policy-Change: clippy.toml"
sibling=$(git rev-parse HEAD)
git reset -q --hard "$base"
fails "22c a reference on a sibling commit" "clippy.toml: HEAD holds a value that no commit after the reference made" \
    cargo xtask policy --base "$sibling"
violate
lines 1000
commit -m "Policy-Change: clippy.toml"
stale=$(git rev-parse HEAD)
lines 60
commit -m "Policy-Change: clippy.toml"
reference=$(git rev-parse HEAD)
lines 59
commit -m "Policy-Change: clippy.toml"
lines 1000
merge_with "$stale"
fails "22d a merge that brings back a stale value" "clippy.toml: HEAD holds a value that no commit after the reference made" \
    cargo xtask policy --base "$reference"
violate
lines 1000
commit
loosened=$(unapproved clippy.toml)
git show "$base:clippy.toml" >clippy.toml
fails "22e HEAD loosened, the tree back at the reference" "$loosened" cargo xtask policy --base "$base"
violate
git rm -q clippy.toml
fails "22f a policy file deleted in the index" "clippy.toml: the working tree changes this policy file" \
    cargo xtask policy --base "$base"
violate
sed -i.bak 's|^members = \["crates/\*", "xtask"\]$|members = ["crates/kernel", "xtask"]|' Cargo.toml
rm Cargo.toml.bak
commit
fails "22g workspace members" "$(unapproved Cargo.toml)" cargo xtask policy --base "$base"
violate
sed -i.bak 's|^members = \["crates/\*", "xtask"\]$|&\
exclude = ["crates/old"]|' Cargo.toml && rm Cargo.toml.bak
commit
fails "22h workspace exclude" "$(unapproved Cargo.toml)" cargo xtask policy --base "$base"
violate
sed -i.bak 's|^members = \["crates/\*", "xtask"\]$|&\
default-members = ["xtask"]|' Cargo.toml && rm Cargo.toml.bak
commit
fails "22i workspace default-members" "$(unapproved Cargo.toml)" cargo xtask policy --base "$base"
violate
printf '\n[workspace.metadata.cargo-machete]\nignored = ["serde"]\n' >>Cargo.toml
commit
fails "22j workspace metadata" "$(unapproved Cargo.toml)" cargo xtask policy --base "$base"
violate
printf '\n[profile.release]\ndebug-assertions = false\n' >>Cargo.toml
commit
fails "22k a release profile" "$(unapproved Cargo.toml)" cargo xtask policy --base "$base"
violate
sed -i.bak 's/^rust-version = .*/rust-version = "1.85"/' Cargo.toml && rm Cargo.toml.bak
commit
fails "22l the workspace rust-version" "$(unapproved Cargo.toml)" cargo xtask policy --base "$base"
violate
printf 'x = inf\n' >>clippy.toml
commit -m "Policy-Change: clippy.toml"
reference=$(git rev-parse HEAD)
sed -i.bak 's/^x = inf$/x = nan/' clippy.toml && rm clippy.toml.bak
commit
fails "22m inf changed to nan" "$(unapproved clippy.toml)" cargo xtask policy --base "$reference"
violate
loosening_merge
git config log.follow true
fails "22n log.follow in the repository config" "$(unapproved clippy.toml)" cargo xtask policy --base "$base"
git config --unset log.follow
violate
loosening_merge
fails "22o log.follow through GIT_CONFIG_COUNT" "$(unapproved clippy.toml)" \
    env GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=log.follow GIT_CONFIG_VALUE_0=true cargo xtask policy --base "$base"
violate
git rm -q clippy.toml
commit
lacking=$(git rev-parse HEAD)
git show "$base:clippy.toml" >clippy.toml
commit
created=$(git rev-parse HEAD)
git reset -q --hard "$lacking"
git show "$base:clippy.toml" >clippy.toml
lines 1000
commit
passes "xtask policy with one commit that creates a policy file" cargo xtask policy --base "$lacking"
loose=$(git rev-parse HEAD)
merge_with "$created"
fails "22p a second commit that creates a policy file" \
    "clippy.toml: commit $loose changes this policy file without a" cargo xtask policy --base "$lacking"
violate
lines 1000
commit -m "pc: clippy.toml"
git config trailer.pc.key Policy-Change
fails "22q a trailer alias in the repository config" "$(unapproved clippy.toml)" cargo xtask policy --base "$base"
git config --unset trailer.pc.key
violate
lines 1000
commit --cleanup=verbatim -m "Policy-Change: clippy.toml" -m "; reviewed by nobody"
git config core.commentChar ';'
fails "22r a trailer above a paragraph that core.commentChar hides" "$(unapproved clippy.toml)" \
    cargo xtask policy --base "$base"
git config --unset core.commentChar
violate
lines 1000
commit --cleanup=verbatim -m "Policy-Change: deny.toml$(printf '\037')clippy.toml"
fails "22s a 0x1f byte in a trailer value" "$(unapproved clippy.toml)" cargo xtask policy --base "$base"
violate
lines 1000
commit
replaced=$(git rev-parse HEAD)
git replace "$replaced" "$({
    git cat-file commit "$replaced"
    printf '\nPolicy-Change: clippy.toml\n'
} | git hash-object -t commit -w --stdin)"
fails "22t a replace ref that adds a trailer" "$(unapproved clippy.toml)" cargo xtask policy --base "$base"
git replace -d "$replaced" >/dev/null
violate
lines 1000
commit
loose=$(git rev-parse HEAD)
commit --allow-empty -m "Policy-Change: clippy.toml"
mkdir -p .git/info
printf '%s %s\n' "$(git rev-parse HEAD)" "$base" >.git/info/grafts
fails "22u a graft that drops a commit from the range" "clippy.toml: commit $loose changes this policy file without a" \
    cargo xtask policy --base "$base"
rm .git/info/grafts
violate
printf '\n[dependencies.itoa]\nversion = "1"\n' >>crates/kernel/Cargo.toml
sed -i.bak 's/^direct = \[/direct = ["itoa", /' crates/dependencies.toml && rm crates/dependencies.toml.bak
commit
fails "22v a dependency listed with no trailer" "$(unapproved crates/dependencies.toml)" \
    cargo xtask policy --base "$base"

# 23. A hook trigger and its gate's file list that drift apart, from either side. Then each
# pre-commit key that can switch a cargo hook off: a hook's exclude or types, the top-level
# files, the stages, a files key on a hook that must run on every file, a renamed hook, and
# the hook types that pre-commit install sets.
violate
sed -i.bak 's/"deny.exceptions.toml"/"deny.drift.toml"/' xtask/src/tool_config.rs && rm xtask/src/tool_config.rs.bak
fails "23a a gate file name that the trigger misses" "the cargo-gates trigger misses deny.drift.toml" \
    "${triggers[@]}"
violate
sed -i.bak 's/|\\\.agentless-mcp\\\.json|/|/' .pre-commit-config.yaml && rm .pre-commit-config.yaml.bak
fails "23b a trigger that drops a gate file name" "the cargo-gates trigger misses .agentless-mcp.json" \
    "${triggers[@]}"
violate
sed -i.bak 's/^      - id: cargo-gates$/&\
        exclude: (?i)\\.(toml|json)/' .pre-commit-config.yaml && rm .pre-commit-config.yaml.bak
fails "23c a hook exclude" "the cargo-gates trigger misses deny.toml" "${triggers[@]}"
violate
sed -i.bak 's/^      - id: cargo-gates$/&\
        types: [rust]/' .pre-commit-config.yaml && rm .pre-commit-config.yaml.bak
fails "23d a hook types filter" "the cargo-gates trigger misses deny.toml" "${triggers[@]}"
violate
printf 'files: ^src/\n' >>.pre-commit-config.yaml
fails "23e a top-level files key" "the cargo-fmt trigger misses Cargo.toml" "${triggers[@]}"
violate
sed -i.bak 's/^      - id: cargo-gates$/&\
        stages: [manual]/' .pre-commit-config.yaml && rm .pre-commit-config.yaml.bak
fails "23f a hook moved to the manual stage" "the cargo-gates hook runs at [], not" "${triggers[@]}"
violate
sed -i.bak 's/^      - id: cargo-erosion$/&\
        files: ^(crates|xtask)\//' .pre-commit-config.yaml && rm .pre-commit-config.yaml.bak
fails "23g a files key on an every-file hook" "the cargo-erosion trigger misses README.md" "${triggers[@]}"
violate
sed -i.bak 's/^      - id: cargo-ground-truth$/      - id: ground-truth/' .pre-commit-config.yaml
rm .pre-commit-config.yaml.bak
fails "23h a renamed ground-truth hook" "no cargo-ground-truth hook" "${triggers[@]}"
violate
sed -i.bak 's/^default_install_hook_types: \[pre-commit, pre-push\]$/default_install_hook_types: [pre-commit]/' \
    .pre-commit-config.yaml && rm .pre-commit-config.yaml.bak
fails "23i no pre-push in default_install_hook_types" "default_install_hook_types does not hold pre-push" \
    "${triggers[@]}"

# 24. A manifest key that drops tests from nextest: test = false, required-features,
# harness = false, and autotests = false beside a file in tests/.
violate
printf '\n[lib]\ntest = false\n' >>crates/kernel/Cargo.toml
fails "24a test = false" "crates/kernel: target kernel sets test = false" cargo xtask lint-tables
violate
mkdir -p crates/kernel/tests
printf '#[test]\nfn t() {}\n' >crates/kernel/tests/t.rs
printf '\n[features]\nx = []\n\n[[test]]\nname = "t"\npath = "tests/t.rs"\nrequired-features = ["x"]\n' \
    >>crates/kernel/Cargo.toml
fails "24b required-features" "crates/kernel: target t has required-features" cargo xtask lint-tables
violate
mkdir -p crates/kernel/tests
printf 'fn main() {}\n\n#[test]\nfn never() {\n    panic!();\n}\n' >crates/kernel/tests/h.rs
printf '\n[[test]]\nname = "h"\npath = "tests/h.rs"\nharness = false\n' >>crates/kernel/Cargo.toml
fails "24c harness = false" "crates/kernel/Cargo.toml: harness = false in a test target is refused" \
    cargo xtask lint-tables
violate
mkdir -p crates/kernel/tests
printf '#[test]\nfn t() {}\n' >crates/kernel/tests/t.rs
sed -i.bak 's/^publish.workspace = true$/&\
autotests = false/' crates/kernel/Cargo.toml && rm crates/kernel/Cargo.toml.bak
fails "24d autotests = false" "crates/kernel/tests/t.rs: cargo builds no test target from this file" \
    cargo xtask lint-tables

# 25. What only the compiler sees: a suppression that a macro builds, a file that rustc
# reads through include_str!, a cfg that rustc does not know, and the lint level or
# check-cfg list that would let one through. Then a build-time package that
# crates/dependencies.toml does not list, an ignored test that a macro builds, and an
# include_str! through a name with a line feed or through a symlink.
violate
printf '\nmod probe;\n' >>crates/kernel/src/lib.rs
cat >crates/kernel/src/probe.rs <<'R'
macro_rules! with {
    ($m:meta, $i:item) => {
        #[$m]
        $i
    };
}

with!(expect(dead_code), fn never_used() {});
R
fails "25a a macro-built expect" "crates/kernel/src/probe.rs:8: clippy --force-warn reports dead_code" \
    cargo xtask ground-truth
violate
printf '\npub mod probe;\n' >>crates/kernel/src/lib.rs
printf 'pub const NOTES: &str = include_str!("../../../README.md");\n' >crates/kernel/src/probe.rs
fails "25b include_str! of a file outside the scan" \
    "crates/kernel/src/lib.rs: rustc reads README.md, which is not a .rs file that the gates scan" \
    cargo xtask ground-truth
violate
printf '\n#[cfg(hidden)]\npub fn probe() {}\n' >>crates/kernel/src/limit.rs
fails "25c a cfg that rustc does not know" "unexpected \`cfg\` condition name: \`hidden\`" cargo xtask ground-truth
violate
sed -i.bak '/^unexpected_cfgs = "forbid"$/d' Cargo.toml && rm Cargo.toml.bak
fails "25d unexpected_cfgs below forbid" \
    "Cargo.toml: [workspace.lints.rust] does not set unexpected_cfgs = \"forbid\"" cargo xtask ground-truth
violate
sed -i.bak 's/^unexpected_cfgs = "forbid"$/unexpected_cfgs = { level = "forbid", check-cfg = ["cfg(hidden)"] }/' \
    Cargo.toml && rm Cargo.toml.bak
printf '\n#[cfg(hidden)]\npub fn probe() {}\n' >>crates/kernel/src/limit.rs
fails "25e a check-cfg list" "Cargo.toml: unexpected_cfgs has a check-cfg list" cargo xtask ground-truth
violate
printf '\n[dependencies.anyhow]\nversion = "1"\n' >>crates/kernel/Cargo.toml
sed -i.bak 's/^direct = \[/direct = ["anyhow", /' crates/dependencies.toml && rm crates/dependencies.toml.bak
fails "25f an unlisted build-time package" \
    "anyhow is a third-party package with a proc-macro or build-script target in the resolved graph, and crates/dependencies.toml does not list it in \`build-time\`" \
    cargo xtask ground-truth
violate
printf '\nmod probe;\n' >>crates/kernel/src/lib.rs
cat >crates/kernel/src/probe.rs <<'R'
#[cfg(test)]
mod tests {
    macro_rules! with {
        ($m:meta, $i:item) => {
            #[$m]
            $i
        };
    }

    with!(ignore = "quiet", #[test] fn always_fails() { assert_eq!(1, 2); });
}
R
fails "25g an ignore attribute that a macro builds" "differ in number" cargo xtask ground-truth
violate
printf 'unreviewed data\n' >"docs/a${line_feed}Cargo.toml"
cat >>crates/kernel/src/lib.rs <<'R'

/// Data read at compile time.
pub const NOTES: &str = include_str!("../../../docs/a\nCargo.toml");
R
fails "25h include_str! of a name with a line feed" "no blank line follows the rule" cargo xtask ground-truth
violate
mkdir -p evil/a/b evil/crates/kernel/src
printf 'unreviewed data\n' >evil/crates/kernel/src/limit.rs
ln -s ../evil/a/b docs/link
cat >>crates/kernel/src/lib.rs <<'R'

/// Data read at compile time.
pub const NOTES: &str = include_str!("../../../docs/link/../../crates/kernel/src/limit.rs");
R
fails "25i include_str! through a symlink outside crates" \
    "crates/kernel/src/lib.rs: rustc reads evil/crates/kernel/src/limit.rs" cargo xtask ground-truth

if ((failures > 0)); then
    echo "$failures of $((bases + checks)) checks failed"
    exit 1
fi
echo "the base passes $bases checks, and each of $checks violations fails its gate"
