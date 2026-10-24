//! `arity`: a call in new code whose positional argument count cannot fit the one plain function
//! it resolves to on strong evidence. Every doubt is a silent pass.

use std::collections::BTreeSet;

use checks::{Finding, Severity};
use facts::SymbolKind;
use graph::{Definition, Tier};
use syntax::Language;

use crate::fragment::{Fragment, language_gaps, unscanned_gaps};
use crate::interpreter::always_bound;
use crate::lexer::{CallSite, Shape, parameter_shape, positional_arguments, structure};
use crate::repo::PatchBaseline;
use crate::{CHECK_ARITY, quoted};

pub(crate) fn check(fragments: &[Fragment<'_>], facts: &PatchBaseline<'_>) -> Vec<Finding> {
    let mut findings = language_gaps(CHECK_ARITY, fragments, "signature grammar");
    findings.extend(unscanned_gaps(CHECK_ARITY, fragments, facts));
    let python = fragments
        .iter()
        .filter(|fragment| fragment.language == Language::Python);
    for fragment in python {
        let introduced: BTreeSet<&str> = fragment
            .introduced
            .iter()
            .map(|symbol| symbol.name.as_str())
            .collect();
        // One scan of the block serves every call in it.
        let depths = structure(&fragment.replace_chars);
        let judged = fragment
            .calls
            .iter()
            .filter(|site| !introduced.contains(site.name.as_str()) && !always_bound(&site.name))
            .filter_map(|site| arity_finding(fragment, site, &depths, facts));
        findings.extend(judged);
    }
    findings
}

fn arity_finding(
    fragment: &Fragment<'_>,
    site: &CallSite,
    depths: &[i64],
    facts: &PatchBaseline<'_>,
) -> Option<Finding> {
    let given = positional_arguments(&fragment.replace_chars, depths, site.offset)?.len();
    let definition = plain_function(&site.name, fragment.path(), facts)?;
    let shape = parameter_shape(&definition.symbol().signature)?;
    if (shape.required..=shape.total).contains(&given) {
        return None;
    }
    Some(arity_report(fragment, site, &definition, shape, given))
}

// The one undecorated function that `name` resolves to at the same-file or imported tier.
fn plain_function<'a>(name: &str, path: &str, facts: &PatchBaseline<'a>) -> Option<Definition<'a>> {
    let resolution = facts.resolver().resolve(name, path)?;
    if !matches!(resolution.tier, Tier::SameFile | Tier::Imported) {
        return None;
    }
    let [definition] = <[Definition<'a>; 1]>::try_from(resolution.candidates).ok()?;
    let symbol = definition.symbol();
    (symbol.kind == SymbolKind::Function && !symbol.decorated).then_some(definition)
}

fn arity_report(
    fragment: &Fragment<'_>,
    site: &CallSite,
    definition: &Definition<'_>,
    shape: Shape,
    given: usize,
) -> Finding {
    let accepts = if shape.required == shape.total {
        shape.required.to_string()
    } else {
        format!("{} to {}", shape.required, shape.total)
    };
    let symbol = definition.symbol();
    Finding {
        check: CHECK_ARITY,
        severity: Severity::Advisory,
        message: format!(
            "the patch calls {} with {given} positional argument(s); \
             the definition it resolves to takes {accepts}",
            quoted(&site.name)
        ),
        path: fragment.path().to_owned(),
        line: fragment.file_line(site.line),
        evidence: format!("{}:{} {}", definition.path(), symbol.line, symbol.signature),
    }
}
