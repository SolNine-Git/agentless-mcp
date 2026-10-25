//! The MCP tool surface as data: the tools in list order, their parameters, the operation tables,
//! and the tools/list entry of each tool.

use json::JsonValue;
use prompts::parameter_descriptions as parameter;
use prompts::tool_descriptions as described;
use text::CodePointString;

use crate::ops::limits::{
    MAX_BUDGET, MAX_CONTEXT_LINES, MAX_DIAGRAM_EDGES, MAX_DIAGRAM_NODES, MAX_LIMIT, MAX_RESOLUTION,
    MAX_TREE_DEPTH, MAX_TREE_ENTRIES, MIN_BUDGET,
};
use crate::ops::{GRANULARITY_NAMES, KIND_NAMES};

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
    /// How a call selects its work.
    pub calls: Calls,
}

/// What a call to a tool can change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// The call reads only, repeats with no further effect, and reaches nothing outside the server.
    ReadOnly,
}

/// How a call to a tool selects its work.
#[derive(Debug)]
pub enum Calls {
    /// The tool answers one question.
    Single {
        /// True when a call shows where code lives, which unlocks the search gate.
        localizes: bool,
    },
    /// The `operation` parameter names one row of this table.
    Operations(&'static [Operation]),
}

/// One operation of a tool.
#[derive(Debug)]
pub struct Operation {
    /// The operation name.
    pub name: &'static str,
    /// The parameters it accepts, in table order, apart from `repo_root` and `operation`.
    pub accepted: &'static [&'static str],
    /// The accepted parameters that a call must give, in table order.
    pub required: &'static [&'static str],
    /// True when a call shows where code lives, which unlocks the search gate.
    pub localizes: bool,
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
pub static TOOLS: [&Tool; 7] = [
    &FIND_REFERENCING_SYMBOLS,
    &HISTORY,
    &CAPABILITIES,
    &ORIENT,
    &GRAPH,
    &SYMBOLS,
    &READ,
];

// The description of the symbol kind type, which the kind parameter's schema carries.
const KIND_DESCRIPTION: &str = "Classification of extracted AST symbols.";

const TEXTS: Shape = list(&Shape::Text, None, None);
const FOCUS: Shape = Shape::AnyOf(&[Shape::Text, TEXTS]);
// One [start, end] pair, or a list of them: no value fits both, so the order sets only the
// order of a refusal's problems.
const LINES: Shape = Shape::AnyOf(&[LINE_RANGE, LINE_RANGES]);
// One or more [start, end] pairs of 1-based line numbers.
const LINE_RANGES: Shape = list(&LINE_RANGE, Some(1), None);
const LINE_RANGE: Shape = list(&LINE_NUMBER, Some(2), Some(2));
const LINE_NUMBER: Shape = Shape::Int {
    minimum: 1,
    maximum: None,
};
const LIMIT: Shape = int(1, MAX_LIMIT);
const BUDGET: Shape = int(MIN_BUDGET, MAX_BUDGET);
const CONTEXT_LINES: Shape = int(0, MAX_CONTEXT_LINES);

const REPO_ROOT: Param = optional("repo_root", Shape::Text, parameter::REPO_ROOT);

const fn int(minimum: usize, maximum: usize) -> Shape {
    Shape::Int {
        minimum,
        maximum: Some(maximum),
    }
}

const fn list(items: &'static Shape, min_items: Option<usize>, max_items: Option<usize>) -> Shape {
    Shape::Array {
        items,
        min_items,
        max_items,
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

const fn required(name: &'static str, description: &'static str) -> Param {
    param(name, Shape::Text, Presence::Required, description)
}

const fn optional(name: &'static str, shape: Shape, description: &'static str) -> Param {
    param(name, shape, Presence::Nullable, description)
}

const fn switch(name: &'static str, description: &'static str) -> Param {
    param(name, Shape::Bool, Presence::DefaultFalse, description)
}

const fn operation(
    name: &'static str,
    accepted: &'static [&'static str],
    required: &'static [&'static str],
) -> Operation {
    Operation {
        name,
        accepted,
        required,
        localizes: false,
    }
}

// An operation whose call shows where code lives.
const fn localizing(
    name: &'static str,
    accepted: &'static [&'static str],
    required: &'static [&'static str],
) -> Operation {
    Operation {
        localizes: true,
        ..operation(name, accepted, required)
    }
}

