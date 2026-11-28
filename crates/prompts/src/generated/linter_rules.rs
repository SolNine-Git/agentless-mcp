//! The linter rules, one record per language, in name order.
//! `cargo xtask generate` writes this file from the prompt JSON files.

use crate::linter_rule::{Linter, LinterGap, LinterRules};

pub(crate) static LINTER_RULES: [LinterRules; 22] = [
    LinterRules {
        language: "bash",
        linter: Err(LinterGap::FollowUp),
    },
    LinterRules {
        language: "c",
        linter: Err(LinterGap::FollowUp),
    },
    LinterRules {
        language: "cpp",
        linter: Err(LinterGap::FollowUp),
    },
    LinterRules {
        language: "csharp",
        linter: Err(LinterGap::TierTwo),
    },
    LinterRules {
        language: "go",
        linter: Err(LinterGap::FollowUp),
    },
    LinterRules {
        language: "hcl",
        linter: Err(LinterGap::DataLanguage),
    },
    LinterRules {
        language: "java",
        linter: Err(LinterGap::FollowUp),
    },
    LinterRules {
        language: "javascript",
        linter: Err(LinterGap::FollowUp),
    },
    LinterRules {
        language: "json",
        linter: Err(LinterGap::DataLanguage),
    },
    LinterRules {
        language: "kotlin",
        linter: Err(LinterGap::TierTwo),
    },
    LinterRules {
        language: "lua",
        linter: Err(LinterGap::FollowUp),
    },
    LinterRules {
        language: "php",
        linter: Err(LinterGap::TierTwo),
    },
    LinterRules {
        language: "python",
        linter: Ok(Linter {
            name: "ruff",
            local: ".venv/bin/ruff",
            program: "ruff",
            args: &[
                "check",
                "--force-exclude",
                "--stdin-filename",
                "{path}",
                "--output-format",
                "json",
                "--exit-zero",
                "-",
            ],
            configs: &["ruff.toml", ".ruff.toml"],
            manifest: "pyproject.toml",
            table: &["tool", "ruff"],
            syntax_code: "invalid-syntax",
        }),
    },
    LinterRules {
        language: "ruby",
        linter: Err(LinterGap::FollowUp),
    },
    LinterRules {
        language: "rust",
        linter: Err(LinterGap::FollowUp),
    },
    LinterRules {
        language: "scala",
        linter: Err(LinterGap::TierTwo),
    },
    LinterRules {
        language: "sql",
        linter: Err(LinterGap::DataLanguage),
    },
    LinterRules {
        language: "swift",
        linter: Err(LinterGap::TierTwo),
    },
    LinterRules {
        language: "toml",
        linter: Err(LinterGap::DataLanguage),
    },
    LinterRules {
        language: "tsx",
        linter: Err(LinterGap::FollowUp),
    },
    LinterRules {
        language: "typescript",
        linter: Err(LinterGap::FollowUp),
    },
    LinterRules {
        language: "yaml",
        linter: Err(LinterGap::DataLanguage),
    },
];
