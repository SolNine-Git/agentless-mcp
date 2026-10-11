#!/usr/bin/env bash
# Prove that every workspace gate fails on its own violation (P0d, D7).
# Copies the tree into a scratch git repository with no remote, checks that the
# base passes every gate, then applies one violation at a time and checks that
# the matching gate fails with the matching message.
set -euo pipefail

gate_bin=${1:?usage: scripts/gate-selftest.sh PATH-TO-PINNED-AGENTLESS-MCP}
root=$(git rev-parse --show-toplevel)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export CARGO_TARGET_DIR="$root/target/selftest"
failures=0

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

erosion() {
    cargo xtask erosion --lint-bin "$gate_bin" --health-bin "$gate_bin" --base "$base"
}

passes() {
    local name=$1
    shift
    if "$@" >/dev/null 2>&1; then
        echo "ok   base passes: $name"
    else
        echo "FAIL base fails: $name"
        failures=$((failures + 1))
    fi
}

# fails NAME PATTERN COMMAND...: the gate must exit non-zero and print PATTERN.
fails() {
    local name=$1 pattern=$2 output
    shift 2
    if output=$("$@" 2>&1); then
        echo "FAIL $name: the gate passed"
        failures=$((failures + 1))
    elif grep -q -- "$pattern" <<<"$output"; then
        echo "ok   $name"
    else
        echo "FAIL $name: the gate failed without \"$pattern\""
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

passes "cargo fmt" cargo fmt --all --check
passes "cargo clippy" cargo clippy --workspace --all-targets -- -D warnings
passes "tests and snapshots" cargo insta test --check --workspace --test-runner nextest
passes "cargo deny" cargo deny check
passes "cargo machete" cargo machete
passes "xtask layers" cargo xtask layers
passes "xtask suppressions" cargo xtask suppressions
passes "xtask lint-tables" cargo xtask lint-tables
passes "xtask public-api" cargo xtask public-api
passes "xtask erosion" erosion

# 1. A function above complexity 10.
violate
{
    echo 'pub fn branchy(x: u32) -> u32 {'
    for i in $(seq 1 12); do echo "    if x == $i { return $i; }"; done
    echo '    0'
    echo '}'
} >crates/kernel/src/branchy.rs
commit
fails "1 complexity_delta" complexity_delta erosion

# 2. A copied block of 40 or more tokens.
violate
sed -n '/fn from_str/,/^    }/p' crates/kernel/src/key.rs \
    | sed 's/fn from_str/fn parse_again/' >crates/kernel/src/copied.rs
commit
fails "2 clone_delta" clone_delta erosion

# 3. unwrap_or_default() on a parsed value in a boundary crate.
violate
mkdir -p crates/io/src
printf '[package]\nname = "io"\nversion.workspace = true\nedition.workspace = true\npublish.workspace = true\n\n[lints]\nworkspace = true\n' \
    >crates/io/Cargo.toml
printf 'pub fn port(text: &str) -> u32 {\n    text.parse().unwrap_or_default()\n}\n' >crates/io/src/lib.rs
commit
fails "3 contract_drift" contract_drift erosion

# 4. A file that does not parse.
violate
printf 'pub fn broken( {\n' >crates/kernel/src/broken.rs
commit
fails "4 not_checked" not_checked erosion

# 5. An expect attribute, and separately an inner allow attribute.
violate
printf '\n#[expect(clippy::too_many_lines)]\nfn long() {}\n' >>crates/kernel/src/error.rs
fails "5a expect attribute" "clippy::too_many_lines" cargo xtask suppressions
violate
printf '#![allow(dead_code)]\n' >crates/kernel/src/inner.rs
fails "5b inner allow attribute" "dead_code" cargo xtask suppressions

# 6. An upward crate edge: kernel (layer 0) depends on prompts (layer 1).
violate
mkdir -p crates/prompts/src
printf '[package]\nname = "prompts"\nversion.workspace = true\nedition.workspace = true\npublish.workspace = true\n\n[lints]\nworkspace = true\n' \
    >crates/prompts/Cargo.toml
: >crates/prompts/src/lib.rs
printf '\n[dependencies.prompts]\npath = "../prompts"\n' >>crates/kernel/Cargo.toml
fails "6 xtask layers" "depends on prompts" cargo xtask layers

# 7. A new public function in kernel.
violate
printf '\n/// Return one.\n#[must_use]\npub const fn one() -> u32 {\n    1\n}\n' >>crates/kernel/src/lib.rs
fails "7 xtask public-api" "public-api.txt" cargo xtask public-api

# 8. A health figure above the baseline.
violate
printf '{\n  "erosion": 0.0,\n  "over_threshold": 0,\n  "clone_share": 0.0\n}\n' >crates/erosion-baseline.json
commit
fails "8 xtask erosion ratchet" "clone share rose" erosion

# 9. A file write outside the io crate.
violate
printf '\n/// Write a file.\npub fn save() {\n    let _ = std::fs::write("x", "y");\n}\n' >>crates/kernel/src/lib.rs
fails "9 clippy disallowed_methods" "disallowed" cargo clippy -p kernel -- -D warnings

# 10. unsafe outside syntax's ffi.rs, and separately a syntax lint table that drifts.
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
undocumented_unsafe_blocks = "deny"
EOF
}
violate
syntax_crate
printf 'pub fn f() {\n    unsafe {}\n}\n' >crates/syntax/src/lib.rs
fails "10a unsafe outside ffi.rs" "ffi.rs only" cargo xtask lint-tables
violate
syntax_crate
: >crates/syntax/src/lib.rs
sed -i.bak '/allow_attributes/d' crates/syntax/Cargo.toml && rm crates/syntax/Cargo.toml.bak
fails "10b syntax lint table drift" "crates/syntax/Cargo.toml" cargo xtask lint-tables

# 11. A license outside the allow-list, and separately a git source.
violate
printf '\n[dependencies.option-ext]\nversion = "0.2"\n' >>crates/kernel/Cargo.toml
fails "11a cargo deny licenses" "MPL-2.0" cargo deny check licenses
violate
printf '\n[dependencies.itoa]\ngit = "https://github.com/dtolnay/itoa"\n' >>crates/kernel/Cargo.toml
fails "11b cargo deny sources" "git" cargo deny check sources

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

if ((failures > 0)); then
    echo "$failures gate check(s) failed"
    exit 1
fi
echo "every gate passes on the base and fails on its violation"