static FIND_REFERENCING_SYMBOLS: Tool = Tool {
    name: "find_referencing_symbols",
    title: "Find referencing symbols",
    description: described::FIND_REFERENCING_SYMBOLS,
    effect: Effect::ReadOnly,
    always_load: false,
    params: &[
        required("target", parameter::REFERENCE_TARGET),
        REPO_ROOT,
        optional("limit", LIMIT, parameter::REFERENCE_LIMIT),
        switch("shared_callers", parameter::SHARED_CALLERS),
    ],
    calls: Calls::Single { localizes: true },
};

static HISTORY: Tool = Tool {
    name: "history",
    title: "History",
    description: described::HISTORY,
    effect: Effect::ReadOnly,
    always_load: false,
    params: &[
        required("target", parameter::HISTORY_TARGET),
        REPO_ROOT,
        optional("limit", LIMIT, parameter::HISTORY_LIMIT),
        optional("budget", BUDGET, parameter::HISTORY_BUDGET),
    ],
    calls: Calls::Single { localizes: false },
};

static CAPABILITIES: Tool = Tool {
    name: "capabilities",
    title: "Capabilities",
    description: described::CAPABILITIES,
    effect: Effect::ReadOnly,
    always_load: false,
    params: &[REPO_ROOT],
    calls: Calls::Single { localizes: false },
};

static ORIENT: Tool = Tool {
    name: "orient",
    title: "Orient",
    description: described::ORIENT,
    effect: Effect::ReadOnly,
    always_load: true,
    params: &ORIENT_PARAMS,
    calls: Calls::Operations(&[localizing(
        "map",
        &["focus", "budget", "limit", "granularity"],
        &[],
    )]),
};

const ORIENT_PARAMS: [Param; 6] = [
    required("operation", parameter::ORIENT_OPERATION),
    REPO_ROOT,
    optional("focus", FOCUS, parameter::ORIENT_FOCUS),
    optional("budget", BUDGET, parameter::MAP_BUDGET),
    optional(
        "granularity",
        Shape::Literal(&GRANULARITY_NAMES),
        parameter::MAP_GRANULARITY,
    ),
    optional("limit", LIMIT, parameter::ORIENT_LIMIT),
];

static GRAPH: Tool = Tool {
    name: "graph",
    title: "Graph",
    description: described::GRAPH,
    effect: Effect::ReadOnly,
    always_load: false,
    params: &GRAPH_PARAMS,
    calls: Calls::Operations(&[
        operation("communities", &["resolution", "limit"], &[]),
        operation("cycles", &["limit"], &[]),
        operation("health", &["limit"], &[]),
        operation(
            "diagram",
            &[
                "focus",
                "max_nodes",
                "max_edges",
                "group_by_communities",
                "resolution",
            ],
            &[],
        ),
        localizing(
            "path",
            &["source", "target", "include_unique", "include_ambiguous"],
            &["source", "target"],
        ),
    ]),
};

const GRAPH_PARAMS: [Param; 12] = [
    required("operation", parameter::GRAPH_OPERATION),
    REPO_ROOT,
    optional("focus", FOCUS, parameter::GRAPH_FOCUS),
    optional("resolution", RESOLUTION, parameter::COMMUNITY_RESOLUTION),
    optional("limit", LIMIT, parameter::GRAPH_LIMIT),
    optional("source", Shape::Text, parameter::PATH_SOURCE),
    optional("target", Shape::Text, parameter::PATH_TARGET),
    optional("include_unique", Shape::Bool, parameter::INCLUDE_UNIQUE),
    optional(
        "include_ambiguous",
        Shape::Bool,
        parameter::INCLUDE_AMBIGUOUS,
    ),
    optional(
        "max_nodes",
        int(1, MAX_DIAGRAM_NODES),
        parameter::DIAGRAM_MAX_NODES,
    ),
    optional(
        "max_edges",
        int(0, MAX_DIAGRAM_EDGES),
        parameter::DIAGRAM_MAX_EDGES,
    ),
    optional(
        "group_by_communities",
        Shape::Bool,
        parameter::GROUP_BY_COMMUNITIES,
    ),
];

const RESOLUTION: Shape = Shape::Number {
    exclusive_minimum: 0,
    maximum: MAX_RESOLUTION,
};

