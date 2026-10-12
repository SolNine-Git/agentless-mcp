//! The contract rules, one record per rule and language, in file order.
//! `cargo xtask generate` writes this file from the prompt JSON files.

use crate::rule::{ContractRule, Severity};

pub(crate) static RULES: [ContractRule; 30] = [
    ContractRule {
        id: "get-default-empty",
        language: "python",
        severity: Severity::Warning,
        query: "(call function: (attribute attribute: (identifier) @_m) \
        arguments: (argument_list . (_) . (_) @_d .) (#eq? @_m \"get\") \
        (#any-of? @_d \"[]\" \"{}\" \"\\\"\\\"\" \"''\" \"0\")) @hit",
        message: "A missing key reads as an empty value, so a renamed or removed \
        field looks like no data. Read the field in one parse step that \
        raises when the key is missing.",
    },
    ContractRule {
        id: "or-empty-default",
        language: "python",
        severity: Severity::Warning,
        query: "(boolean_operator operator: \"or\" right: (_) @_d (#any-of? @_d \
        \"[]\" \"{}\")) @hit",
        message: "A missing or falsy value reads as an empty container, so a \
        changed response shape looks like no data. Check for the \
        missing value in the parse step and raise there.",
    },
    ContractRule {
        id: "getattr-default",
        language: "python",
        severity: Severity::Warning,
        query: "(call function: (identifier) @_f arguments: (argument_list . \
        (_) . (_) . (_) .) (#eq? @_f \"getattr\")) @hit",
        message: "A missing attribute reads as the default, so a renamed field \
        looks like a present one. Read the attribute directly, or check \
        for it in the parse step and raise there.",
    },
    ContractRule {
        id: "except-return-default",
        language: "python",
        severity: Severity::Warning,
        query: "(except_clause (block (return_statement (_) @_v) @hit) \
        (#any-of? @_v \"None\" \"False\" \"0\" \"[]\" \"{}\"))",
        message: "The handler turns an error into an ordinary value, so a failure \
        looks like an empty result. Let the error propagate, or raise a \
        named error that carries the context.",
    },
    ContractRule {
        id: "except-pass",
        language: "python",
        severity: Severity::Warning,
        query: "(except_clause [(block . (pass_statement) .) (block . \
        (pass_statement) . (comment) .)]) @hit",
        message: "The handler drops the error, so a failure looks like success. \
        Handle the error, or let it propagate.",
    },
    ContractRule {
        id: "nullish-empty-default",
        language: "javascript",
        severity: Severity::Warning,
        query: "(binary_expression operator: \"??\" right: (_) @_d (#any-of? \
        @_d \"[]\" \"{}\")) @hit",
        message: "A missing value reads as an empty container, so a renamed or \
        removed field looks like no data. Validate the response in one \
        parse step that throws when the field is missing.",
    },
    ContractRule {
        id: "nullish-empty-default",
        language: "typescript",
        severity: Severity::Warning,
        query: "(binary_expression operator: \"??\" right: (_) @_d (#any-of? \
        @_d \"[]\" \"{}\")) @hit",
        message: "A missing value reads as an empty container, so a renamed or \
        removed field looks like no data. Validate the response in one \
        parse step that throws when the field is missing.",
    },
    ContractRule {
        id: "nullish-empty-default",
        language: "tsx",
        severity: Severity::Warning,
        query: "(binary_expression operator: \"??\" right: (_) @_d (#any-of? \
        @_d \"[]\" \"{}\")) @hit",
        message: "A missing value reads as an empty container, so a renamed or \
        removed field looks like no data. Validate the response in one \
        parse step that throws when the field is missing.",
    },
    ContractRule {
        id: "or-empty-default",
        language: "javascript",
        severity: Severity::Warning,
        query: "(binary_expression operator: \"||\" right: (_) @_d (#any-of? \
        @_d \"[]\" \"{}\")) @hit",
        message: "A missing or falsy value reads as an empty container, so a \
        renamed or removed field looks like no data. Validate the \
        response in one parse step that throws when the field is \
        missing.",
    },
    ContractRule {
        id: "or-empty-default",
        language: "typescript",
        severity: Severity::Warning,
        query: "(binary_expression operator: \"||\" right: (_) @_d (#any-of? \
        @_d \"[]\" \"{}\")) @hit",
        message: "A missing or falsy value reads as an empty container, so a \
        renamed or removed field looks like no data. Validate the \
        response in one parse step that throws when the field is \
        missing.",
    },
    ContractRule {
        id: "or-empty-default",
        language: "tsx",
        severity: Severity::Warning,
        query: "(binary_expression operator: \"||\" right: (_) @_d (#any-of? \
        @_d \"[]\" \"{}\")) @hit",
        message: "A missing or falsy value reads as an empty container, so a \
        renamed or removed field looks like no data. Validate the \
        response in one parse step that throws when the field is \
        missing.",
    },
    ContractRule {
        id: "catch-return-default",
        language: "javascript",
        severity: Severity::Warning,
        query: "(catch_clause body: (statement_block (return_statement (_) @_v) \
        @hit) (#any-of? @_v \"null\" \"undefined\" \"false\" \"0\" \
        \"[]\" \"{}\"))",
        message: "The handler turns an error into an ordinary value, so a failure \
        looks like an empty result. Rethrow the error, or throw a named \
        error that carries the context.",
    },
    ContractRule {
        id: "catch-return-default",
        language: "typescript",
        severity: Severity::Warning,
        query: "(catch_clause body: (statement_block (return_statement (_) @_v) \
        @hit) (#any-of? @_v \"null\" \"undefined\" \"false\" \"0\" \
        \"[]\" \"{}\"))",
        message: "The handler turns an error into an ordinary value, so a failure \
        looks like an empty result. Rethrow the error, or throw a named \
        error that carries the context.",
    },
    ContractRule {
        id: "catch-return-default",
        language: "tsx",
        severity: Severity::Warning,
        query: "(catch_clause body: (statement_block (return_statement (_) @_v) \
        @hit) (#any-of? @_v \"null\" \"undefined\" \"false\" \"0\" \
        \"[]\" \"{}\"))",
        message: "The handler turns an error into an ordinary value, so a failure \
        looks like an empty result. Rethrow the error, or throw a named \
        error that carries the context.",
    },
    ContractRule {
        id: "catch-empty",
        language: "javascript",
        severity: Severity::Warning,
        query: "(catch_clause body: (statement_block) @_b (#any-of? @_b \
        \"{}\")) @hit\n(catch_clause body: (statement_block . (comment) \
        .)) @hit",
        message: "The handler drops the error, so a failure looks like success. \
        Handle the error, or let it propagate.",
    },
    ContractRule {
        id: "catch-empty",
        language: "typescript",
        severity: Severity::Warning,
        query: "(catch_clause body: (statement_block) @_b (#any-of? @_b \
        \"{}\")) @hit\n(catch_clause body: (statement_block . (comment) \
        .)) @hit",
        message: "The handler drops the error, so a failure looks like success. \
        Handle the error, or let it propagate.",
    },
    ContractRule {
        id: "catch-empty",
        language: "tsx",
        severity: Severity::Warning,
        query: "(catch_clause body: (statement_block) @_b (#any-of? @_b \
        \"{}\")) @hit\n(catch_clause body: (statement_block . (comment) \
        .)) @hit",
        message: "The handler drops the error, so a failure looks like success. \
        Handle the error, or let it propagate.",
    },
    ContractRule {
        id: "err-check-returns-nil",
        language: "go",
        severity: Severity::Warning,
        query: "(if_statement condition: (binary_expression left: (identifier) \
        @_e operator: \"!=\" right: (nil)) consequence: (block \
        (statement_list (return_statement (expression_list (_) @_last \
        .)) @hit)) (#eq? @_e \"err\") (#eq? @_last \"nil\"))",
        message: "The branch sees an error and returns nil as the last value, so \
        the caller sees success. Return the error, wrapped with its \
        context.",
    },
    ContractRule {
        id: "unwrap-or-default",
        language: "rust",
        severity: Severity::Warning,
        query: "(call_expression function: (field_expression field: \
        (field_identifier) @_m) (#eq? @_m \"unwrap_or_default\")) @hit",
        message: "A None or an Err reads as the type's default value, so a \
        missing field looks like an empty one. Return the error with ?, \
        or handle the None case.",
    },
    ContractRule {
        id: "unwrap-or-empty",
        language: "rust",
        severity: Severity::Warning,
        query: "(call_expression function: (field_expression field: \
        (field_identifier) @_m) arguments: (arguments . (_) @_d .) \
        (#eq? @_m \"unwrap_or\") (#any-of? @_d \"vec![]\" \
        \"Vec::new()\" \"String::new()\" \"HashMap::new()\" \
        \"HashSet::new()\" \"BTreeMap::new()\" \"BTreeSet::new()\" \
        \"VecDeque::new()\" \"\\\"\\\"\" \"0\" \"Default::default()\")) \
        @hit",
        message: "A None or an Err reads as an empty value, so a missing field \
        looks like no data. Return the error with ?, or handle the None \
        case.",
    },
    ContractRule {
        id: "ok-statement",
        language: "rust",
        severity: Severity::Warning,
        query: "(expression_statement (call_expression function: \
        (field_expression field: (field_identifier) @_m) arguments: \
        (arguments)) @hit (#eq? @_m \"ok\"))",
        message: "The statement turns a Result into an Option and drops it, so \
        the error is never seen. Handle the Result, or return the error \
        with ?.",
    },
    ContractRule {
        id: "catch-empty",
        language: "java",
        severity: Severity::Warning,
        query: "(catch_clause body: (block) @_b (#any-of? @_b \"{}\")) \
        @hit\n(catch_clause body: (block . [(line_comment) \
        (block_comment)] .)) @hit",
        message: "The handler drops the exception, so a failure looks like \
        success. Handle the exception, or let it propagate.",
    },
    ContractRule {
        id: "catch-return-null",
        language: "java",
        severity: Severity::Warning,
        query: "(catch_clause body: (block (return_statement (null_literal)) \
        @hit))",
        message: "The handler turns an exception into null, so a failure looks \
        like a missing value. Rethrow the exception, or throw a named \
        exception that carries the context.",
    },
    ContractRule {
        id: "get-or-default-empty",
        language: "java",
        severity: Severity::Warning,
        query: "(method_invocation name: (identifier) @_m arguments: \
        (argument_list . (_) . (_) @_d .) (#eq? @_m \"getOrDefault\") \
        (#any-of? @_d \"\\\"\\\"\" \"0\" \"List.of()\" \"Set.of()\" \
        \"Map.of()\" \"Collections.emptyList()\" \
        \"Collections.emptySet()\" \"Collections.emptyMap()\" \"new \
        ArrayList<>()\" \"new HashMap<>()\" \"new HashSet<>()\" \"new \
        LinkedList<>()\" \"new TreeMap<>()\" \"new TreeSet<>()\")) @hit",
        message: "A missing key reads as an empty value, so a renamed or removed \
        field looks like no data. Check for the key in the parse step \
        and throw there.",
    },
    ContractRule {
        id: "rescue-nil",
        language: "ruby",
        severity: Severity::Warning,
        query: "(rescue_modifier handler: (nil)) @hit\n(rescue body: (then . \
        (nil) .)) @hit",
        message: "The rescue turns an error into nil, so a failure looks like a \
        missing value. Rescue a named error class and handle it, or let \
        the error propagate.",
    },
    ContractRule {
        id: "fetch-default-empty",
        language: "ruby",
        severity: Severity::Warning,
        query: "(call method: (identifier) @_m arguments: (argument_list . (_) \
        . (_) @_d .) (#eq? @_m \"fetch\") (#any-of? @_d \"[]\" \"{}\" \
        \"\\\"\\\"\" \"''\" \"0\")) @hit",
        message: "A missing key reads as an empty value, so fetch no longer \
        raises KeyError for it. Call fetch without a default, so a \
        missing key raises.",
    },
    ContractRule {
        id: "or-empty-default",
        language: "ruby",
        severity: Severity::Warning,
        query: "(binary operator: \"||\" right: (_) @_d (#any-of? @_d \"[]\" \
        \"{}\")) @hit",
        message: "A nil or false value reads as an empty container, so a renamed \
        or removed field looks like no data. Check for the missing \
        value in the parse step and raise there.",
    },
    ContractRule {
        id: "or-empty-table",
        language: "lua",
        severity: Severity::Warning,
        query: "(binary_expression left: (_) \"or\" right: (table_constructor) \
        @_d (#any-of? @_d \"{}\")) @hit",
        message: "A nil or false value reads as an empty table, so a missing \
        field looks like no data. Check for the missing value in the \
        parse step and raise an error there.",
    },
    ContractRule {
        id: "pcall-statement",
        language: "lua",
        severity: Severity::Warning,
        query: "(chunk (function_call name: (identifier) @_f) @hit (#eq? @_f \
        \"pcall\"))\n(block (function_call name: (identifier) @_f) @hit \
        (#eq? @_f \"pcall\"))",
        message: "The statement discards the status that pcall returns, so an \
        error is never seen. Read the status and the error, and handle \
        them.",
    },
    ContractRule {
        id: "catch-all-empty",
        language: "cpp",
        severity: Severity::Warning,
        query: "(catch_clause parameters: (parameter_list \"...\") body: \
        (compound_statement) @_b (#any-of? @_b \"{}\")) \
        @hit\n(catch_clause parameters: (parameter_list \"...\") body: \
        (compound_statement . (comment) .)) @hit",
        message: "The handler catches every exception and drops it, so a failure \
        looks like success. Catch the exception types you can handle, \
        or let them propagate.",
    },
];
