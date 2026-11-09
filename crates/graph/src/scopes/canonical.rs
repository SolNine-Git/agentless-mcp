//! The owned form of the import scopes, in list order, that an update is compared by.

use std::ptr;

use serde_json::{Value, json};

use super::{ImportScope, Qualified, Scopes};

impl Scopes<'_> {
    // Each file's scope in scan order, and the path order. The scopes must read the facts of the
    // scan they were built from.
    pub(crate) fn canonical(&self, files: &[&crate::FileFacts]) -> Value {
        assert!(ptr::eq(self.files, files));
        let scopes: Vec<Value> = (self.files.iter().zip(&self.per_file))
            .map(|(file, scope)| json!([file.path, scope.canonical()]))
            .collect();
        let exports: Vec<Value> = (self.exports.iter())
            .map(|(key, target)| json!([key, target]))
            .collect();
        json!({
            "scopes": scopes,
            "by_path": self.by_path,
            "entries": self.entries.canonical(),
            "exports": exports,
            "importers": self.importers,
        })
    }
}

impl ImportScope<'_> {
    fn canonical(&self) -> Value {
        let statements: Vec<Value> = (self.statements.iter())
            .map(|row| {
                json!([
                    row.module,
                    row.line,
                    row.resolved,
                    row.relative,
                    row.statement
                ])
            })
            .collect();
        let qualified: Vec<Value> = (self.qualified.iter())
            .map(|(qualifier, named)| match named {
                Qualified::Module(module) => json!([qualifier, module]),
                Qualified::Foreign => json!([qualifier, null]),
            })
            .collect();
        json!({
            "wholesale": self.wholesale,
            "module_bindings": self.module_bindings,
            "named": self.named,
            "members": self.members,
            "statements": statements,
            "qualified": qualified,
        })
    }
}
