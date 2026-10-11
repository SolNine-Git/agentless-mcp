use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use serde::Deserialize;

use crate::workspace::BUILD_SCRIPT;
use crate::{Failure, process};

// A hang guard: far above a cold run, on a slower runner with a cold registry too.
const CLIPPY: Duration = Duration::from_secs(60);
const DEAD_CODE: &str = "dead_code";

/// Whether a run compiles each target with the test harness.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Test,
}

/// The two cargo selections: together they build the units of `--all-targets`, and each unit in one mode.
pub const SELECTIONS: [(Mode, &[&str]); 2] = [
    (Mode::Normal, &["--lib", "--bins", "--examples"]),
    (Mode::Test, &["--tests", "--benches"]),
];

/// A force-warned diagnostic: the crate root of its target, its lint, where it points, and its text.
pub struct Diagnostic {
    pub target: PathBuf,
    pub lint: String,
    pub place: String,
    pub message: String,
}

/// A unit that cargo built: the crate root of its target and its dep-info file.
pub struct Unit {
    pub target: PathBuf,
    pub dep_info: PathBuf,
}

/// What clippy reports for the workspace packages: force-warned diagnostics, units, and compile errors.
#[derive(Default)]
pub struct Run {
    pub diagnostics: Vec<Diagnostic>,
    pub units: Vec<Unit>,
    pub errors: Vec<String>,
}