static SYMBOLS: Tool = Tool {
    name: "symbols",
    title: "Symbols",
    description: described::SYMBOLS,
    effect: Effect::ReadOnly,
    always_load: true,
    params: &SYMBOLS_PARAMS,
    calls: Calls::Operations(&[
        localizing("find", &["name", "kind", "limit"], &["name"]),
        localizing("overview", &["paths", "docstrings"], &["paths"]),
        localizing("expand", &["stable_ids", "limit"], &["stable_ids"]),
        localizing("explain", &["target", "limit"], &["target"]),
        operation(
            "locate",
            &["path", "locations", "context_lines"],
            &["path", "locations"],
        ),
    ]),
};

const SYMBOLS_PARAMS: [Param; 12] = [
    required("operation", parameter::SYMBOLS_OPERATION),
    REPO_ROOT,
    optional("name", Shape::Text, parameter::FIND_NAME),
    optional("kind", KIND, parameter::SYMBOL_KIND),
    optional("limit", LIMIT, parameter::SYMBOLS_LIMIT),
    optional("paths", TEXTS, parameter::OVERVIEW_PATHS),
    optional("docstrings", Shape::Bool, parameter::DOCSTRINGS),
    optional("stable_ids", TEXTS, parameter::STABLE_IDS),
    optional("target", Shape::Text, parameter::EXPLAIN_TARGET),
    optional("path", Shape::Text, parameter::FILE_PATH),
    optional("locations", TEXTS, parameter::LOCATIONS),
    optional("context_lines", CONTEXT_LINES, parameter::CONTEXT_LINES),
];

const KIND: Shape = Shape::Enum {
    names: &KIND_NAMES,
    description: KIND_DESCRIPTION,
};

static READ: Tool = Tool {
    name: "read",
    title: "Read",
    description: described::READ,
    effect: Effect::ReadOnly,
    always_load: true,
    params: &READ_PARAMS,
    calls: Calls::Operations(&[
        localizing(
            "slice",
            &["path", "lines", "context_lines", "whole_file"],
            &["path"],
        ),
        operation("dir", &["path", "depth", "max_entries"], &[]),
    ]),
};

const READ_PARAMS: [Param; 8] = [
    required("operation", parameter::READ_OPERATION),
    REPO_ROOT,
    optional("path", Shape::Text, parameter::READ_PATH),
    optional("lines", LINES, parameter::SLICE_LINES),
    optional("context_lines", CONTEXT_LINES, parameter::CONTEXT_LINES),
    optional("whole_file", Shape::Bool, parameter::WHOLE_FILE),
    optional("depth", int(1, MAX_TREE_DEPTH), parameter::TREE_DEPTH),
    optional(
        "max_entries",
        int(1, MAX_TREE_ENTRIES),
        parameter::TREE_MAX_ENTRIES,
    ),
];

impl Tool {
    /// Returns the operation named `name`, or `None` when the tool has no such operation.
    #[must_use]
    pub fn operation(&self, name: &str) -> Option<&'static Operation> {
        match self.calls {
            Calls::Single { .. } => None,
            Calls::Operations(operations) => {
                operations.iter().find(|operation| operation.name == name)
            }
        }
    }

    /// Returns true when a call with this `operation` argument shows where code lives.
    #[must_use]
    pub fn localizes(&self, operation: Option<&str>) -> bool {
        match self.calls {
            Calls::Single { localizes } => localizes,
            Calls::Operations(_) => operation
                .and_then(|name| self.operation(name))
                .is_some_and(|operation| operation.localizes),
        }
    }
}

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
        .map(|param| (param.name, param_schema(tool, param)))
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
fn param_schema(tool: &Tool, param: &Param) -> JsonValue {
    let mut entries = value_entries(tool, param);
    // The parameter's description replaces the description of its type.
    entries.retain(|(key, _)| *key != "description");
    if param.presence == Presence::DefaultFalse {
        entries.push(("default", JsonValue::Bool(false)));
    }
    entries.push(("description", text(param.description)));
    schema(entries)
}

// The `operation` parameter publishes the operation table's names, in table order, as its enum.
// Its validation stays a string, so an unknown name gets the refusal that lists the operations.
fn value_entries(tool: &Tool, param: &Param) -> Vec<(&'static str, JsonValue)> {
    match tool.calls {
        Calls::Operations(operations) if param.name == "operation" => {
            let names = operations.iter().map(|operation| text(operation.name));
            vec![
                ("enum", JsonValue::Array(names.collect())),
                ("type", text("string")),
            ]
        }
        _ => shape_entries(&param.shape),
    }
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
