//! The parsers of the write subcommands: `patch` with its four steps, `validate` and `vote`.

use app::ops::{JSON, REPO};
use app::{DEFAULT_JOBS, DEFAULT_REPEAT_BASELINE, DEFAULT_TIMEOUT_SECONDS};
use io::{KILL_REAP, TERM_GRACE};

use super::{built, converted, flag, option, subcommand};
use crate::args::{Action, Arg, Command, Kind, Parser, Value, action, help_action, int_value};

const FILE_HELP: &str =
    "read the patch from FILE (SEARCH/REPLACE text or edits.json); default: stdin";
const IN_PLACE_HELP: &str =
    "write to the checkout instead of a scratch worktree; requires a clean tree";
const CANDIDATES_HELP: &str =
    "directory of candidate patches, one file each (edits.json or SEARCH/REPLACE)";
const TEST_CMD_HELP: &str = "the regression command; must pass on unpatched HEAD or the run is \
    UNVERIFIED. Falls back to test_cmd in the repository's .agentless-mcp.json, if it has one \
    and --allow-repo-test-cmd was given";
const ALLOW_REPO_TEST_CMD_HELP: &str = "allow the test command to come from the analysed \
    repository's .agentless-mcp.json; without this the fallback is refused, because the \
    repository would be choosing the command that judges it";
const ALLOW_TEST_CONFIG_EDITS_HELP: &str = "allow a candidate patch to edit conftest.py, a build \
    file or a CI workflow; without this such a candidate is refused before it is applied, \
    because it would be choosing how it is judged";
const PASS_ENV_HELP: &str = "pass one additional parent environment variable to test commands; \
    repeatable. By default a test command inherits only a short per-platform allowlist: PATH, \
    HOME, LANG and TMPDIR on POSIX, and the names a Windows interpreter needs to start on Windows";
const REPRO_CMD_HELP: &str = "the reproduction command; must FAIL on unpatched HEAD to count";
const RUN_TIMEOUT_HELP: &str = "bound the whole run, not one command (default: unbounded). A \
    batch is at most repeat_baseline + 1 + candidates x 2 commands; exact-result groups and \
    regression failures reduce it; candidates the deadline never reached are reported \
    not_evaluated";
const OUTPUT_HELP: &str = "write the verdicts document here instead of stdout";
const VERDICTS_HELP: &str = "a validate output file";
const ENVIRONMENT_NAME_REFUSAL: &str =
    r"environment variable names must match '[A-Za-z_][A-Za-z0-9_]*\\Z'";

// The steps of `patch`, in the reference order.
static PATCH_COMMANDS: [Command; 4] = [
    built("parse", "turn SEARCH/REPLACE text into edits.json", || {
        patch_step("parse", Vec::new())
    }),
    built("check", "apply in memory and report syntax deltas", || {
        patch_step("check", repo_and_json())
    }),
    built(
        "apply",
        "apply in a worktree and emit the diff",
        patch_apply,
    ),
    built("normalize", "AST-equivalence key for the change", || {
        patch_step("normalize", repo_and_json())
    }),
];

pub(super) fn patch() -> Parser {
    let steps = Action {
        strings: &[],
        dest: "patch_command",
        metavar: None,
        help: None,
        kind: Kind::Commands(&PATCH_COMMANDS),
        required: true,
    };
    subcommand("patch", vec![help_action(), steps])
}

fn patch_apply() -> Parser {
    let mut actions = repo_and_json();
    actions.push(flag(&["--in-place"], "in_place", IN_PLACE_HELP));
    patch_step("apply", actions)
}

// A step's parser: the help, the patch input, then the step's own options.
fn patch_step(name: &str, own: Vec<Action>) -> Parser {
    let file = option(&["-f", "--file"], "file", Some("FILE"), FILE_HELP);
    let mut actions = vec![help_action(), file];
    actions.extend(own);
    subcommand(&format!("patch {name}"), actions)
}

fn repo_and_json() -> Vec<Action> {
    [&REPO, &JSON].into_iter().filter_map(action).collect()
}

pub(super) fn validate() -> Parser {
    let candidates = option(
        &["--candidates"],
        "candidates",
        Some("DIR"),
        CANDIDATES_HELP,
    );
    let mut actions = vec![help_action(), required(candidates)];
    actions.extend(action(&REPO));
    actions.extend([
        option(&["--test-cmd"], "test_cmd", Some("CMD"), TEST_CMD_HELP),
        flag(
            &["--allow-repo-test-cmd"],
            "allow_repo_test_cmd",
            ALLOW_REPO_TEST_CMD_HELP,
        ),
        flag(
            &["--allow-test-config-edits"],
            "allow_test_config_edits",
            ALLOW_TEST_CONFIG_EDITS_HELP,
        ),
        pass_env(),
        option(&["--repro-cmd"], "repro_cmd", Some("CMD"), REPRO_CMD_HELP),
        integer(&["--timeout"], "timeout", None, &timeout_help()),
        integer(
            &["--jobs"],
            "jobs",
            None,
            &format!("distinct candidate results to run concurrently (default: {DEFAULT_JOBS})"),
        ),
        integer(
            &["--run-timeout"],
            "run_timeout",
            Some("SECONDS"),
            RUN_TIMEOUT_HELP,
        ),
        integer(
            &["--repeat-baseline"],
            "repeat_baseline",
            Some("N"),
            &format!(
                "run the baseline N times before any candidate (default: \
                 {DEFAULT_REPEAT_BASELINE}); runs that disagree make the whole validation \
                 UNVERIFIED"
            ),
        ),
        option(&["-o", "--output"], "output", Some("FILE"), OUTPUT_HELP),
    ]);
    subcommand("validate", actions)
}

pub(super) fn vote() -> Parser {
    let verdicts = option(&["--verdicts"], "verdicts", Some("FILE"), VERDICTS_HELP);
    let mut actions = vec![help_action(), required(verdicts)];
    actions.extend(action(&JSON));
    subcommand("vote", actions)
}

fn required(action: Action) -> Action {
    Action {
        required: true,
        ..action
    }
}

fn pass_env() -> Action {
    Action {
        kind: Kind::Value {
            convert: environment_name,
            choices: &[],
            repeat: true,
        },
        ..option(&["--pass-env"], "pass_env", Some("NAME"), PASS_ENV_HELP)
    }
}

fn integer(
    strings: &'static [&'static str],
    dest: &'static str,
    metavar: Option<&'static str>,
    help: &str,
) -> Action {
    Action {
        kind: converted(int_value),
        ..option(strings, dest, metavar, help)
    }
}

fn timeout_help() -> String {
    let cleanup = json::general((TERM_GRACE + KILL_REAP).as_secs_f64());
    format!(
        "per-command hard bound in seconds (default: {DEFAULT_TIMEOUT_SECONDS}); a command that \
         hits it is a failure, never a pass; killing a stubborn command adds at most {cleanup}s \
         of cleanup"
    )
}

// The `--pass-env` value: an ASCII letter or `_`, then ASCII letters, digits and `_`.
fn environment_name(arg: &Arg) -> Result<Value, String> {
    let mut chars = arg.text.chars();
    let starts = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if starts && chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Ok(Value::Text(arg.clone()))
    } else {
        Err(ENVIRONMENT_NAME_REFUSAL.to_owned())
    }
}
