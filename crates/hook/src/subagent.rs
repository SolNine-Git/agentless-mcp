//! brief-record and subagent-map: the map sentence for a subagent's first prompt. `SubagentStart`
//! carries no brief, so brief-record keeps the seeds and the named files of each Agent call's
//! brief, and subagent-map pairs a starting subagent with the one pending record of its type.

use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use app::{IntArg, MapRequest, MapResult, Services};
use io::Granularity;
use json::{JsonValue, get};
use kernel::Clock;
use prompts::messages::SUBAGENT_MAP_CONTEXT;
use text::CodePointString;

use crate::ledger;
use crate::prompt_map::MapFn;
use crate::prompt_map::seeds::{self, MAP_FAILED, NO_PROMPT, NO_SEED, Seeded, TOO_LONG};
use crate::repository;
use crate::run::{self, Facts, LEDGER_STORAGE_UNAVAILABLE, NO_REPOSITORY, Outcome, Run};
use crate::search::text_field;
use crate::state;
use crate::text::sha256_hex;

// A fork inherits its parent's context, so its brief needs no map.
const FORK: &str = "fork";
// The type of an Agent call that names none.
const DEFAULT_TYPE: &str = "general-purpose";
// A record older than this pairs with no subagent.
const PENDING: Duration = Duration::from_secs(60);
// The ranked files that the map reads before it drops the files the brief names.
const RANKED_FILES: usize = 5;
// Each try takes the next nanosecond, for parallel calls of one type in one nanosecond.
const NAME_TRIES: u128 = 8;

const ROOT_KEY: &str = "root";
const SEEDS_KEY: &str = "seeds";
const FILES_KEY: &str = "files";

const RECORDED: &str = "recorded";
const FORKED: &str = "fork";
const NO_AGENT_TYPE: &str = "no_agent_type";
const NO_RECORD: &str = "no_record";
const AMBIGUOUS_PAIRING: &str = "ambiguous_pairing";
const RECORD_UNREADABLE: &str = "record_unreadable";
const NO_NEW_FILE: &str = "no_new_file";

// A brief with no seed is recorded too: a record of every brief keeps a pairing honest.
pub(crate) fn recorded(
    run: &Run<'_>,
    services: &Services,
    stdin: &mut dyn Read,
    facts: &mut Facts,
) -> Result<Outcome, &'static str> {
    let fields = run::fields(stdin)?;
    let context = run::context(&fields, facts)?;
    let kind = run::input_text(&fields, "subagent_type").unwrap_or(DEFAULT_TYPE);
    if kind == FORK {
        return Err(FORKED);
    }
    let prompt = run::input_text(&fields, "prompt").ok_or(NO_PROMPT)?;
    let root = run
        .cwd(&fields)
        .and_then(|cwd| repository::root_of(services, &cwd));
    let seeded = match &root {
        Some(root) => seeds::seeded(prompt, &|rel| root.join(rel).is_file()),
        None => seeds::seeded(prompt, &|_| false),
    };
    if !state::private_dir(&run.ledger) {
        return Err(LEDGER_STORAGE_UNAVAILABLE);
    }
    let dir = ledger::briefs_dir(&run.ledger, &context.session);
    let text = json::value_text(&record_value(root.as_deref(), &seeded));
    written(&dir, kind, wall_ns(run.clock), &text)?;
    Ok(Outcome::Recorded(RECORDED))
}

fn record_value(root: Option<&Path>, seeded: &Seeded) -> JsonValue {
    let texts = |items: &[String]| {
        JsonValue::Array(
            items
                .iter()
                .map(|item| JsonValue::Str(item.as_str().into()))
                .collect(),
        )
    };
    let root = root.and_then(Path::to_str);
    let root = root.map_or(JsonValue::Null, |root| JsonValue::Str(root.into()));
    JsonValue::Object(vec![
        (ROOT_KEY.into(), root),
        (SEEDS_KEY.into(), texts(&seeded.seeds)),
        (FILES_KEY.into(), texts(&seeded.files)),
    ])
}

// The type is hashed into the name, because a type is the caller's text.
fn record_prefix(kind: &str) -> String {
    format!("{}-", sha256_hex(kind.as_bytes()))
}

fn written(dir: &Path, kind: &str, ns: u128, text: &str) -> Result<(), &'static str> {
    io::create_missing_owner_dir(dir).map_err(|_| LEDGER_STORAGE_UNAVAILABLE)?;
    for at in ns..ns + NAME_TRIES {
        let name = dir.join(format!("{}{at}", record_prefix(kind)));
        match io::write_new_file(&name, text.as_bytes()) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(_) => return Err(LEDGER_STORAGE_UNAVAILABLE),
        }
    }
    Err(LEDGER_STORAGE_UNAVAILABLE)
}

