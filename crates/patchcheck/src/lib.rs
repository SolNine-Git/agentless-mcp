//! The seven patch checks over a candidate's changed files: undeclared imports, shadowing, near
//! duplicates, dangling references, dangling callers, arity, and the cycle delta.

mod arity;
mod callers;
mod cycles;
mod dangling;
mod duplicates;
mod fragment;
mod interpreter;
mod lexer;
mod manifest;
mod repo;
mod shadowing;
mod undeclared;

#[cfg(test)]
mod tests;

use std::cmp::Ordering;
use std::sync::Arc;

use checks::{Finding, Severity};
use patch::Edit;
use serde::Serialize;
use syntax::{Language, Parsed, Unparsable};

use crate::fragment::{Fragment, PatchNames, fragments};
pub use manifest::{
    DeclaredDependencies, ManifestParse, PYPROJECT_NAME, REQUIREMENTS_GLOB,
    parse_pyproject_dependencies, parse_requirements,
};
pub use repo::PatchBaseline;

/// The check that reports packages the patch imports and nothing declares.
pub const CHECK_UNDECLARED_IMPORTS: &str = "undeclared_imports";
/// The check that reports module-level definitions over a name the file already defines.
pub const CHECK_SHADOWING: &str = "shadowing";
/// The check that reports introduced bodies that the repository already has.
pub const CHECK_NEAR_DUPLICATES: &str = "near_duplicates";
/// The check that reports names the patch uses that nothing defines.
pub const CHECK_DANGLING_REFERENCES: &str = "dangling_references";
/// The check that reports removed symbols that untouched files still reference.
pub const CHECK_DANGLING_CALLERS: &str = "dangling_callers";
/// The check that reports calls whose argument count cannot fit the callee.
pub const CHECK_ARITY: &str = "arity";
/// The check that reports import cycles the patch introduces.
pub const CHECK_CYCLE_DELTA: &str = "cycle_delta";
/// The name of the gaps that are no one check's: an edit no grammar reads, or the tables'
/// interpreter.
pub const CHECK_COVERAGE: &str = "coverage";

// How many existing sites a finding lists before it counts the rest.
const MAX_DUPLICATE_SITES: usize = 3;

/// Every finding of one patch in (check, path, line, message, evidence) order, and the warnings
/// of the repository read that the findings depend on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PatchLint {
    pub findings: Vec<Finding>,
    pub warnings: Vec<String>,
}

type Parse = dyn Fn(Language, Arc<str>) -> Result<Parsed, Unparsable>;

/// Run the seven checks over one patch's edits against the pre-patch repository.
#[must_use]
pub fn lint_patch(edits: &[Edit], facts: &PatchBaseline<'_>) -> PatchLint {
    lint_with(edits, facts, &syntax::parse)
}

fn lint_with(edits: &[Edit], facts: &PatchBaseline<'_>, parse: &Parse) -> PatchLint {
    let (fragments, mut findings) = fragments(edits, facts, parse);
    let names = PatchNames::of(&fragments);
    findings.extend(interpreter_gaps(&fragments, facts));
    findings.extend(undeclared::check(&fragments, facts));
    findings.extend(shadowing::check(&fragments, facts, &names));
    findings.extend(guarded(
        CHECK_NEAR_DUPLICATES,
        duplicates::check(&fragments, facts),
    ));
    findings.extend(dangling::check(&fragments, facts, &names));
    findings.extend(callers::check(&fragments, facts, &names));
    findings.extend(arity::check(&fragments, facts));
    findings.extend(guarded(
        CHECK_CYCLE_DELTA,
        cycles::check(&fragments, facts, parse),
    ));
    findings.sort_by(order);
    PatchLint {
        findings,
        warnings: facts.dependencies().warnings.clone(),
    }
}

fn order(left: &Finding, right: &Finding) -> Ordering {
    order_key(left).cmp(&order_key(right))
}

fn order_key(finding: &Finding) -> (&str, &str, u32, &str, &str) {
    (
        finding.check,
        &finding.path,
        finding.line,
        &finding.message,
        &finding.evidence,
    )
}

// A check that a parse refused becomes one gap, which names the refusal's exception class.
fn guarded(check: &'static str, run: Result<Vec<Finding>, Unparsable>) -> Vec<Finding> {
    run.unwrap_or_else(|error| {
        vec![gap(
            check,
            "",
            format!(
                "not checked: the check could not run over this patch ({})",
                exception_class(&error)
            ),
            error.to_string(),
        )]
    })
}

// The class that the reference names for the same refusal: a grammar that does not load is
// LanguageUnavailable, and every other refusal of a parse is a ValueError.
fn exception_class(error: &Unparsable) -> &'static str {
    match error {
        Unparsable::Abi { .. } => "LanguageUnavailable",
        Unparsable::NoTree { .. } | Unparsable::TooLarge { .. } | Unparsable::Boundary { .. } => {
            "ValueError"
        }
    }
}

// A statement about coverage, not about the patch.
fn gap(check: &'static str, path: &str, message: String, evidence: String) -> Finding {
    Finding {
        check,
        severity: Severity::NotChecked,
        message,
        path: path.to_owned(),
        line: 0,
        evidence,
    }
}

// The tables describe Python 3.13, which the repository's declared floor may not be: the
// disagreement is stated once, for both checks that read them.
fn interpreter_gaps(fragments: &[Fragment<'_>], facts: &PatchBaseline<'_>) -> Option<Finding> {
    if !fragments
        .iter()
        .any(|fragment| fragment.language == Language::Python)
    {
        return None;
    }
    let declared = &facts.dependencies().requires_python;
    let floor = manifest::python_floor(declared)?;
    if floor == interpreter::VERSION {
        return None;
    }
    let running = interpreter::version_text();
    Some(gap(
        CHECK_COVERAGE,
        "",
        format!(
            "partially checked: undeclared_imports and dangling_references read \
             Python {running}'s builtin and standard-library tables, \
             and this repository requires {declared}"
        ),
        format!("requires-python {declared}; tables read from Python {running}"),
    ))
}

// The tail that counts the items past the listed ones.
fn more(count: usize, listed: usize) -> String {
    if count <= listed {
        String::new()
    } else {
        format!(" and {} more", count - listed)
    }
}

// Python's repr of a str, which the messages quote names with.
fn quoted(name: &str) -> String {
    text::CodePointString::from(name).quoted()
}
