//! `cargo xtask generate`: writes crates/prompts/src/generated/ from the prompt JSON files, and
//! crates/syntax/src/generated/ from the language files.

mod literal;
mod placeholders;
mod rules;
mod syntax;
mod templates;

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use crate::{Failure, process, repo};

const ENVELOPE: &str = "src/agentless_mcp/prompts/envelope.json";
const MESSAGES: &str = "src/agentless_mcp/prompts/messages.json";
const RULES: &str = "src/agentless_mcp/prompts/contract_rules.json";
const OUTPUT: &str = "crates/prompts/src/generated";

type Files = Vec<(&'static str, String)>;

/// Writes each generated file of crates/prompts/src/generated/ and crates/syntax/src/generated/.
pub fn run(root: &Path) -> Result<(), Failure> {
    write_all(root, OUTPUT, fresh(root)?)?;
    write_all(root, syntax::OUTPUT, fresh_syntax(root)?)
}

fn write_all(root: &Path, dir: &str, files: Files) -> Result<(), Failure> {
    let output = root.join(dir);
    std::fs::create_dir_all(&output).map_err(|error| repo::broken(&output, &error))?;
    for (name, text) in files {
        repo::write(&output.join(name), &text)?;
    }
    Ok(())
}

// The digest covers the formatted tables, the bytes that the syntax crate compiles.
fn fresh_syntax(root: &Path) -> Result<Files, Failure> {
    let module = syntax::module(&syntax::read(root)?).map_err(Failure::Gate)?;
    let tables = formatted(root, vec![("languages.rs", module)])?;
    let digest = formatted(root, vec![("digest.rs", syntax::digest_module(&tables))])?;
    Ok([tables, digest].concat())
}

fn fresh(root: &Path) -> Result<Files, Failure> {
    let read = |file: &str| repo::read(&root.join(file));
    let sources =
        sources(&read(ENVELOPE)?, &read(MESSAGES)?, &read(RULES)?).map_err(Failure::Gate)?;
    formatted(root, sources)
}

fn sources(envelope: &str, messages: &str, rules: &str) -> Result<Files, Vec<String>> {
    let parsed = (
        templates::parse(ENVELOPE, envelope),
        templates::parse(MESSAGES, messages),
        rules::parse(RULES, rules),
    );
    let (envelope, messages, rules) = match parsed {
        (Ok(envelope), Ok(messages), Ok(rules)) => (envelope, messages, rules),
        (envelope, messages, rules) => {
            return Err([envelope.err(), messages.err(), rules.err()]
                .into_iter()
                .flatten()
                .flatten()
                .collect());
        }
    };
    let used: BTreeSet<&str> = envelope
        .iter()
        .chain(&messages)
        .flat_map(|template| template.values.iter().map(|(name, _)| *name))
        .collect();
    let stale = placeholders::stale(&used);
    if !stale.is_empty() {
        return Err(stale);
    }
    Ok(vec![
        (
            "envelope.rs",
            templates::module("The envelope templates.", &envelope),
        ),
        (
            "messages.rs",
            templates::module("The message templates.", &messages),
        ),
        ("rules.rs", rules::module(&rules)),
        (
            "catalog.rs",
            templates::catalog(&[("envelope", &envelope), ("messages", &messages)]),
        ),
    ])
}

// rustfmt formats files in place, so the sources go through a scratch directory.
fn formatted(root: &Path, sources: Files) -> Result<Files, Failure> {
    let dir = std::env::temp_dir().join(format!("xtask-generate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|error| repo::broken(&dir, &error))?;
    let files = format_in(root, &dir, sources);
    let removed = std::fs::remove_dir_all(&dir).map_err(|error| repo::broken(&dir, &error));
    let files = files?;
    removed?;
    Ok(files)
}

fn format_in(root: &Path, dir: &Path, sources: Files) -> Result<Files, Failure> {
    let mut paths = Vec::new();
    for (name, text) in &sources {
        let path = dir.join(name);
        repo::write(&path, text)?;
        paths.push(path);
    }
    // From the root, rustup picks the toolchain that rust-toolchain.toml pins.
    let mut command = Command::new("rustfmt");
    command
        .current_dir(root)
        .args(["--edition", "2024", "--config-path"])
        .arg(root.join("rustfmt.toml"))
        .args(&paths);
    process::stdout(&mut command, process::RUSTFMT)?;
    sources
        .into_iter()
        .zip(&paths)
        .map(|((name, _), path)| Ok((name, repo::read(path)?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::Value;

    use super::{
        ENVELOPE, Files, MESSAGES, OUTPUT, RULES, fresh, fresh_syntax, rules, sources, syntax,
        templates,
    };
    use crate::repo;

    fn read(file: &str) -> String {
        repo::read(&repo::root().unwrap().join(file)).unwrap()
    }

    fn assert_fresh(dir: &str, files: &Files) {
        let output = repo::root().unwrap().join(dir);
        for (name, text) in files {
            let committed = std::fs::read_to_string(output.join(name)).unwrap_or_default();
            assert!(
                committed == *text,
                "{dir}/{name} differs from a fresh generation: run cargo xtask generate"
            );
            let long = text.lines().position(|line| line.chars().count() > 100);
            assert_eq!(long, None, "{dir}/{name}: a line is over 100 characters");
        }
        let generated: BTreeSet<String> =
            files.iter().map(|(name, _)| (*name).to_owned()).collect();
        let present: BTreeSet<String> = std::fs::read_dir(&output)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(
            present, generated,
            "{dir} holds a file that the generator does not write"
        );
    }

    #[test]
    fn the_committed_files_are_a_fresh_generation() {
        let root = repo::root().unwrap();
        assert_fresh(OUTPUT, &fresh(&root).unwrap());
        assert_fresh(syntax::OUTPUT, &fresh_syntax(&root).unwrap());
    }

    #[test]
    fn the_prompt_files_parse_with_no_problem() {
        let envelope = templates::parse(ENVELOPE, &read(ENVELOPE)).unwrap();
        let messages = templates::parse(MESSAGES, &read(MESSAGES)).unwrap();
        assert_eq!((envelope.len(), messages.len()), (12, 116));
        let text = read(RULES);
        let entries: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(entries["rules"].as_array().map(Vec::len), Some(22));
        assert_eq!(rules::parse(RULES, &text).unwrap().len(), 30);
        assert!(sources(&read(ENVELOPE), &read(MESSAGES), &text).is_ok());
    }

    #[test]
    fn a_table_name_that_no_template_uses_is_a_problem() {
        let rules = read(RULES);
        let problems = sources("{\"a\": \"{path}\"}", "{}", &rules).unwrap_err();
        let stale =
            |problem: &String| problem.contains("placeholders.rs") && problem.contains("\"start\"");
        assert!(problems.iter().any(stale), "{problems:?}");
        assert!(!problems.iter().any(|problem| problem.contains("\"path\"")));
    }

    #[test]
    fn the_problems_of_every_file_come_together() {
        let problems = sources("[]", "{\"a\": 1}", "{}").unwrap_err();
        assert_eq!(problems.len(), 3, "{problems:?}");
        for file in [ENVELOPE, MESSAGES, RULES] {
            assert!(
                problems.iter().any(|problem| problem.starts_with(file)),
                "{file}: {problems:?}"
            );
        }
    }
}
