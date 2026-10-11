#!/usr/bin/env bash
# Prove that every workspace gate fails on its own violation.
# Copies the tree into a scratch git repository with no remote, checks that the
# base passes every gate, then applies one violation at a time and checks that
# the matching gate fails with the matching message. It needs network access:
# cargo deny checks advisories and yanked crates, and violations 11 to 13 add
# crates that cargo fetches.
set -euo pipefail

gate_bin=${1:?usage: scripts/gate-selftest.sh PATH-TO-PINNED-AGENTLESS-MCP}
root=$(git rev-parse --show-toplevel)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
work=$scratch/work
outside=$scratch/outside
mkdir "$work"
export CARGO_TARGET_DIR="$root/target/selftest"
failures=0
bases=0
checks=0

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

# erosion REV: run the erosion gate against the base revision REV.
erosion() {
    cargo xtask erosion --lint-bin "$gate_bin" --health-bin "$gate_bin" --base "$1"
}

passes() {
    local name=$1
    shift
    bases=$((bases + 1))
    if "$@" >/dev/null 2>&1; then
        echo "ok   base passes: $name"
    else
        echo "FAIL base fails: $name"
        failures=$((failures + 1))
    fi
}

# fails NAME TEXT COMMAND...: the gate must exit non-zero and print TEXT, a fixed string.
fails() {
    local name=$1 text=$2 output
    shift 2
    checks=$((checks + 1))
    if output=$("$@" 2>&1); then
        echo "FAIL $name: the gate passed"
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
passes "xtask erosion" erosion "$base"

# 1. A function above complexity 10.
violate
{
    echo 'pub fn branchy(x: u32) -> u32 {'
    for i in $(seq 1 12); do echo "    if x == $i { return $i; }"; done
    echo '    0'
    echo '}'
} >crates/kernel/src/branchy.rs
commit
fails "1 complexity_delta" complexity_delta erosion "$base"

# 2. A copied block of 40 or more tokens.
violate
sed -n '/fn from_str/,/^    }/p' crates/kernel/src/key.rs \
    | sed 's/fn from_str/fn parse_again/' >crates/kernel/src/copied.rs
commit
fails "2 clone_delta" clone_delta erosion "$base"

# 3. unwrap_or_default() on a parsed value in a boundary crate.
violate
mkdir -p crates/io/src
printf '[package]\nname = "io"\nversion.workspace = true\nedition.workspace = true\npublish.workspace = true\n\n[lints]\nworkspace = true\n' \
    >crates/io/Cargo.toml
printf 'pub fn port(text: &str) -> u32 {\n    text.parse().unwrap_or_default()\n}\n' >crates/io/src/lib.rs
commit
fails "3 contract_drift" contract_drift erosion "$base"

# 4. A file that does not parse.
violate
printf 'pub fn broken( {\n' >crates/kernel/src/broken.rs
commit
fails "4 not_checked" not_checked erosion "$base"

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

# 7. A new public function in kernel, and the same in a lib target at a custom path.
violate
printf '\n/// Return one.\n#[must_use]\npub const fn one() -> u32 {\n    1\n}\n' >>crates/kernel/src/lib.rs
fails "7 xtask public-api" "public-api.txt" cargo xtask public-api
violate
mv crates/kernel/src/lib.rs crates/kernel/src/root.rs
printf '\n[lib]\npath = "src/root.rs"\n' >>crates/kernel/Cargo.toml
printf '\n/// Return one.\n#[must_use]\npub const fn one() -> u32 {\n    1\n}\n' >>crates/kernel/src/root.rs
fails "7b public-api reads the lib target" "public-api.txt does not match" cargo xtask public-api

# 8. A health figure above the base's baseline. Then a tree baseline above the
# base's or deleted, and a --base that is missing, an option, or not a commit.
violate
printf '{\n  "erosion": 0.0,\n  "over_threshold": 0,\n  "clone_share": 0.0\n}\n' >crates/erosion-baseline.json
commit
fails "8 xtask erosion ratchet" "clone share rose" erosion HEAD
violate
printf '{\n  "erosion": 1.0,\n  "over_threshold": 99,\n  "clone_share": 1.0\n}\n' >crates/erosion-baseline.json
commit
fails "8b tree baseline above the base's" "is above the base's" erosion "$base"
violate
rm crates/erosion-baseline.json
commit
fails "8c tree baseline deleted" "is missing, but the base commit has one" erosion "$base"
violate
fails "8d --base with no value" "usage: cargo xtask" \
    cargo xtask erosion --lint-bin "$gate_bin" --health-bin "$gate_bin" --base
violate
fails "8e --base with an option for a value" "usage: cargo xtask" \
    cargo xtask erosion --lint-bin "$gate_bin" --health-bin "$gate_bin" --base "--output=$scratch/injected"
violate
fails "8f --base names a tree" "expected commit type" erosion "HEAD^{tree}"

# 9. A file write outside the io crate.
violate
printf '\n/// Write a file.\npub fn save() {\n    let _ = std::fs::write("x", "y");\n}\n' >>crates/kernel/src/lib.rs
fails "9 clippy disallowed_methods" "disallowed" cargo clippy -p kernel -- -D warnings

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
allow_attributes = "deny"
dbg_macro = "deny"
todo = "deny"
unimplemented = "deny"
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
fails "13 cargo insta test --check" "snapshot" cargo insta test --check -p kernel --test-runner nextest

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

if ((failures > 0)); then
    echo "$failures of $((bases + checks)) checks failed"
    exit 1
fi
echo "the base passes $bases checks, and each of $checks violations fails its gate"
