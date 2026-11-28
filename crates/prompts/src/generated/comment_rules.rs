//! The comment rules, one record per language, in name order.
//! `cargo xtask generate` writes this file from the prompt JSON files.

use crate::comment_rule::{
    CommentGap, CommentRules, Declared, DocRules, Head, RunRules, Suppressed, SuppressionRules,
};

pub(crate) static COMMENT_RULES: [CommentRules; 22] = [
    CommentRules {
        language: "bash",
        runs: Declared::Rules(RunRules {
            line: "^#",
            block: None,
            doc: None,
            head: Head::Comment,
            directive: Some("^(?:#!|#\\s*shellcheck\\s+source=)"),
            banner: "^#\\s*(?:[-=~*#]{4,}|[-=~*#]{3,}.*[-=~*#]{3,})\\s*$",
        }),
        docs: Declared::Gap(CommentGap::FollowUp),
        suppressions: Declared::Gap(CommentGap::FollowUp),
    },
    CommentRules {
        language: "c",
        runs: Declared::Rules(RunRules {
            line: "^//",
            block: Some("^/\\*"),
            doc: Some(
                "^(?:///(?:[^/]|$)|//!(?:[^!]|$)|/\\*\\*(?:[^*]|$)|/\\*!(?:[^!]|$\
        ))",
            ),
            head: Head::FirstLine,
            directive: Some("^(?://|/\\*)\\s*clang-format\\s+(?:off|on)"),
            banner: "^(?://|/\\*)\\s*(?:[-=~*#/]{4,}|[-=~*#]{3,}.*[-=~*#]{3,})\\s*(?:\
        \\*/)?\\s*$",
        }),
        docs: Declared::Gap(CommentGap::FollowUp),
        suppressions: Declared::Gap(CommentGap::FollowUp),
    },
    CommentRules {
        language: "cpp",
        runs: Declared::Rules(RunRules {
            line: "^//",
            block: Some("^/\\*"),
            doc: Some(
                "^(?:///(?:[^/]|$)|//!(?:[^!]|$)|/\\*\\*(?:[^*]|$)|/\\*!(?:[^!]|$\
        ))",
            ),
            head: Head::FirstLine,
            directive: Some("^(?://|/\\*)\\s*clang-format\\s+(?:off|on)"),
            banner: "^(?://|/\\*)\\s*(?:[-=~*#/]{4,}|[-=~*#]{3,}.*[-=~*#]{3,})\\s*(?:\
        \\*/)?\\s*$",
        }),
        docs: Declared::Gap(CommentGap::FollowUp),
        suppressions: Declared::Gap(CommentGap::FollowUp),
    },
    CommentRules {
        language: "csharp",
        runs: Declared::Gap(CommentGap::TierTwo),
        docs: Declared::Gap(CommentGap::TierTwo),
        suppressions: Declared::Gap(CommentGap::TierTwo),
    },
    CommentRules {
        language: "go",
        runs: Declared::Gap(CommentGap::DocAttachment),
        docs: Declared::Gap(CommentGap::DocAttachment),
        suppressions: Declared::Gap(CommentGap::DocAttachment),
    },
    CommentRules {
        language: "hcl",
        runs: Declared::Gap(CommentGap::DataLanguage),
        docs: Declared::Gap(CommentGap::DataLanguage),
        suppressions: Declared::Gap(CommentGap::DataLanguage),
    },
    CommentRules {
        language: "java",
        runs: Declared::Rules(RunRules {
            line: "^//",
            block: Some("^/\\*"),
            doc: Some("^/\\*\\*(?:[^*]|$)"),
            head: Head::FirstLine,
            directive: None,
            banner: "^(?://|/\\*)\\s*(?:[-=~*#/]{4,}|[-=~*#]{3,}.*[-=~*#]{3,})\\s*(?:\
        \\*/)?\\s*$",
        }),
        docs: Declared::Gap(CommentGap::FollowUp),
        suppressions: Declared::Gap(CommentGap::FollowUp),
    },
    CommentRules {
        language: "javascript",
        runs: Declared::Rules(RunRules {
            line: "^//",
            block: Some("^/\\*"),
            doc: Some("^/\\*\\*"),
            head: Head::FirstLine,
            directive: Some(
                "^(?:///\\s*<|//\\s*@(?:flow|jsx|jsxImportSource|jsxRuntime|vites\
        t-environment|jest-environment)\\b)",
            ),
            banner: "^(?://|/\\*)\\s*(?:[-=~*#/]{4,}|[-=~*#]{3,}.*[-=~*#]{3,})\\s*(?:\
        \\*/)?\\s*$",
        }),
        docs: Declared::Rules(DocRules::Jsdoc {
            tags: "(?m)^\\s*\\*?\\s*@(param|arg|argument|returns?|throws)\\b",
            aliases: &[
                ("arg", "param"),
                ("argument", "param"),
                ("return", "returns"),
            ],
        }),
        suppressions: Declared::Rules(SuppressionRules {
            on: Suppressed::Comment,
            pattern: "^(?://|/\\*)\\s*(?:eslint-disable|@ts-ignore|@ts-nocheck|@ts-exp\
        ect-error|prettier-ignore|biome-ignore)",
        }),
    },
    CommentRules {
        language: "json",
        runs: Declared::Gap(CommentGap::DataLanguage),
        docs: Declared::Gap(CommentGap::DataLanguage),
        suppressions: Declared::Gap(CommentGap::DataLanguage),
    },
    CommentRules {
        language: "kotlin",
        runs: Declared::Gap(CommentGap::TierTwo),
        docs: Declared::Gap(CommentGap::TierTwo),
        suppressions: Declared::Gap(CommentGap::TierTwo),
    },
    CommentRules {
        language: "lua",
        runs: Declared::Rules(RunRules {
            line: "^--",
            block: Some("^--\\[=*\\["),
            doc: Some("^(?:---(?:[^-]|$)|--\\[\\[--)"),
            head: Head::FirstLine,
            directive: None,
            banner: "^(?:--|--\\[\\[)\\s*(?:[-=~*#/]{4,}|[-=~*#]{3,}.*[-=~*#]{3,})\\s\
        *(?:\\]\\])?\\s*$",
        }),
        docs: Declared::Gap(CommentGap::FollowUp),
        suppressions: Declared::Gap(CommentGap::FollowUp),
    },
    CommentRules {
        language: "php",
        runs: Declared::Gap(CommentGap::TierTwo),
        docs: Declared::Gap(CommentGap::TierTwo),
        suppressions: Declared::Gap(CommentGap::TierTwo),
    },
    CommentRules {
        language: "python",
        runs: Declared::Rules(RunRules {
            line: "^#",
            block: None,
            doc: None,
            head: Head::Comment,
            directive: Some("^#\\s*(?:!|-\\*-|type:|pragma:|fmt:|isort:)"),
            banner: "^#\\s*(?:[-=~*#]{4,}|[-=~*#]{3,}.*[-=~*#]{3,})\\s*$",
        }),
        docs: Declared::Rules(DocRules::Docstring {
            private: "^_",
            sections: "(?m)^\\s*(?:Args|Arguments|Parameters|Returns|Yields|Raises):\\s\
        *$",
        }),
        suppressions: Declared::Rules(SuppressionRules {
            on: Suppressed::Comment,
            pattern: "#\\s*(?:noqa\\b|nosec\\b|type:\\s*ignore\\b|(?:pylint|pyright|my\
        py|ruff):\\s*(?:disable|ignore|noqa))",
        }),
    },
    CommentRules {
        language: "ruby",
        runs: Declared::Gap(CommentGap::DocAttachment),
        docs: Declared::Gap(CommentGap::DocAttachment),
        suppressions: Declared::Gap(CommentGap::DocAttachment),
    },
    CommentRules {
        language: "rust",
        runs: Declared::Rules(RunRules {
            line: "^//",
            block: Some("^/\\*"),
            doc: None,
            head: Head::FirstLine,
            directive: None,
            banner: "^(?://[/!]?|/\\*[*!]?)\\s*(?:[-=~*#/]{4,}|[-=~*#]{3,}.*[-=~*#]{3\
        ,})\\s*(?:\\*/)?\\s*$",
        }),
        docs: Declared::Rules(DocRules::RustItems {
            item: "^(?P<visibility>pub\\b(?:\\s*\\([^)]*\\))?)?\\s*(?:(?:default|co\
        nst|async|unsafe|safe|extern)\\s+)*(?P<kind>fn|struct|enum|union\
        |trait|type|const|static|mod|use|impl|macro_rules!)?\\s*(?P<name\
        >\\$?(?:r#)?\\w+)?",
            public: "pub",
            export: "macro_export",
            facing: "\\bderive\\s*\\([^)]*\\b(?:Parser|Subcommand|Args|ValueEnum|Json\
        Schema)\\b|#\\[\\s*(?:pyclass|pymethods|pyfunction|pymodule|tool\
        |tool_router)\\b",
            sections: "(?m)^\\s*#\\s+(?:Arguments|Args|Parameters|Params|Returns?)\\s*$",
        }),
        suppressions: Declared::Rules(SuppressionRules {
            on: Suppressed::Attribute,
            pattern: "^(?:(?:allow|expect)\\s*\\(|rustfmt::skip\\b|cfg_attr\\s*\\(.*\\\
        b(?:allow|expect)\\s*\\()",
        }),
    },
    CommentRules {
        language: "scala",
        runs: Declared::Gap(CommentGap::TierTwo),
        docs: Declared::Gap(CommentGap::TierTwo),
        suppressions: Declared::Gap(CommentGap::TierTwo),
    },
    CommentRules {
        language: "sql",
        runs: Declared::Gap(CommentGap::DataLanguage),
        docs: Declared::Gap(CommentGap::DataLanguage),
        suppressions: Declared::Gap(CommentGap::DataLanguage),
    },
    CommentRules {
        language: "swift",
        runs: Declared::Gap(CommentGap::TierTwo),
        docs: Declared::Gap(CommentGap::TierTwo),
        suppressions: Declared::Gap(CommentGap::TierTwo),
    },
    CommentRules {
        language: "toml",
        runs: Declared::Gap(CommentGap::DataLanguage),
        docs: Declared::Gap(CommentGap::DataLanguage),
        suppressions: Declared::Gap(CommentGap::DataLanguage),
    },
    CommentRules {
        language: "tsx",
        runs: Declared::Rules(RunRules {
            line: "^//",
            block: Some("^/\\*"),
            doc: Some("^/\\*\\*"),
            head: Head::FirstLine,
            directive: Some(
                "^(?:///\\s*<|//\\s*@(?:flow|jsx|jsxImportSource|jsxRuntime|vites\
        t-environment|jest-environment)\\b)",
            ),
            banner: "^(?://|/\\*)\\s*(?:[-=~*#/]{4,}|[-=~*#]{3,}.*[-=~*#]{3,})\\s*(?:\
        \\*/)?\\s*$",
        }),
        docs: Declared::Rules(DocRules::Jsdoc {
            tags: "(?m)^\\s*\\*?\\s*@(param|arg|argument|returns?|throws)\\b",
            aliases: &[
                ("arg", "param"),
                ("argument", "param"),
                ("return", "returns"),
            ],
        }),
        suppressions: Declared::Rules(SuppressionRules {
            on: Suppressed::Comment,
            pattern: "^(?://|/\\*)\\s*(?:eslint-disable|@ts-ignore|@ts-nocheck|@ts-exp\
        ect-error|prettier-ignore|biome-ignore)",
        }),
    },
    CommentRules {
        language: "typescript",
        runs: Declared::Rules(RunRules {
            line: "^//",
            block: Some("^/\\*"),
            doc: Some("^/\\*\\*"),
            head: Head::FirstLine,
            directive: Some(
                "^(?:///\\s*<|//\\s*@(?:flow|jsx|jsxImportSource|jsxRuntime|vites\
        t-environment|jest-environment)\\b)",
            ),
            banner: "^(?://|/\\*)\\s*(?:[-=~*#/]{4,}|[-=~*#]{3,}.*[-=~*#]{3,})\\s*(?:\
        \\*/)?\\s*$",
        }),
        docs: Declared::Rules(DocRules::Jsdoc {
            tags: "(?m)^\\s*\\*?\\s*@(param|arg|argument|returns?|throws)\\b",
            aliases: &[
                ("arg", "param"),
                ("argument", "param"),
                ("return", "returns"),
            ],
        }),
        suppressions: Declared::Rules(SuppressionRules {
            on: Suppressed::Comment,
            pattern: "^(?://|/\\*)\\s*(?:eslint-disable|@ts-ignore|@ts-nocheck|@ts-exp\
        ect-error|prettier-ignore|biome-ignore)",
        }),
    },
    CommentRules {
        language: "yaml",
        runs: Declared::Gap(CommentGap::DataLanguage),
        docs: Declared::Gap(CommentGap::DataLanguage),
        suppressions: Declared::Gap(CommentGap::DataLanguage),
    },
];
