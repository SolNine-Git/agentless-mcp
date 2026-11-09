//! The MCP tool surface as data: the tools in list order, their parameters, and the tools/list
//! entry of each tool.

use json::JsonValue;
use prompts::parameter_descriptions as parameter;
use prompts::tool_descriptions as described;
use text::CodePointString;

use crate::search::{MAX_SEARCH_CONTEXT, MAX_SEARCH_LIMIT};
use crate::trace::{MAX_TARGETS, MIN_TARGETS};

/// One tool of the surface.
#[derive(Debug)]
pub struct Tool {
    /// The tool name.
    pub name: &'static str,
    /// The display title, which the annotations repeat.
    pub title: &'static str,
    /// The description that a client reads.
    pub description: &'static str,
    /// What a call can change, which the annotation hints state.
    pub effect: Effect,
    /// True when a client that defers tool schemas loads this one at the start.
    pub always_load: bool,
    /// The parameters in declaration order, which is the order of the schema properties.
    pub params: &'static [Param],
}

/// What a call to a tool can change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// The call reads only, repeats with no further effect, and reaches nothing outside the server.
    ReadOnly,
}

/// One parameter of a tool.
#[derive(Debug)]
pub struct Param {
    /// The parameter name.
    pub name: &'static str,
    /// The type and the bounds of its value.
    pub shape: Shape,
    /// Whether a call must give it, and what its absence reads as.
    pub presence: Presence,
    /// The description that a client reads.
    pub description: &'static str,
}

/// Whether a call must give a parameter, and what its absence reads as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// A call must give it.
    Required,
    /// A call can give null or leave it out, and both read as null.
    Nullable,
    /// A call can leave it out, and its absence reads as false.
    DefaultFalse,
}

/// The type and the bounds of a parameter value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// A string.
    Text,
    /// A boolean.
    Bool,
    /// An integer from `minimum` up to `maximum`.
    Int {
        /// The smallest value.
        minimum: usize,
        /// The largest value, or `None` for no bound.
        maximum: Option<usize>,
    },
    /// A real number above `exclusive_minimum` and at most `maximum`.
    Number {
        /// The bound that a value must exceed, written as an integer.
        exclusive_minimum: usize,
        /// The largest value, written as a real number.
        maximum: f64,
    },
    /// One of these strings.
    Literal(&'static [&'static str]),
    /// One of these strings, with the description of the named type that holds them.
    Enum {
        /// The strings in declaration order.
        names: &'static [&'static str],
        /// The description of the type.
        description: &'static str,
    },
    /// A list of values of one shape.
    Array {
        /// The shape of each item.
        items: &'static Shape,
        /// The fewest items, or `None` for no bound.
        min_items: Option<usize>,
        /// The most items, or `None` for no bound.
        max_items: Option<usize>,
    },
    /// A value of any one of these shapes.
    AnyOf(&'static [Shape]),
}

/// How many roots a call can select among, which decides whether tools/list publishes `repo_root`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootCount {
    /// One root or none, so a call has nothing to select.
    One,
    /// More than one root, or a roots file whose roots can change after start-up.
    Several,
}

/// The tools in the order that a tools/list answer gives them.
pub static TOOLS: [&Tool; 2] = [&TRACE, &SEARCH];

/// The values of the trace tool's `more`, in the order of [`crate::trace::Section::ALL`].
pub const MORE_SECTIONS: [&str; 12] = [
    "candidates",
    "callers",
    "callees",
    "tests",
    "similar",
    "history",
    "body",
    "declares",
    "imports",
    "importers",
    "cycles",
    "tree",
];

const TARGETS: Shape = Shape::AnyOf(&[
    Shape::Text,
    Shape::Array {
        items: &Shape::Text,
        min_items: Some(MIN_TARGETS),
        max_items: Some(MAX_TARGETS),
    },
]);

const REPO_ROOT: Param = optional("repo_root", Shape::Text, parameter::REPO_ROOT);

const fn int(minimum: usize, maximum: usize) -> Shape {
    Shape::Int {
        minimum,
        maximum: Some(maximum),
    }
}

const fn param(
    name: &'static str,
    shape: Shape,
    presence: Presence,
    description: &'static str,
) -> Param {
    Param {
        name,
        shape,
        presence,
        description,
    }
}

const fn optional(name: &'static str, shape: Shape, description: &'static str) -> Param {
    param(name, shape, Presence::Nullable, description)
}

static TRACE: Tool = Tool {
    name: "trace",
    title: "Trace",
    description: described::TRACE,
    effect: Effect::ReadOnly,
    always_load: true,
    params: &[
        param(
            "target",
            TARGETS,
            Presence::Required,
            parameter::TRACE_TARGET,
        ),
        REPO_ROOT,
        optional(
            "more",
            Shape::Literal(&MORE_SECTIONS),
            parameter::TRACE_MORE,
        ),
        optional("rev", Shape::Text, parameter::TRACE_REV),
    ],
};

static SEARCH: Tool = Tool {
    name: "search",
    title: "Search",
    description: described::SEARCH,
    effect: Effect::ReadOnly,
    always_load: true,
    params: &[
        optional("pattern", Shape::Text, parameter::SEARCH_PATTERN),
        REPO_ROOT,
        optional("files", Shape::Text, parameter::SEARCH_FILES),
        optional("path", Shape::Text, parameter::SEARCH_PATH),
        optional("ignore_case", Shape::Bool, parameter::SEARCH_IGNORE_CASE),
        optional(
            "context",
            int(0, MAX_SEARCH_CONTEXT),
            parameter::SEARCH_CONTEXT,
        ),
        optional("limit", int(1, MAX_SEARCH_LIMIT), parameter::SEARCH_LIMIT),
    ],
};

