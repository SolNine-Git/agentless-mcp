use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::sources::Source;
use crate::tool_config::CLIPPY_NAMES;
use crate::workspace::Package;
use crate::{Failure, repo};

const MANIFEST: &str = "Cargo.toml";

/// The files that rustc may read for a workspace target: the scanned sources, the manifests, and clippy configs.
pub struct Allowed {
    root: PathBuf,
    files: BTreeSet<PathBuf>,
}

impl Allowed {
    /// Allow each scanned source, the root manifest and each package manifest.
    pub fn new(root: &Path, sources: &[Source], packages: &[Package]) -> Self {
        let sources = sources.iter().map(|source| root.join(&source.file));
        let manifests = packages.iter().map(|package| package.dir.join(MANIFEST));
        Self {
            root: root.to_path_buf(),
            files: sources
                .chain(manifests)
                .chain([root.join(MANIFEST)])
                .collect(),
        }
    }

    // clippy-driver lists the clippy config that it read. The tool-config gate decides which may exist.
    fn allows(&self, path: &Path) -> bool {
        let clippy = path.starts_with(&self.root)
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| CLIPPY_NAMES.contains(&name));
        clippy || self.files.contains(path)
    }
}

/// Return each input that rustc's dep-info `text` lists, or why the text is not in rustc's format.
pub fn inputs(text: &str) -> Result<Vec<String>, String> {
    // rustc writes `<output>: <inputs>` and a blank line per output, then `<input>:` per input, then an
    // optional blank line and `# ` comments. It escapes only spaces, so a line feed in a name splits a rule.
    let body = text
        .strip_suffix('\n')
        .ok_or_else(|| "the text does not end with a line feed".to_owned())?;
    let mut lines = body.split('\n').peekable();
    let mut listed: Option<Vec<String>> = None;
    while let Some(line) = lines.next_if(|line| line.contains(": ")) {
        let inputs = output_rule(line)?;
        if lines.next() != Some("") {
            return Err(format!("no blank line follows the rule `{line}`"));
        }
        if listed.as_ref().is_some_and(|first| *first != inputs) {
            return Err(format!(
                "the rule `{line}` lists other inputs than the first rule"
            ));
        }
        listed = Some(inputs);
    }
    let listed =
        listed.ok_or_else(|| "the text starts with no rule `<output>: <inputs>`".to_owned())?;
    let mut phony = Vec::new();
    while let Some(line) = lines.next_if(|line| !line.is_empty()) {
        phony.push(input_rule(line)?);
    }
    if phony != listed {
        return Err("the `<input>:` rules do not list the inputs of the output rules".to_owned());
    }
    match lines.next() {
        Some(_) => comments(lines).map(|()| listed),
        None => Ok(listed),
    }
}

/// Return each path in `inputs` resolved against the root and canonicalized, with every symlink followed.
pub fn canonical(root: &Path, inputs: &[String]) -> Result<BTreeSet<PathBuf>, Failure> {
    inputs
        .iter()
        .map(|input| {
            let path = root.join(input);
            std::fs::canonicalize(&path).map_err(|error| repo::broken(&path, &error))
        })
        .collect()
}

/// Return a problem for each path that rustc reads for `target` and that `allowed` does not hold.
pub fn problems(
    root: &Path,
    target: &Path,
    paths: &BTreeSet<PathBuf>,
    allowed: &Allowed,
) -> Vec<String> {
    paths
        .iter()
        .filter(|path| !allowed.allows(path))
        .map(|path| {
            format!(
                "{}: rustc reads {}, which is not a .rs file that the gates scan: an include!, include_str!, include_bytes! or #[path] names it, directly or through a macro. Move the code into a module under crates/ or xtask/, or read the data at run time",
                repo::relative(target, root),
                repo::relative(path, root)
            )
        })
        .collect()
}

fn output_rule(line: &str) -> Result<Vec<String>, String> {
    let (output, inputs) = line
        .split_once(": ")
        .ok_or_else(|| format!("the line `{line}` is not a rule `<output>: <inputs>`"))?;
    let inputs = words(inputs);
    if output.is_empty() || inputs.iter().any(String::is_empty) {
        return Err(format!("the rule `{line}` has an empty path"));
    }
    Ok(inputs)
}

