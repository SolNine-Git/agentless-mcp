//! The tools/list entries and the initialize answer: the reference surface, capabilities and
//! protocol versions.

use std::collections::BTreeMap;

use app::surface::{TOOLS, tool_entry};
use rmcp::model::{
    Implementation, InitializeResult, JsonObject, PromptsCapability, ProtocolVersion,
    ResourcesCapability, ServerCapabilities, Tool, ToolsCapability,
};

// The newest protocol version of the reference, and the answer to a version it does not know.
pub(crate) const NEWEST: ProtocolVersion = ProtocolVersion::V_2025_11_25;
const NAME: &str = "agentless-mcp";
const UI_EXTENSION: &str = "io.modelcontextprotocol/ui";

// Each tool's entry, read from the surface table's JSON so that its keys keep their order.
pub(crate) fn listed_tools() -> Vec<Tool> {
    TOOLS
        .iter()
        .map(|tool| {
            let text = json::value_text(&tool_entry(tool));
            serde_json::from_str(&text).expect("each surface entry is a tool")
        })
        .collect()
}

// The capabilities that the reference server advertises, with no client roots asked for.
pub(crate) fn initialized() -> InitializeResult {
    let mut capabilities = ServerCapabilities::default();
    capabilities.experimental = Some(BTreeMap::new());
    capabilities.logging = Some(JsonObject::new());
    let mut prompts = PromptsCapability::default();
    prompts.list_changed = Some(false);
    capabilities.prompts = Some(prompts);
    let mut resources = ResourcesCapability::default();
    resources.subscribe = Some(false);
    resources.list_changed = Some(false);
    capabilities.resources = Some(resources);
    let mut tools = ToolsCapability::default();
    tools.list_changed = Some(true);
    capabilities.tools = Some(tools);
    capabilities.extensions = Some(BTreeMap::from([(
        UI_EXTENSION.to_owned(),
        JsonObject::new(),
    )]));
    let mut result = InitializeResult::new(capabilities);
    result.protocol_version = NEWEST;
    result.server_info = Implementation::new(NAME, env!("CARGO_PKG_VERSION"));
    result
}
