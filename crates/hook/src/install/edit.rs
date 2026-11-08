//! The pure edit of a settings document: the removal of every agentless handler and of each group
//! and event that the removal empties, then, for an install, one group per row of the entry
//! table. Every other key, group and handler stays where it is.

use app::hooks::{self, DEFAULT_SERVER_NAME, Entry, EntryState, Expected, Handler, HooksShape};
use app::hooks::{Hook, Retired};
use json::JsonValue;
use text::CodePointString;

use crate::text::lossy;

const HOOKS_KEY: &str = "hooks";
const MATCHER_KEY: &str = "matcher";
const TYPE_KEY: &str = "type";
const COMMAND_KEY: &str = "command";
const TIMEOUT_KEY: &str = "timeout";
const COMMAND_TYPE: &str = "command";

type Fields = Vec<(CodePointString, JsonValue)>;

// Why a document cannot take the edit: its top level is not an object, or its hooks value has
// another shape than Claude Code reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Unshaped {
    TopLevel,
    Hooks(HooksShape),
}

// The agentless handlers that the removal took out, one item per handler, and the count of the
// other handlers that it kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Removed {
    pub(crate) current: Vec<Hook>,
    pub(crate) retired: Vec<Retired>,
    pub(crate) kept: usize,
}

// One event of the hooks value, and whether the removal took its last group.
struct Event {
    key: CodePointString,
    groups: Vec<JsonValue>,
    emptied: bool,
}

// The hooks value split into its events, with the place of the hooks key among the top keys.
struct Split {
    top: Fields,
    at: Option<usize>,
    events: Vec<Event>,
    removed: Removed,
}

// The document with the table's rows, for the binary at `binary`.
pub(crate) fn installed(
    document: &JsonValue,
    binary: &str,
    server_name: &str,
) -> Result<(JsonValue, Removed), Unshaped> {
    let mut split = split(document)?;
    for entry in hooks::entries(server_name) {
        appended(&mut split.events, &entry, binary);
    }
    Ok(joined(split, false))
}

// The document with no agentless handler.
pub(crate) fn uninstalled(document: &JsonValue) -> Result<(JsonValue, Removed), Unshaped> {
    split(document).map(|split| joined(split, true))
}

fn split(document: &JsonValue) -> Result<Split, Unshaped> {
    let JsonValue::Object(top) = document else {
        return Err(Unshaped::TopLevel);
    };
    let at = top
        .iter()
        .position(|(key, _)| key.as_str() == Some(HOOKS_KEY));
    let mut removed = Removed::default();
    let events = match at {
        Some(at) => pruned(&top[at].1, &mut removed)?,
        None => Vec::new(),
    };
    Ok(Split {
        top: top.clone(),
        at,
        events,
        removed,
    })
}

// The parse of the hooks value checks its shape and reads its handlers in file order; the walk
// below meets them in the same order.
fn pruned(value: &JsonValue, removed: &mut Removed) -> Result<Vec<Event>, Unshaped> {
    let parsed = hooks::handlers(value).map_err(Unshaped::Hooks)?;
    let mut parsed = parsed.into_iter();
    let JsonValue::Object(events) = value else {
        return Err(shape(String::new(), Expected::Object));
    };
    let mut kept = Vec::new();
    for (key, groups) in events {
        let JsonValue::Array(groups) = groups else {
            return Err(shape(lossy(key), Expected::Array));
        };
        let left: Vec<JsonValue> = (groups.iter())
            .filter_map(|group| pruned_group(group, &mut parsed, removed))
            .collect();
        kept.push(Event {
            key: key.clone(),
            emptied: left.is_empty() && !groups.is_empty(),
            groups: left,
        });
    }
    Ok(kept)
}

// The group without its agentless handlers, or None when the removal took its last handler.
fn pruned_group(
    group: &JsonValue,
    parsed: &mut impl Iterator<Item = Handler>,
    removed: &mut Removed,
) -> Option<JsonValue> {
    let JsonValue::Object(fields) = group else {
        return Some(group.clone());
    };
    let at = fields
        .iter()
        .position(|(key, _)| key.as_str() == Some(HOOKS_KEY));
    let Some(JsonValue::Array(listed)) = at.map(|at| &fields[at].1) else {
        return Some(group.clone());
    };
    let left: Vec<JsonValue> = (listed.iter())
        .filter(|_| kept(parsed.next().as_ref(), removed))
        .cloned()
        .collect();
    if left.is_empty() && !listed.is_empty() {
        return None;
    }
    let mut fields = fields.clone();
    if let Some(at) = at {
        fields[at].1 = JsonValue::Array(left);
    }
    Some(JsonValue::Object(fields))
}

// An agentless entry runs `<path> hook <name>` (design rule 1). The table's own comparison
// decides it, so the rule keeps one home.
fn kept(handler: Option<&Handler>, removed: &mut Removed) -> bool {
    let Some(handler) = handler else {
        return true;
    };
    let status = hooks::compare(std::slice::from_ref(handler), "", DEFAULT_SERVER_NAME);
    if let Some(retired) = status.retired.first() {
        removed.retired.push(retired.hook);
        return false;
    }
    let current = (status.rows.iter()).find(|row| row.state != EntryState::Missing);
    if let Some(row) = current {
        removed.current.push(row.entry.hook);
        return false;
    }
    removed.kept += 1;
    true
}

fn appended(events: &mut Vec<Event>, entry: &Entry, binary: &str) {
    let key = entry.event.as_str();
    let group = group(entry, binary);
    match events
        .iter_mut()
        .find(|event| event.key.as_str() == Some(key))
    {
        Some(event) => event.groups.push(group),
        None => events.push(Event {
            key: key.into(),
            groups: vec![group],
            emptied: false,
        }),
    }
}

fn group(entry: &Entry, binary: &str) -> JsonValue {
    let command = hooks::command(binary, entry.hook);
    let handler = JsonValue::Object(vec![
        (TYPE_KEY.into(), JsonValue::Str(COMMAND_TYPE.into())),
        (COMMAND_KEY.into(), JsonValue::Str(command.as_str().into())),
        (
            TIMEOUT_KEY.into(),
            JsonValue::Int(entry.timeout.to_string()),
        ),
    ]);
    let mut fields: Fields = Vec::new();
    if let Some(matcher) = &entry.matcher {
        fields.push((MATCHER_KEY.into(), JsonValue::Str(matcher.as_str().into())));
    }
    fields.push((HOOKS_KEY.into(), JsonValue::Array(vec![handler])));
    JsonValue::Object(fields)
}

// The events back in the document, without each that the removal emptied and nothing refilled.
// An uninstall that empties the hooks value removes the hooks key too.
fn joined(split: Split, drop_empty_hooks: bool) -> (JsonValue, Removed) {
    let Split {
        mut top,
        at,
        events,
        removed,
    } = split;
    let emptied_all = !events.is_empty() && events.iter().all(|event| event.emptied);
    let events: Fields = (events.into_iter())
        .filter(|event| !(event.emptied && event.groups.is_empty()))
        .map(|event| (event.key, JsonValue::Array(event.groups)))
        .collect();
    match at {
        Some(at) if drop_empty_hooks && emptied_all => {
            top.remove(at);
        }
        Some(at) => top[at].1 = JsonValue::Object(events),
        None if !events.is_empty() => top.push((HOOKS_KEY.into(), JsonValue::Object(events))),
        None => {}
    }
    (JsonValue::Object(top), removed)
}

fn shape(at: String, expected: Expected) -> Unshaped {
    Unshaped::Hooks(HooksShape { at, expected })
}
