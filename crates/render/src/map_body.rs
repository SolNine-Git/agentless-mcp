//! The body map's two forms: the map's file rows, then the bodies and their misses, in the text;
//! the map document with a `bodies` key in the JSON, trimmed at its bodies.

use app::BodyMapResult;
use serde::Serialize;

use crate::envelope::{Listing, wrap_json};
use crate::map::{MapJson, map_text};
use crate::symbols::{ExpandJson, expand_stderr_rows, expansion_text};
use crate::{Rendered, Truncation, View};

impl View for BodyMapResult {
    fn rendered(&self) -> Rendered<'_> {
        let truncation = Truncation {
            shown: self.bodies.cards.len(),
            total: self.map.candidates,
            unit: "symbols",
        };
        let payload = BodyMapJson {
            map: MapJson::of(&self.map),
            bodies: ExpandJson::of(&self.bodies),
        };
        Rendered::new(body_map_text(self), payload, Some(truncation), wrap_json)
    }
}

// A tool call has one channel, so the misses ride in the body. Every id came off the scan
// moments earlier, so a miss here is evidence of a race.
fn body_map_text(result: &BodyMapResult) -> String {
    let mut body = vec![expansion_text(&result.bodies)];
    body.extend(expand_stderr_rows(&result.bodies));
    format!("{}\n{}", map_text(&result.map), body.join("\n"))
}

#[derive(Clone, Serialize)]
struct BodyMapJson<'a> {
    #[serde(flatten)]
    map: MapJson<'a>,
    bodies: ExpandJson<'a>,
}

// The bulk of the answer is the bodies, so the ceiling trims them and keeps the file rows.
impl Listing for BodyMapJson<'_> {
    fn items(&self) -> usize {
        self.bodies.symbols.len()
    }

    fn prefix(&self, kept: usize) -> Self {
        let mut prefix = self.clone();
        prefix.bodies.symbols.truncate(kept);
        prefix
    }
}
