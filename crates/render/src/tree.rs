//! The directory tree's two forms: the tree text, and its JSON with the counts behind it.

use app::TreeView;
use serde::Serialize;

use crate::envelope::wrap_json_whole;
use crate::{Rendered, View};

#[derive(Serialize)]
struct TreeJson<'a> {
    tree: &'a str,
    file_count: usize,
    depth: usize,
    max_entries: usize,
}

impl View for TreeView {
    fn rendered(&self) -> Rendered<'_> {
        let payload = TreeJson {
            tree: &self.text,
            file_count: self.file_count,
            depth: self.depth,
            max_entries: self.max_entries,
        };
        Rendered::new(self.text.clone(), payload, None, wrap_json_whole)
    }
}
