#!/usr/bin/env bash
# Prove that every workspace gate fails on its own violation.
# Copies the tree into a scratch git repository with no remote, checks that the
# base passes every gate, then applies one violation at a time and checks that
# the matching gate fails with the matching message. It needs network access:
# cargo deny checks advisories and yanked crates, violations 11 to 13 add
# crates that cargo fetches, and the hook trigger check fetches PyYAML.
set -euo pipefail

gate_bin=${1:?usage: scripts/gate-selftest.sh PATH-TO-PINNED-AGENTLESS-MCP}
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

# deadline.pl SECONDS COMMAND...: run COMMAND in a new session. At the deadline (exit 124) or on Ctrl-C,
# kill each process below it and their process groups. When it ends, kill what is left in its group,
# so that no process holds its output open.
cat >"$scratch/deadline.pl" <<'PL'
use strict;
use warnings;
use POSIX ();

my $seconds = shift @ARGV;
pipe(my $reader, my $writer) or die "deadline: cannot open a pipe: $!\n";
my $leader = fork;
defined $leader or die "deadline: cannot fork: $!\n";
if ($leader == 0) {
    # The leader outlives the command, so its group id stays ours until the parent kills it.
    close $reader;
    POSIX::setsid() > 0 or die "deadline: cannot start a session: $!\n";
    my $command = fork;
    defined $command or die "deadline: cannot fork: $!\n";
    if ($command == 0) {
        close $writer;
        exec { $ARGV[0] } @ARGV;
        exit 127;
    }
    waitpid($command, 0);
    print {$writer} "$?\n";
    close $writer;
    sleep 1 while 1;
}
close $writer;

# Kill each process below the leader and each process group they are in: a child can start its own group.
sub stop {
    open my $ps, '-|', 'ps', '-A', '-o', 'pid=', '-o', 'ppid=', '-o', 'pgid='
        or die "deadline: cannot run ps: $!\n";
    my (%children, %group);
    while (my $line = <$ps>) {
        my ($pid, $ppid, $pgid) = split ' ', $line;
        push @{ $children{$ppid} }, $pid;
        $group{$pid} = $pgid;
    }
    close $ps or die "deadline: ps exited with status $?\n";
    my @tree = ($leader);
    for (my $next = 0; $next < @tree; $next++) {
        push @tree, @{ $children{ $tree[$next] } || [] };
    }
    my %groups = map { $_ => 1 } $leader, grep { defined } @group{@tree};
    kill 'KILL', @tree;
    kill '-KILL', keys %groups;
}

my $caught = '';
$SIG{$_} = sub { $caught ||= $_[0]; stop() } for qw(ALRM INT TERM HUP);
alarm $seconds;
my $status = <$reader>;
alarm 0;
stop();
waitpid($leader, 0);
if ($caught eq 'ALRM') {
    print STDERR "deadline: @ARGV ran past its $seconds s deadline: it and the processes it started were killed\n";
    exit 124;
}
if ($caught) {
    $SIG{$caught} = 'DEFAULT';
    kill $caught, $$;
}
defined $status or die "deadline: the leader of @ARGV ended before the command did\n";
exit($status & 127 ? 128 + ($status & 127) : $status >> 8);
PL

# triggers.py CONFIG ROOT: print each listed file name that a cargo hook trigger does not match.
cat >"$scratch/triggers.py" <<'PY'
"""Fail when a cargo hook trigger misses a file name that its gate lists."""

import re
import sys
from pathlib import Path

import yaml

# The hook, the Rust file and constant that list the names, and whether the gate reads a name in any directory.
LISTS = [
    ("cargo-gates", "xtask/src/tool_config.rs", "TOOL_FILES", True),
    ("cargo-policy", "xtask/src/policy.rs", "FILES", False),
    ("cargo-policy", "xtask/src/policy.rs", "CLIPPY_NAMES", True),
]


def names(source: Path, constant: str) -> list[str]:
    text = source.read_text(encoding="utf-8")
    body = re.search(rf"const {constant}\b[^=]*=\s*\[(.*?)\];", text, re.S)
    found = re.findall(r'"([^"]+)"', body.group(1)) if body else []
    if not found:
        raise SystemExit(f"{source}: {constant} lists no file names")
    return found


def spellings(name: str, anywhere: bool) -> list[str]:
    paths = [name, f"crates/kernel/{name}"] if anywhere else [name]
    # A case-insensitive file system serves CLIPPY.TOML as clippy.toml, so the trigger must ignore case.
    return [spelling for path in paths for spelling in (path, path.upper())]


def main() -> int:
    config_path, root = Path(sys.argv[1]), Path(sys.argv[2])
    config = yaml.safe_load(config_path.read_text(encoding="utf-8"))
    # pre-commit's defaults: no exclude drops nothing, and no files matches every file.
    excluded = re.compile(config.get("exclude", "^$"))
    triggers = {
        hook["id"]: re.compile(hook.get("files", ""))
        for repo in config["repos"]
        for hook in repo["hooks"]
    }
    misses = []
    for hook, source, constant, anywhere in LISTS:
        if hook not in triggers:
            raise SystemExit(f"{config_path}: no {hook} hook")
        for name in names(root / source, constant):
            misses += [
                f"the {hook} trigger misses {path}"
                for path in spellings(name, anywhere)
                if not triggers[hook].search(path) or excluded.search(path)
            ]
    print("\n".join(misses))
    return 1 if misses else 0


if __name__ == "__main__":
    sys.exit(main())
PY

git -C "$root" ls-files -z --cached --others --exclude-standard \
    | tar -C "$root" --null -T - -cf - | tar -C "$work" -xf -