fn input_rule(line: &str) -> Result<String, String> {
    match line.strip_suffix(':').map(words).as_deref() {
        Some([path]) if !path.is_empty() => Ok(path.clone()),
        _ => Err(format!("the line `{line}` is not a rule `<input>:`")),
    }
}

fn comments<'a>(mut lines: impl Iterator<Item = &'a str>) -> Result<(), String> {
    match lines.find(|line| !line.starts_with("# ")) {
        Some(line) => Err(format!(
            "the line `{line}` after the rules is not a `# ` comment"
        )),
        None => Ok(()),
    }
}

// rustc writes a space in a path as `\ ` and changes no other character. So a name that ends in a
// backslash joins the next input, and the `<input>:` check breaks.
fn words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' if characters.next_if_eq(&' ').is_some() => current.push(' '),
            ' ' => words.push(std::mem::take(&mut current)),
            other => current.push(other),
        }
    }
    words.push(current);
    words
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use super::{Allowed, canonical, inputs, problems};
    use crate::Failure;
    use crate::repo::fixture::scratch;
    use crate::sources::Source;
    use crate::tokens;
    use crate::workspace::Package;

    const RULES: &str = "/r/xtask/clippy.toml xtask/src/main.rs \
xtask/src/../../crates/kernel/src/id.rs my\\ notes.md Cargo.toml";
    const DEP_INFO: &str =
        "/r/target/debug/deps/xtask-f9.d: /r/xtask/clippy.toml xtask/src/main.rs \
xtask/src/../../crates/kernel/src/id.rs my\\ notes.md Cargo.toml

/r/target/debug/deps/libxtask-f9.rmeta: /r/xtask/clippy.toml xtask/src/main.rs \
xtask/src/../../crates/kernel/src/id.rs my\\ notes.md Cargo.toml

/r/xtask/clippy.toml:
xtask/src/main.rs:
xtask/src/../../crates/kernel/src/id.rs:
my\\ notes.md:
Cargo.toml:

# env-dep:CARGO_MANIFEST_DIR=/r/xtask
# env-dep:CLIPPY_CONF_DIR
";

    fn allowed() -> Allowed {
        let sources: Vec<Source> = ["xtask/src/main.rs", "crates/kernel/src/id.rs"]
            .iter()
            .map(|file| Source {
                file: (*file).to_owned(),
                scan: tokens::scan("").unwrap(),
            })
            .collect();
        let packages = [Package {
            name: "xtask".to_owned(),
            dir: PathBuf::from("/r/xtask"),
            rust_version: None,
            dependencies: Vec::new(),
            targets: Vec::new(),
        }];
        Allowed::new(Path::new("/r"), &sources, &packages)
    }

    fn set(paths: &[&str]) -> BTreeSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    fn text_with(rules: &str) -> String {
        format!("/r/t/kernel-1.d: {RULES}\n\n{rules}")
    }

    #[test]
    fn the_inputs_are_read_in_order_with_escaped_spaces_unescaped() {
        assert_eq!(
            inputs(DEP_INFO).unwrap(),
            [
                "/r/xtask/clippy.toml",
                "xtask/src/main.rs",
                "xtask/src/../../crates/kernel/src/id.rs",
                "my notes.md",
                "Cargo.toml",
            ]
        );
    }

    #[test]
    fn a_line_feed_in_an_input_name_makes_the_text_unreadable() {
        // rustc escapes only spaces, so an input named "docs/a<LF>Cargo.toml" splits both of its rules.
        let text = "/r/t/kernel-1.d: crates/kernel/src/lib.rs crates/kernel/src/../../../docs/a\nCargo.toml\n\n\
crates/kernel/src/lib.rs:\ncrates/kernel/src/../../../docs/a\nCargo.toml:\n";
        assert!(inputs(text).is_err(), "{:?}", inputs(text));
    }

    #[test]
    fn a_text_outside_rustcs_format_is_an_error() {
        let phony = "/r/xtask/clippy.toml:\nxtask/src/main.rs:\nxtask/src/../../crates/kernel/src/id.rs:\n\
my\\ notes.md:\nCargo.toml:\n";
        assert!(inputs(&text_with(phony)).is_ok());
        let two_rules =
            format!("/r/t/kernel-1.d: {RULES}\n\n/r/t/libkernel-1.rmeta: Cargo.toml\n\n{phony}");
        for (text, error) in [
            (
                text_with(phony).trim_end().to_owned(),
                "does not end with a line feed",
            ),
            (phony.to_owned(), "starts with no rule"),
            (
                format!("/r/t/kernel-1.d: {RULES}\n{phony}"),
                "no blank line follows",
            ),
            (two_rules, "lists other inputs than the first rule"),
            (
                text_with(&phony.replace("Cargo.toml:\n", "")),
                "do not list the inputs",
            ),
            (
                text_with(&phony.replace("Cargo.toml:", "Cargo.toml")),
                "is not a rule `<input>:`",
            ),
            (
                text_with(&phony.replace("my\\ notes", "my notes")),
                "is not a rule `<input>:`",
            ),
            (
                text_with(&format!("{phony}\n# env-dep:X\nCargo.toml:\n")),
                "is not a `# ` comment",
            ),
            (
                "/r/t/kernel-1.d: a  b\n\na:\n:\nb:\n".to_owned(),
                "has an empty path",
            ),
        ] {
            let found = inputs(&text).err().unwrap_or_default();
            assert!(found.contains(error), "{text:?}: {found}");
        }
    }

    #[test]
    fn each_input_is_canonical_through_a_symlink_and_a_missing_one_is_broken() {
        let scratch = scratch("dep-info");
        for dir in [
            "evil/a/b",
            "evil/crates/kernel/src",
            "crates/kernel/src",
            "docs",
        ] {
            std::fs::create_dir_all(scratch.join(dir)).unwrap();
        }
        for file in [
            "evil/crates/kernel/src/limit.rs",
            "crates/kernel/src/limit.rs",
        ] {
            std::fs::write(scratch.join(file), "").unwrap();
        }
        std::os::unix::fs::symlink("../evil/a/b", scratch.join("docs/link")).unwrap();
        let root = std::fs::canonicalize(&scratch).unwrap();
        let read =
            ["crates/kernel/src/../../../docs/link/../../crates/kernel/src/limit.rs".to_owned()];
        let resolved = canonical(&root, &read);
        let missing = canonical(&root, &["crates/kernel/src/gone.rs".to_owned()]);
        std::fs::remove_dir_all(&scratch).unwrap();
        assert_eq!(
            resolved.unwrap(),
            BTreeSet::from([root.join("evil/crates/kernel/src/limit.rs")])
        );
        assert!(matches!(missing, Err(Failure::Broken(message)) if message.contains("gone.rs")));
    }

    #[test]
    fn only_scanned_sources_manifests_and_clippy_configs_under_the_root_are_allowed() {
        let read = set(&[
            "/r/xtask/src/main.rs",
            "/r/crates/kernel/src/id.rs",
            "/r/Cargo.toml",
            "/r/xtask/Cargo.toml",
            "/r/xtask/clippy.toml",
            "/r/.clippy.toml",
            "/r/crates/kernel/src/payload.txt",
            "/r/xtask/src/unlisted.rs",
            "/r/crates/kernel/Cargo.toml",
            "/home/u/clippy.toml",
            "/etc/hostname",
        ]);
        let problems = problems(
            Path::new("/r"),
            Path::new("/r/xtask/src/main.rs"),
            &read,
            &allowed(),
        );
        let named: Vec<&str> = problems
            .iter()
            .map(|problem| problem.split(", which").next().unwrap())
            .collect();
        assert_eq!(
            named,
            [
                "xtask/src/main.rs: rustc reads /etc/hostname",
                "xtask/src/main.rs: rustc reads /home/u/clippy.toml",
                "xtask/src/main.rs: rustc reads crates/kernel/Cargo.toml",
                "xtask/src/main.rs: rustc reads crates/kernel/src/payload.txt",
                "xtask/src/main.rs: rustc reads xtask/src/unlisted.rs",
            ]
        );
    }
}