fn wall_ns(clock: &dyn Clock) -> u128 {
    let since = clock.wall().duration_since(UNIX_EPOCH);
    since.map_or(0, |since| since.as_nanos())
}

// A brief as subagent-map reads it back.
struct Brief {
    root: Option<PathBuf>,
    seeds: Vec<String>,
    files: Vec<String>,
}

pub(crate) fn mapped(
    run: &Run<'_>,
    map: MapFn,
    services: &Services,
    stdin: &mut dyn Read,
    facts: &mut Facts,
) -> Result<Outcome, &'static str> {
    let fields = run::fields(stdin)?;
    let context = run::context(&fields, facts)?;
    let kind = text_field(&fields, "agent_type").and_then(CodePointString::as_str);
    let kind = kind.ok_or(NO_AGENT_TYPE)?;
    if !state::private_dir(&run.ledger) {
        return Err(LEDGER_STORAGE_UNAVAILABLE);
    }
    let dir = ledger::briefs_dir(&run.ledger, &context.session);
    let brief = taken(&dir, kind, wall_ns(run.clock))?;
    if brief.seeds.is_empty() {
        return Err(NO_SEED);
    }
    let root = (brief.root.clone()).or_else(|| repository::root_of(services, &run.cwd(&fields)?));
    let request = MapRequest {
        focus: brief.seeds.clone(),
        budget: None,
        max_files: Some(IntArg::from_usize(RANKED_FILES)),
        granularity: Some(Granularity::File),
    };
    let (services, root) = (services.clone(), root.ok_or(NO_REPOSITORY)?);
    let finished = run.timed(move || map(&services, &root, &request), facts);
    let result = run::finished(finished, MAP_FAILED)?;
    sentence(&result, &brief).map(Outcome::Context)
}

// The one pending record of the type, which this run removes. Two or more are ambiguous: each
// is removed, and none pairs.
fn taken(dir: &Path, kind: &str, now: u128) -> Result<Brief, &'static str> {
    let pending = pending(dir, kind, now);
    let [only] = pending.as_slice() else {
        if pending.is_empty() {
            return Err(NO_RECORD);
        }
        for record in &pending {
            drop(io::remove_file(record));
        }
        return Err(AMBIGUOUS_PAIRING);
    };
    let text = fs::read_to_string(only).map_err(|_| NO_RECORD)?;
    // A parallel run that removed it first took it.
    if !matches!(io::remove_file(only), Ok(true)) {
        return Err(NO_RECORD);
    }
    brief_of(&text).ok_or(RECORD_UNREADABLE)
}

fn pending(dir: &Path, kind: &str, now: u128) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let prefix = record_prefix(kind);
    let young = |name: &str| {
        let ns = name.strip_prefix(&prefix)?.parse::<u128>().ok()?;
        (now.saturating_sub(ns) < PENDING.as_nanos()).then_some(())
    };
    let records = entries.filter_map(|entry| {
        let path = entry.ok()?.path();
        young(path.file_name()?.to_str()?)?;
        Some(path)
    });
    records.collect()
}

fn brief_of(text: &str) -> Option<Brief> {
    let document = json::parse(text).ok()?;
    let JsonValue::Object(fields) = &document else {
        return None;
    };
    let texts = |key: &str| match get(fields, key) {
        Some(JsonValue::Array(items)) => items.iter().map(owned_text).collect(),
        _ => None,
    };
    let root = match get(fields, ROOT_KEY) {
        Some(JsonValue::Null) => None,
        Some(value) => Some(PathBuf::from(owned_text(value)?)),
        None => return None,
    };
    Some(Brief {
        root,
        seeds: texts(SEEDS_KEY)?,
        files: texts(FILES_KEY)?,
    })
}

fn owned_text(value: &JsonValue) -> Option<String> {
    match value {
        JsonValue::Str(text) => text.as_str().map(str::to_owned),
        _ => None,
    }
}

// The files are the first 2 of the first 5 ranked files that the brief does not name.
fn sentence(result: &MapResult, brief: &Brief) -> Result<String, &'static str> {
    let names = seeds::resolved(result, &brief.seeds)?;
    let files: Vec<String> = (result.files.iter().take(RANKED_FILES))
        .map(|file| file.path.clone())
        .filter(|path| !brief.files.contains(path))
        .take(seeds::MAX_FILES)
        .collect();
    if files.is_empty() {
        return Err(NO_NEW_FILE);
    }
    seeds::sentence(&SUBAGENT_MAP_CONTEXT, &names, &files).ok_or(TOO_LONG)
}

#[cfg(test)]
mod tests;