cd "$work"
git init -q
# A scratch identity; each commit carries a body, as a commit-msg hook may require.
git config user.name selftest
git config user.email selftest@invalid
git add -A
git commit -q -m base -m "The tree every violation below starts from."
base=$(git rev-parse HEAD)

# The erosion gate. Append the base revision. An array, because the deadline runs a program, not a function.
erosion=(cargo xtask erosion --lint-bin "$gate_bin" --health-bin "$gate_bin" --base)
# The hook trigger check, with the PyYAML version that uv.lock holds.
triggers=(uv run --no-project --with pyyaml==6.0.3 python -I "$scratch/triggers.py" .pre-commit-config.yaml .)

deadline() {
    perl "$scratch/deadline.pl" "$step_deadline" "$@" </dev/null
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
    git clean -q -fdx -e target
}

commit() {
    git add -A
    git commit -q -m violation -m "One violation, which one gate must catch."
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

passes "cargo fmt" cargo fmt --all --check
passes "cargo clippy" cargo clippy --workspace --all-targets -- -D warnings
passes "tests and snapshots" cargo insta test --check --workspace --test-runner nextest
passes "cargo deny" cargo deny check
passes "cargo machete" cargo machete
passes "xtask layers" cargo xtask layers
passes "xtask suppressions" cargo xtask suppressions
passes "xtask lint-tables" cargo xtask lint-tables
passes "xtask static-gates" cargo xtask static-gates
passes "xtask public-api" cargo xtask public-api
passes "xtask erosion" "${erosion[@]}" "$base"
passes "xtask policy" cargo xtask policy --base "$base"
passes "hook triggers match the gates' file lists" "${triggers[@]}"

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

# 6. An upward crate edge: kernel (layer 0) depends on prompts (layer 1). Then no
# package under crates/, and a path dependency outside the workspace.
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

# 7. A new public function in kernel, the same in a lib target at a custom path, and a
# snapshot in a package with no lib target.
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
fails "7c public-api.txt with no lib target" "xtask/public-api.txt belongs to a package with no library target" \
    cargo xtask public-api

# 8. A health figure above the base's baseline. Then a tree baseline above the lowest
# committed one or deleted, and a --base that is missing, an option, not a commit, or
# given twice. Then a base with no baseline, and health that counts at another threshold.
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
# a macro in ffi.rs, a nested crate with no lint table, and a hidden file.
syntax_crate() {
    mkdir -p crates/syntax/src
    cat >crates/syntax/Cargo.toml <<'EOF'
[package]
name = "syntax"
version.workspace = true
edition.workspace = true
publish.workspace = true

[lints.rust]
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

# 11. A license outside the allow-list, and separately a git source.
violate
printf '\n[dependencies.option-ext]\nversion = "0.2"\n' >>crates/kernel/Cargo.toml
fails "11a cargo deny licenses" "MPL-2.0" cargo deny check licenses
violate
printf '\n[dependencies.itoa]\ngit = "https://github.com/dtolnay/itoa"\n' >>crates/kernel/Cargo.toml
fails "11b cargo deny sources" "source-not-allowed" cargo deny check sources

# 12. An unused dependency.
violate
printf '\n[dependencies.itoa]\nversion = "1"\n' >>crates/kernel/Cargo.toml
fails "12 cargo machete" "itoa" cargo machete

# 13. A snapshot that was never accepted.
violate
printf '\n[dev-dependencies.insta]\nversion = "1.49"\n' >>crates/kernel/Cargo.toml
mkdir -p crates/kernel/tests
printf '#[test]\nfn pending() {\n    insta::assert_snapshot!("value");\n}\n' >crates/kernel/tests/selftest.rs
fails "13 cargo insta test --check" "snapshot assertion for 'pending' failed" \
    cargo insta test --check -p kernel --test-runner nextest

# 14 is reserved for the golden files (P1).

# 15. A tool config file outside the allowed set, and an extra root cargo alias.
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

# 16. A symlink under crates/.
violate
ln -s lib.rs crates/kernel/src/alias.rs
fails "16 symlink under crates" "a symlink under crates/ or xtask/ is refused" cargo xtask suppressions

# 17. A crate root or a package outside the files the gates scan.
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

# 18. static-gates fails when one of its gates fails.
violate
printf '\n#[expect(clippy::too_many_lines)]\nfn long() {}\n' >>crates/kernel/src/error.rs
fails "18 xtask static-gates" "xtask static-gates: suppressions failed" cargo xtask static-gates

# 19. Code that hides a suppression or a file read from the gates: a # that starts no
# attribute, a tt fragment, a macro call named by a fragment, a fragment in a use item,
# the identifier include, a shebang line, a nested macro_rules!, and a .RS extension.
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
# under crates/io, which would read crates/io's clippy config.
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

# 22. A policy file loosened in a commit with no Policy-Change trailer, the same commit
# with the trailer, and a policy file loosened in the working tree.
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

# 23. A hook trigger and its gate's file list that drift apart, from either side.
violate
sed -i.bak 's/"deny.exceptions.toml"/"deny.drift.toml"/' xtask/src/tool_config.rs && rm xtask/src/tool_config.rs.bak
fails "23a a gate file name that the trigger misses" "the cargo-gates trigger misses deny.drift.toml" \
    "${triggers[@]}"
violate
sed -i.bak 's/|\\\.agentless-mcp\\\.json|/|/' .pre-commit-config.yaml && rm .pre-commit-config.yaml.bak
fails "23b a trigger that drops a gate file name" "the cargo-gates trigger misses .agentless-mcp.json" \
    "${triggers[@]}"

if ((failures > 0)); then
    echo "$failures of $((bases + checks)) checks failed"
    exit 1
fi
echo "the base passes $bases checks, and each of $checks violations fails its gate"