#[derive(Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
enum Record {
    CompilerMessage {
        manifest_path: PathBuf,
        target: Target,
        message: Rustc,
    },
    CompilerArtifact {
        manifest_path: PathBuf,
        target: Target,
        filenames: Vec<PathBuf>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct Target {
    name: String,
    kind: Vec<String>,
    src_path: PathBuf,
}

#[derive(Deserialize)]
struct Rustc {
    level: String,
    code: Option<Code>,
    message: String,
    spans: Vec<Span>,
    rendered: String,
}

#[derive(Deserialize)]
struct Code {
    code: String,
}

#[derive(Deserialize)]
struct Span {
    file_name: String,
    line_start: usize,
    is_primary: bool,
}

/// Run cargo clippy over one selection, with each lint in `forced` force-warned, under its deadline.
pub fn clippy(
    root: &Path,
    target_dir: &Path,
    forced: &[String],
    selection: &[&str],
) -> Result<Output, Failure> {
    let mut command = Command::new("cargo");
    command
        .args([
            "clippy",
            "--workspace",
            "--message-format=json",
            "--target-dir",
        ])
        .arg(target_dir)
        .args(selection)
        .arg("--")
        .args(forced.iter().map(|lint| format!("--force-warn={lint}")))
        .current_dir(root);
    process::run(&mut command, CLIPPY)
}

/// Read one run's JSON messages for the packages in `manifests`. A failed run with no compile error is broken.
pub fn parse(output: &Output, mode: Mode, manifests: &[PathBuf]) -> Result<Run, Failure> {
    let run = collect(records(&output.stdout)?, mode, manifests)?;
    if output.status.success() || !run.errors.is_empty() {
        Ok(run)
    } else {
        Err(Failure::Broken(format!(
            "cargo clippy exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

fn records(stdout: &[u8]) -> Result<Vec<Record>, Failure> {
    stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_slice(line).map_err(|error| {
                Failure::Broken(format!(
                    "cargo clippy printed a line that is not a cargo message ({error}): {}",
                    String::from_utf8_lossy(line)
                ))
            })
        })
        .collect()
}

fn collect(records: Vec<Record>, mode: Mode, manifests: &[PathBuf]) -> Result<Run, Failure> {
    let mut run = Run::default();
    for record in records {
        match record {
            Record::CompilerMessage {
                manifest_path,
                target,
                message,
            } if manifests.contains(&manifest_path) => {
                if message.level == "error" {
                    run.errors.push(message.rendered);
                } else {
                    run.diagnostics.extend(diagnostic(&target, message, mode));
                }
            }
            Record::CompilerArtifact {
                manifest_path,
                target,
                filenames,
            } if manifests.contains(&manifest_path) => {
                for output in &filenames {
                    run.units.push(unit(&target, output)?);
                }
            }
            _ => {}
        }
    }
    Ok(run)
}

// The test harness allows dead_code on a bin's main, and --force-warn overrides that allow too, so each
// item that only main reaches would be reported.
fn diagnostic(target: &Target, message: Rustc, mode: Mode) -> Option<Diagnostic> {
    let lint = message.code?.code;
    let harness =
        mode == Mode::Test && lint == DEAD_CODE && target.kind.iter().any(|kind| kind == "bin");
    (message.level == "warning" && !harness).then(|| Diagnostic {
        target: target.src_path.clone(),
        lint,
        place: place(&message.spans),
        message: message.message,
    })
}

fn place(spans: &[Span]) -> String {
    spans.iter().find(|span| span.is_primary).map_or_else(
        || "(no source span)".to_owned(),
        |span| format!("{}:{}", span.file_name, span.line_start),
    )
}

fn unit(target: &Target, output: &Path) -> Result<Unit, Failure> {
    dep_info_path(target, output)
        .map(|dep_info| Unit {
            target: target.src_path.clone(),
            dep_info,
        })
        .ok_or_else(|| {
            Failure::Broken(format!(
                "cargo built {}, and its dep-info file cannot be named from it",
                output.display()
            ))
        })
}

// rustc names a unit's outputs <crate>-<hash>, with lib before an .rmeta or a library. cargo reports a build
// script as build-script-build in a <package>-<hash> directory. The dep-info is <crate>-<hash>.d.
fn dep_info_path(target: &Target, output: &Path) -> Option<PathBuf> {
    let build_script = target.kind.iter().any(|kind| kind == BUILD_SCRIPT);
    let hashed = if build_script {
        output.parent()?.file_name()?
    } else {
        output.file_name()?
    };
    let stem = hashed.to_str()?.split('.').next()?;
    let (_, hash) = stem.rsplit_once('-')?;
    let crate_name = target.name.replace('-', "_");
    Some(output.with_file_name(format!("{crate_name}-{hash}.d")))
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::path::{Path, PathBuf};
    use std::process::{ExitStatus, Output};

    use super::{Mode, Run, Target, clippy, dep_info_path, parse};
    use crate::Failure;
    use crate::repo::fixture::scratch;

    const WORKSPACE: &str = "/r/xtask/Cargo.toml";

    fn target(name: &str, kind: &str) -> Target {
        Target {
            name: name.to_owned(),
            kind: vec![kind.to_owned()],
            src_path: PathBuf::from("/r/src/lib.rs"),
        }
    }

    fn message(manifest: &str, kind: &str, level: &str, code: &str) -> String {
        format!(
            r#"{{"reason":"compiler-message","manifest_path":"{manifest}","target":{{"name":"xtask","kind":["{kind}"],"src_path":"/r/xtask/src/main.rs"}},"message":{{"level":"{level}","code":{code},"message":"function `f` is never used","spans":[{{"file_name":"xtask/src/repo.rs","line_start":9,"is_primary":true}}],"rendered":"{level}: f"}}}}"#
        )
    }

    fn artifact(filename: &str) -> String {
        format!(
            r#"{{"reason":"compiler-artifact","manifest_path":"{WORKSPACE}","target":{{"name":"xtask","kind":["bin"],"src_path":"/r/xtask/src/main.rs"}},"filenames":["{filename}"],"fresh":true}}"#
        )
    }

    fn output(lines: &[String], code: i32) -> Output {
        Output {
            status: ExitStatus::from_raw(code << 8),
            stdout: lines.join("\n").into_bytes(),
            stderr: b"error: could not compile".to_vec(),
        }
    }

    fn run(lines: &[String], mode: Mode) -> Run {
        parse(&output(lines, 0), mode, &[PathBuf::from(WORKSPACE)]).unwrap()
    }

    #[test]
    fn a_lint_warning_in_a_workspace_package_is_a_diagnostic_with_its_place() {
        let dead = r#"{"code":"dead_code","explanation":null}"#;
        let lines = [
            message(WORKSPACE, "lib", "warning", dead),
            message(
                "/home/u/.cargo/registry/serde/Cargo.toml",
                "lib",
                "warning",
                dead,
            ),
            message(WORKSPACE, "lib", "warning", "null"),
            r#"{"reason":"build-finished","success":true}"#.to_owned(),
        ];
        let run = run(&lines, Mode::Normal);
        assert_eq!(run.diagnostics.len(), 1);
        let found = &run.diagnostics[0];
        assert_eq!(
            (found.lint.as_str(), found.place.as_str()),
            ("dead_code", "xtask/src/repo.rs:9")
        );
        assert_eq!(found.target, Path::new("/r/xtask/src/main.rs"));
    }

    #[test]
    fn dead_code_of_a_bin_is_left_out_only_in_the_test_harness_run() {
        let dead = r#"{"code":"dead_code","explanation":null}"#;
        let other = r#"{"code":"clippy::ptr_arg","explanation":null}"#;
        let lines = [
            message(WORKSPACE, "bin", "warning", dead),
            message(WORKSPACE, "lib", "warning", dead),
            message(WORKSPACE, "bin", "warning", other),
        ];
        let codes = |mode| -> Vec<String> {
            run(&lines, mode)
                .diagnostics
                .into_iter()
                .map(|found| found.lint)
                .collect()
        };
        assert_eq!(
            codes(Mode::Normal),
            ["dead_code", "dead_code", "clippy::ptr_arg"]
        );
        assert_eq!(codes(Mode::Test), ["dead_code", "clippy::ptr_arg"]);
    }

    #[test]
    fn a_compile_error_is_kept_and_a_failure_without_one_is_broken() {
        let failed = output(&[message(WORKSPACE, "lib", "error", "null")], 101);
        let run = parse(&failed, Mode::Normal, &[PathBuf::from(WORKSPACE)]).unwrap();
        assert_eq!(run.errors, ["error: f"]);
        let silent = output(&[], 101);
        assert!(matches!(
            parse(&silent, Mode::Normal, &[PathBuf::from(WORKSPACE)]),
            Err(Failure::Broken(message)) if message.contains("could not compile")
        ));
    }

    #[test]
    fn a_line_that_is_not_a_cargo_message_is_broken() {
        let lines = ["Checking kernel".to_owned()];
        let outcome = parse(&output(&lines, 0), Mode::Normal, &[]);
        assert!(
            matches!(outcome, Err(Failure::Broken(message)) if message.contains("Checking kernel"))
        );
    }

    #[test]
    fn each_artifact_names_its_dep_info_file() {
        let run = run(
            &[artifact(
                "/r/target/debug/deps/libxtask-b0f036fad7d412e7.rmeta",
            )],
            Mode::Normal,
        );
        assert_eq!(
            run.units[0].dep_info,
            Path::new("/r/target/debug/deps/xtask-b0f036fad7d412e7.d")
        );
        for (name, kind, output, dep_info) in [
            (
                "my-crate",
                "lib",
                "/t/deps/libmy_crate-0a1b.rlib",
                "/t/deps/my_crate-0a1b.d",
            ),
            (
                "build-script-build",
                "custom-build",
                "/t/debug/build/syntax-76f54d1fb342e5ad/build-script-build",
                "/t/debug/build/syntax-76f54d1fb342e5ad/build_script_build-76f54d1fb342e5ad.d",
            ),
        ] {
            assert_eq!(
                dep_info_path(&target(name, kind), Path::new(output)).unwrap(),
                Path::new(dep_info)
            );
        }
        let library = target("x", "lib");
        assert!(dep_info_path(&library, Path::new("/t/deps/nohash.rmeta")).is_none());
    }

    #[test]
    fn the_dep_info_file_named_for_a_real_build_script_exists() {
        let scratch = scratch("build-script");
        std::fs::create_dir_all(scratch.join("src")).unwrap();
        let manifest =
            "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n";
        for (file, text) in [
            ("Cargo.toml", manifest),
            ("build.rs", "fn main() {}\n"),
            ("src/lib.rs", ""),
        ] {
            std::fs::write(scratch.join(file), text).unwrap();
        }
        // cargo reports paths under the canonical directory: on macOS the temp dir is under a symlink.
        let root = std::fs::canonicalize(&scratch).unwrap();
        let built = clippy(&root, &root.join("target"), &[], &["--lib"]);
        let run = parse(&built.unwrap(), Mode::Normal, &[root.join("Cargo.toml")]).unwrap();
        let named: Vec<(PathBuf, bool)> = run
            .units
            .iter()
            .filter(|unit| unit.target == root.join("build.rs"))
            .map(|unit| (unit.dep_info.clone(), unit.dep_info.is_file()))
            .collect();
        std::fs::remove_dir_all(&scratch).unwrap();
        assert!(!named.is_empty(), "cargo reported no build script unit");
        assert!(named.iter().all(|(_, exists)| *exists), "{named:?}");
    }
}
