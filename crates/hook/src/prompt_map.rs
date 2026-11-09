//! The prompt map: on the first prompt of a session that names files or symbols of the
//! repository, one sentence of context that names the files the map ranks first for them.

pub(crate) mod seeds;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use app::hooks::Hook;
use app::{EnvConfig, IntArg, MapRequest, MapResult, Reading, Repo, Services, Session};
use io::Granularity;
use kernel::SystemClock;
use text::CodePointString;

use crate::repository;
use crate::run::{self, Facts, NO_REPOSITORY, Outcome, Run};
use crate::search::text_field;
use crate::state::{self, Claim};
use crate::text::sha256_hex;
use seeds::{MAP_FAILED, NO_PROMPT, NO_SEED, NO_SEED_RESOLVED, TOO_LONG};

pub(crate) type MapFn = fn(&Services, &Path, &MapRequest) -> Result<MapResult, app::Error>;

const MARKER_DIR: &str = "agentless_prompt_map";

const ALREADY_ENRICHED: &str = "already_enriched";
const MARKER_STORAGE_UNAVAILABLE: &str = "marker_storage_unavailable";

/// Answers the `UserPromptSubmit` payload on `stdin` with one JSON object on `stdout` that names
/// the map's first files for the prompt's names, or with nothing, and returns 0.
pub fn prompt_map(
    env: &EnvConfig,
    services: &Services,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) -> u8 {
    let clock = SystemClock;
    let hook = PromptMap {
        run: Run::of(Hook::PromptMap, env, &clock),
        markers: state::temporary_dir(env, MARKER_DIR),
        map: in_process,
    };
    answer(&hook, services, stdin, stdout);
    0
}

// A run of the prompt map: the shared run, the marker directory and the map.
pub(crate) struct PromptMap<'a> {
    pub(crate) run: Run<'a>,
    pub(crate) markers: PathBuf,
    pub(crate) map: MapFn,
}

// The map of one prompt, as the command line's `map` makes it.
pub(crate) fn in_process(
    services: &Services,
    root: &Path,
    request: &MapRequest,
) -> Result<MapResult, app::Error> {
    let repo = Repo::load(services, root)?;
    let session = Session::open_facts(services, repo, false).session.value?;
    app::map(services, &Reading::of(&session), request).value
}

pub(crate) fn answer(
    hook: &PromptMap<'_>,
    services: &Services,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) {
    run::answered(&hook.run, stdout, |facts| {
        decide(hook, services, stdin, facts)
    });
}

// The steps in order. A prompt with no seed writes no marker, so a later prompt can still emit.
fn decide(
    hook: &PromptMap<'_>,
    services: &Services,
    stdin: &mut dyn Read,
    facts: &mut Facts,
) -> Result<Outcome, &'static str> {
    let run = &hook.run;
    let fields = run::fields(stdin)?;
    let context = run::context(&fields, facts)?;
    let prompt = text_field(&fields, "prompt").and_then(CodePointString::as_str);
    let prompt = prompt.ok_or(NO_PROMPT)?;
    let cwd = run.cwd(&fields).ok_or(NO_REPOSITORY)?;
    let root = repository::root_of(services, &cwd)
        .map_err(|no_root| run::rootless(&no_root, NO_REPOSITORY, facts))?;
    let found = seeds::seeds(prompt, &|rel| root.join(rel).is_file());
    facts.seeds = Some(found.clone());
    if found.is_empty() {
        return Err(NO_SEED);
    }
    claim(&hook.markers, &context.session)?;
    let request = MapRequest {
        focus: found.clone(),
        budget: None,
        max_files: Some(IntArg::from_usize(seeds::MAX_FILES)),
        granularity: Some(Granularity::File),
    };
    let (services, map) = (services.clone(), hook.map);
    let finished = run.timed(move || map(&services, &root, &request), facts);
    let result = run::finished(finished, MAP_FAILED)?;
    context_sentence(&result, &found, facts).map(Outcome::Context)
}

// The session's one try: the marker is written before the map runs, so a deadline or a crash
// uses it too.
fn claim(markers: &Path, session: &str) -> Result<(), &'static str> {
    if !state::private_dir(markers) {
        return Err(MARKER_STORAGE_UNAVAILABLE);
    }
    match state::claim(&markers.join(sha256_hex(session.as_bytes()))) {
        Claim::New => Ok(()),
        Claim::Held => Err(ALREADY_ENRICHED),
        Claim::Unavailable => Err(MARKER_STORAGE_UNAVAILABLE),
    }
}

fn context_sentence(
    result: &MapResult,
    found: &[String],
    facts: &mut Facts,
) -> Result<String, &'static str> {
    let files: Vec<String> = (result.files.iter())
        .take(seeds::MAX_FILES)
        .map(|file| file.path.clone())
        .collect();
    facts.files = Some(files.clone());
    let names = seeds::resolved(result, found)?;
    if files.is_empty() {
        return Err(NO_SEED_RESOLVED);
    }
    seeds::context_text(&names, &files).ok_or(TOO_LONG)
}

#[cfg(test)]
mod tests;