/// Returns the entry that a tools/list answer of a server with `count` roots holds for `tool`,
/// with its keys in wire order.
#[must_use]
pub fn tool_entry(tool: &Tool, count: RootCount) -> JsonValue {
    object(vec![
        ("name", text(tool.name)),
        ("title", text(tool.title)),
        ("description", text(tool.description)),
        ("inputSchema", input_schema(tool, count)),
        ("annotations", annotations(tool)),
        ("_meta", meta(tool)),
    ])
}

fn input_schema(tool: &Tool, count: RootCount) -> JsonValue {
    let params = published(tool, count);
    let properties = params
        .iter()
        .map(|param| (param.name, param_schema(param)))
        .collect();
    let required: Vec<JsonValue> = params
        .iter()
        .filter(|param| param.presence == Presence::Required)
        .map(|param| text(param.name))
        .collect();
    let mut entries = vec![
        ("additionalProperties", JsonValue::Bool(false)),
        ("properties", object(properties)),
        ("type", text("object")),
    ];
    if !required.is_empty() {
        entries.push(("required", JsonValue::Array(required)));
    }
    schema(entries)
}

// One root leaves a call nothing to select, so the schema leaves out `repo_root`. The validation
// still accepts it.
fn published(tool: &Tool, count: RootCount) -> Vec<&'static Param> {
    tool.params
        .iter()
        .filter(|param| count == RootCount::Several || param.name != REPO_ROOT.name)
        .collect()
}

fn annotations(tool: &Tool) -> JsonValue {
    let [read_only, destructive, idempotent, open_world] = hints(tool.effect);
    object(vec![
        ("title", text(tool.title)),
        ("readOnlyHint", JsonValue::Bool(read_only)),
        ("destructiveHint", JsonValue::Bool(destructive)),
        ("idempotentHint", JsonValue::Bool(idempotent)),
        ("openWorldHint", JsonValue::Bool(open_world)),
    ])
}

// The read-only, destructive, idempotent and open-world hints of an effect.
fn hints(effect: Effect) -> [bool; 4] {
    match effect {
        Effect::ReadOnly => [true, false, true, false],
    }
}

// Every tool's _meta also carries the empty `fastmcp` tag list of the published surface.
fn meta(tool: &Tool) -> JsonValue {
    let tags = (
        "fastmcp",
        object(vec![("tags", JsonValue::Array(Vec::new()))]),
    );
    if tool.always_load {
        object(vec![("anthropic/alwaysLoad", JsonValue::Bool(true)), tags])
    } else {
        object(vec![tags])
    }
}

// A nullable parameter publishes its shape alone: the validation still reads a null as absent.
fn param_schema(param: &Param) -> JsonValue {
    let mut entries = shape_entries(&param.shape);
    // The parameter's description replaces the description of its type.
    entries.retain(|(key, _)| *key != "description");
    if param.presence == Presence::DefaultFalse {
        entries.push(("default", JsonValue::Bool(false)));
    }
    entries.push(("description", text(param.description)));
    schema(entries)
}

fn shape_schema(shape: &Shape) -> JsonValue {
    schema(shape_entries(shape))
}

fn shape_entries(shape: &Shape) -> Vec<(&'static str, JsonValue)> {
    match *shape {
        Shape::Text => vec![("type", text("string"))],
        Shape::Bool => vec![("type", text("boolean"))],
        Shape::Int { minimum, maximum } => {
            let mut entries = vec![("minimum", int_value(minimum)), ("type", text("integer"))];
            entries.extend(maximum.map(|maximum| ("maximum", int_value(maximum))));
            entries
        }
        Shape::Number {
            exclusive_minimum,
            maximum,
        } => vec![
            ("exclusiveMinimum", int_value(exclusive_minimum)),
            ("maximum", JsonValue::Float(maximum)),
            ("type", text("number")),
        ],
        Shape::Literal(names) => vec![("enum", texts(names)), ("type", text("string"))],
        Shape::Enum { names, description } => vec![
            ("description", text(description)),
            ("enum", texts(names)),
            ("type", text("string")),
        ],
        Shape::Array {
            items,
            min_items,
            max_items,
        } => array_entries(items, min_items, max_items),
        Shape::AnyOf(shapes) => vec![(
            "anyOf",
            JsonValue::Array(shapes.iter().map(shape_schema).collect()),
        )],
    }
}

fn array_entries(
    items: &Shape,
    min_items: Option<usize>,
    max_items: Option<usize>,
) -> Vec<(&'static str, JsonValue)> {
    let mut entries = vec![("items", shape_schema(items)), ("type", text("array"))];
    entries.extend(min_items.map(|count| ("minItems", int_value(count))));
    entries.extend(max_items.map(|count| ("maxItems", int_value(count))));
    entries
}

// A schema object lists its keys in code point order.
fn schema(mut entries: Vec<(&'static str, JsonValue)>) -> JsonValue {
    entries.sort_by_key(|(key, _)| *key);
    object(entries)
}

fn object(entries: Vec<(&str, JsonValue)>) -> JsonValue {
    JsonValue::Object(
        entries
            .into_iter()
            .map(|(key, value)| (CodePointString::from(key), value))
            .collect(),
    )
}

fn text(value: &str) -> JsonValue {
    JsonValue::Str(CodePointString::from(value))
}

fn texts(values: &[&str]) -> JsonValue {
    JsonValue::Array(values.iter().map(|value| text(value)).collect())
}

fn int_value(value: usize) -> JsonValue {
    JsonValue::Int(value.to_string())
}

#[cfg(test)]
mod tests;
