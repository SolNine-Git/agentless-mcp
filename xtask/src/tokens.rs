use std::collections::BTreeSet;
use std::str::FromStr;

use proc_macro2::{Delimiter, Group, Ident, TokenStream, TokenTree};

type Bound = BTreeSet<String>;
type Test = fn(&[TokenTree], usize) -> bool;

const RULES: [(Rule, Test); 7] = [
    (Rule::LoneHash, lone_hash),
    (Rule::FragmentCall, fragment_call),
    (Rule::TtFragment, tt_fragment),
    (Rule::NestedDefinition, nested_definition),
    (Rule::UseFragment, use_fragment),
    (Rule::Cfg, cfg_attribute),
    (Rule::Cfg, cfg_macro),
];

/// A name and the 1-based line it starts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub line: usize,
    pub name: String,
}

/// One item of an attribute's delimited list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Path(String),
    Assignment,
    Unreadable(String),
}

/// An attribute item with `cfg_attr` unwrapped: its path and its list, if it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    pub line: usize,
    pub path: String,
    pub list: Option<Vec<Item>>,
}

/// A form that the gates refuse, and the 1-based line it starts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refused {
    pub line: usize,
    pub rule: Rule,
}

/// Why the gates refuse a form: each rule closes one route to code or an attribute that a gate does not see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    Shebang,
    LoneHash,
    FragmentCall,
    TtFragment,
    NestedDefinition,
    UseFragment,
    Cfg,
    IncludeName,
}

/// Everything the gates read from one file's tokens, at every depth.
#[derive(Debug, Default)]
pub struct Scan {
    pub attributes: Vec<Meta>,
    pub unreadable: Vec<Located>,
    pub macro_calls: Vec<Located>,
    pub macro_definitions: Vec<Located>,
    pub use_names: Vec<Located>,
    pub words: Vec<Located>,
    pub refused: Vec<Refused>,
}

enum Shape {
    Binding,
    Fragment(String),
    Meta { path: String, list: Option<Group> },
    Unreadable,
}

/// Tokenize `text` with proc-macro2 and collect its attributes, macros, use names, words and refused forms.
pub fn scan(text: &str) -> Result<Scan, String> {
    let (body, shebang) = strip_shebang(text);
    let stream = TokenStream::from_str(body).map_err(|error| {
        let at = error.span().start();
        format!("{}:{}: {error}", at.line, at.column + 1)
    })?;
    let mut scan = Scan::default();
    walk(stream.clone(), &Bound::new(), &mut scan);
    scan.refused = shebang
        .into_iter()
        .chain(refusals(stream.clone()))
        .chain(include_names(stream))
        .collect();
    scan.refused.sort_by_key(|refused| refused.line);
    Ok(scan)
}

fn strip_shebang(text: &str) -> (&str, Option<Refused>) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    // rustc skips a first line that starts `#!` unless an attribute follows. Refusing every `#!`
    // without `[` right after it also refuses rustc's rare `#! [..]`, so the check fails closed.
    match text.strip_prefix("#!") {
        Some(rest) if !rest.starts_with('[') => {
            let body = rest.find('\n').map_or("", |at| &rest[at..]);
            let refused = Refused {
                line: 1,
                rule: Rule::Shebang,
            };
            (body, Some(refused))
        }
        _ => (text, None),
    }
}

fn walk(stream: TokenStream, bound: &Bound, scan: &mut Scan) {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    let mut index = 0;
    while index < tokens.len() {
        index = match &tokens[index] {
            TokenTree::Punct(punct) if punct.as_char() == '#' => {
                attribute_at(&tokens, index, bound, scan)
            }
            TokenTree::Ident(_) => ident_at(&tokens, index, scan),
            TokenTree::Group(group) => {
                walk(group.stream(), bound, scan);
                index + 1
            }
            TokenTree::Punct(_) | TokenTree::Literal(_) => index + 1,
        };
    }
}

fn attribute_at(tokens: &[TokenTree], index: usize, bound: &Bound, scan: &mut Scan) -> usize {
    let Some((at, group)) = attribute_group(tokens, index) else {
        return index + 1;
    };
    let content: Vec<TokenTree> = group.stream().into_iter().collect();
    item(&content, line_of(&tokens[index]), bound, scan);
    walk(group.stream(), bound, scan);
    at + 1
}

fn attribute_group(tokens: &[TokenTree], index: usize) -> Option<(usize, &Group)> {
    let at = if is_punct(tokens.get(index + 1), '!') {
        index + 2
    } else {
        index + 1
    };
    match tokens.get(at) {
        Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Bracket => {
            Some((at, group))
        }
        _ => None,
    }
}

fn item(tokens: &[TokenTree], line: usize, bound: &Bound, scan: &mut Scan) {
    match shape(tokens) {
        Shape::Binding => {}
        Shape::Fragment(name) if bound.contains(&name) => {}
        Shape::Meta {
            path,
            list: Some(group),
        } if path == "cfg_attr" => {
            for part in split(group.stream()).iter().skip(1) {
                item(part, line, bound, scan);
            }
        }
        Shape::Meta { path, list } => scan.attributes.push(Meta {
            line,
            path,
            list: list.map(|group| {
                split(group.stream())
                    .iter()
                    .map(|part| list_item(part))
                    .collect()
            }),
        }),
        Shape::Fragment(_) | Shape::Unreadable => scan.unreadable.push(Located {
            line,
            name: text(tokens),
        }),
    }
}

fn shape(tokens: &[TokenTree]) -> Shape {
    if is_punct(tokens.first(), '$') {
        return fragment_shape(&tokens[1..]);
    }
    let (segments, used) = path(tokens);
    if segments.is_empty() {
        return Shape::Unreadable;
    }
    let path = segments.join("::");
    match &tokens[used..] {
        [] => Shape::Meta { path, list: None },
        [TokenTree::Group(group)] => Shape::Meta {
            path,
            list: Some(group.clone()),
        },
        [TokenTree::Punct(punct), ..] if punct.as_char() == '=' => Shape::Meta { path, list: None },
        _ => Shape::Unreadable,
    }
}

fn fragment_shape(rest: &[TokenTree]) -> Shape {
    match rest {
        [TokenTree::Ident(name)] => Shape::Fragment(plain(name)),
        [
            TokenTree::Ident(_),
            TokenTree::Punct(colon),
            TokenTree::Ident(_),
        ] if colon.as_char() == ':' => Shape::Binding,
        _ => Shape::Unreadable,
    }
}

fn list_item(part: &[TokenTree]) -> Item {
    if part.iter().any(|token| is_punct(Some(token), '=')) {
        return Item::Assignment;
    }
    let (segments, used) = path(part);
    if !segments.is_empty() && used == part.len() {
        Item::Path(segments.join("::"))
    } else {
        Item::Unreadable(text(part))
    }
}

fn split(stream: TokenStream) -> Vec<Vec<TokenTree>> {
    let mut parts = vec![Vec::new()];
    for token in stream {
        if is_punct(Some(&token), ',') {
            parts.push(Vec::new());
        } else if let Some(last) = parts.last_mut() {
            last.push(token);
        }
    }
    parts.retain(|part| !part.is_empty());
    parts
}

fn path(tokens: &[TokenTree]) -> (Vec<String>, usize) {
    let mut segments = Vec::new();
    let mut used = 0;
    while let Some(TokenTree::Ident(ident)) = tokens.get(used) {
        segments.push(plain(ident));
        used += 1;
        let separated = is_punct(tokens.get(used), ':') && is_punct(tokens.get(used + 1), ':');
        if !(separated && matches!(tokens.get(used + 2), Some(TokenTree::Ident(_)))) {
            break;
        }
        used += 2;
    }
    (segments, used)
}

fn ident_at(tokens: &[TokenTree], index: usize, scan: &mut Scan) -> usize {
    let TokenTree::Ident(ident) = &tokens[index] else {
        return index + 1;
    };
    let word = located(ident);
    if word.name == "macro_rules"
        && let Some(next) = definition_at(tokens, index, &word, scan)
    {
        return next;
    }
    if is_call(tokens, index) {
        scan.macro_calls.push(word.clone());
    }
    for token in use_item(tokens, index).unwrap_or_default() {
        identifiers(token, &mut scan.use_names);
    }
    scan.words.push(word);
    index + 1
}

fn is_call(tokens: &[TokenTree], index: usize) -> bool {
    is_punct(tokens.get(index + 1), '!')
        && matches!(tokens.get(index + 2), Some(TokenTree::Group(_)))
}

fn use_item(tokens: &[TokenTree], index: usize) -> Option<&[TokenTree]> {
    let word = matches!(tokens.get(index), Some(TokenTree::Ident(ident)) if plain(ident) == "use");
    // `use<..>` is a precise-capturing bound, not a use item.
    if !word || is_punct(tokens.get(index + 1), '<') {
        return None;
    }
    let rest = &tokens[index + 1..];
    let end = rest
        .iter()
        .position(|token| is_punct(Some(token), ';'))
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

fn definition_at(
    tokens: &[TokenTree],
    index: usize,
    word: &Located,
    scan: &mut Scan,
) -> Option<usize> {
    let (true, Some(TokenTree::Ident(name)), Some(TokenTree::Group(body))) = (
        is_punct(tokens.get(index + 1), '!'),
        tokens.get(index + 2),
        tokens.get(index + 3),
    ) else {
        return None;
    };
    let name = plain(name);
    scan.words.push(word.clone());
    if let Some(rules) = rules(body.stream()) {
        for (matcher, transcriber) in rules {
            let bound = bindings(matcher.stream());
            walk(matcher.stream(), &bound, scan);
            walk(transcriber.stream(), &bound, scan);
        }
    } else {
        scan.unreadable.push(Located {
            line: word.line,
            name: format!("macro_rules! {name}"),
        });
        walk(body.stream(), &Bound::new(), scan);
    }
    scan.macro_definitions.push(Located {
        line: word.line,
        name,
    });
    Some(index + 4)
}

fn rules(body: TokenStream) -> Option<Vec<(Group, Group)>> {
    let tokens: Vec<TokenTree> = body.into_iter().collect();
    let mut rules = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let (Some(TokenTree::Group(matcher)), true, true, Some(TokenTree::Group(transcriber))) = (
            tokens.get(index),
            is_punct(tokens.get(index + 1), '='),
            is_punct(tokens.get(index + 2), '>'),
            tokens.get(index + 3),
        ) else {
            return None;
        };
        rules.push((matcher.clone(), transcriber.clone()));
        index += 4;
        if is_punct(tokens.get(index), ';') {
            index += 1;
        }
    }
    Some(rules)
}

fn bindings(stream: TokenStream) -> Bound {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    let mut bound = Bound::new();
    for (index, token) in tokens.iter().enumerate() {
        let TokenTree::Group(group) = token else {
            continue;
        };
        if follows_a_hash(&tokens[..index]) {
            bound.extend(meta_binding(group));
        }
        bound.extend(bindings(group.stream()));
    }
    bound
}

fn follows_a_hash(before: &[TokenTree]) -> bool {
    match before {
        [.., hash, bang] if is_punct(Some(bang), '!') => is_punct(Some(hash), '#'),
        [.., hash] => is_punct(Some(hash), '#'),
        [] => false,
    }
}

fn meta_binding(group: &Group) -> Option<String> {
    let tokens: Vec<TokenTree> = group.stream().into_iter().collect();
    match tokens.as_slice() {
        [
            TokenTree::Punct(dollar),
            TokenTree::Ident(name),
            TokenTree::Punct(colon),
            TokenTree::Ident(kind),
        ] if group.delimiter() == Delimiter::Bracket
            && dollar.as_char() == '$'
            && colon.as_char() == ':'
            && kind == "meta" =>
        {
            Some(plain(name))
        }
        _ => None,
    }
}

fn identifiers(token: &TokenTree, into: &mut Vec<Located>) {
    match token {
        TokenTree::Ident(ident) => into.push(located(ident)),
        TokenTree::Group(group) => {
            for inner in group.stream() {
                identifiers(&inner, into);
            }
        }
        TokenTree::Punct(_) | TokenTree::Literal(_) => {}
    }
}

fn refusals(stream: TokenStream) -> Vec<Refused> {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    let mut refused = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        for (rule, test) in RULES {
            if test(&tokens, index) {
                let line = line_of(token);
                refused.push(Refused { line, rule });
            }
        }
        if let TokenTree::Group(group) = token {
            refused.extend(refusals(group.stream()));
        }
    }
    refused
}

fn lone_hash(tokens: &[TokenTree], index: usize) -> bool {
    is_punct(tokens.get(index), '#') && attribute_group(tokens, index).is_none()
}

fn fragment_call(tokens: &[TokenTree], index: usize) -> bool {
    let named = matches!(
        tokens.get(index + 1),
        Some(TokenTree::Group(_) | TokenTree::Ident(_))
    );
    let before = &tokens[..index];
    is_punct(tokens.get(index), '!')
        && named
        && (after_fragment(before) || after_repetition(before))
}

fn after_fragment(before: &[TokenTree]) -> bool {
    matches!(before, [.., dollar, TokenTree::Ident(name)]
        if is_punct(Some(dollar), '$') && plain(name) != "crate")
}

// A repetition `$(..) sep? op` ends in `*`, `+` or `?`. A separator is one rustc token, which
// proc-macro2 splits into at most three punctuation tokens.
fn after_repetition(before: &[TokenTree]) -> bool {
    let Some((op, rest)) = before.split_last() else {
        return false;
    };
    let kleene = ['*', '+', '?']
        .into_iter()
        .any(|character| is_punct(Some(op), character));
    let group = rest
        .iter()
        .rposition(|token| matches!(token, TokenTree::Group(_)));
    kleene && group.is_some_and(|at| rest.len() - at <= 4 && opens_a_repetition(&rest[..=at]))
}

fn opens_a_repetition(through_group: &[TokenTree]) -> bool {
    matches!(through_group, [.., dollar, TokenTree::Group(group)]
        if is_punct(Some(dollar), '$') && group.delimiter() == Delimiter::Parenthesis)
}

fn tt_fragment(tokens: &[TokenTree], index: usize) -> bool {
    matches!(
        tokens.get(index..index + 4),
        Some([TokenTree::Punct(dollar), TokenTree::Ident(_), TokenTree::Punct(colon), TokenTree::Ident(kind)])
            if dollar.as_char() == '$' && colon.as_char() == ':' && plain(kind) == "tt"
    )
}

fn nested_definition(tokens: &[TokenTree], index: usize) -> bool {
    match tokens.get(index..index + 4) {
        Some([TokenTree::Ident(word), bang, _, TokenTree::Group(body)])
            if plain(word) == "macro_rules" && is_punct(Some(bang), '!') =>
        {
            defines_a_macro(body.stream())
        }
        _ => false,
    }
}

fn defines_a_macro(stream: TokenStream) -> bool {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    tokens.iter().enumerate().any(|(index, token)| match token {
        TokenTree::Ident(word) => {
            plain(word) == "macro_rules" && is_punct(tokens.get(index + 1), '!')
        }
        TokenTree::Group(group) => defines_a_macro(group.stream()),
        TokenTree::Punct(_) | TokenTree::Literal(_) => false,
    })
}

fn use_fragment(tokens: &[TokenTree], index: usize) -> bool {
    use_item(tokens, index).is_some_and(holds_a_fragment)
}

fn holds_a_fragment(tokens: &[TokenTree]) -> bool {
    tokens.iter().enumerate().any(|(index, token)| match token {
        TokenTree::Punct(dollar) => {
            dollar.as_char() == '$'
                && !matches!(tokens.get(index + 1), Some(TokenTree::Ident(name)) if plain(name) == "crate")
        }
        TokenTree::Group(group) => {
            holds_a_fragment(&group.stream().into_iter().collect::<Vec<TokenTree>>())
        }
        TokenTree::Ident(_) | TokenTree::Literal(_) => false,
    })
}

fn cfg_attribute(tokens: &[TokenTree], index: usize) -> bool {
    is_punct(tokens.get(index), '#')
        && attribute_group(tokens, index).is_some_and(|(_, group)| {
            hides_from_clippy(&group.stream().into_iter().collect::<Vec<TokenTree>>())
        })
}

// clippy sets the cfg `clippy`, so a predicate that names it, or a fragment that can, removes code
// from clippy alone.
fn hides_from_clippy(meta: &[TokenTree]) -> bool {
    let (segments, used) = path(meta);
    let Some(TokenTree::Group(list)) = meta.get(used) else {
        return false;
    };
    match segments.last().map(String::as_str) {
        Some("cfg") => names_clippy_or_a_fragment(list.stream()),
        Some("cfg_attr") => {
            split(list.stream())
                .split_first()
                .is_some_and(|(predicate, attributes)| {
                    names_clippy_or_a_fragment(predicate.clone())
                        || attributes.iter().any(|part| hides_from_clippy(part))
                })
        }
        _ => false,
    }
}

fn cfg_macro(tokens: &[TokenTree], index: usize) -> bool {
    match tokens.get(index..index + 3) {
        Some([TokenTree::Ident(name), bang, TokenTree::Group(predicate)])
            if plain(name) == "cfg" && is_punct(Some(bang), '!') =>
        {
            names_clippy_or_a_fragment(predicate.stream())
        }
        _ => false,
    }
}

fn names_clippy_or_a_fragment(tokens: impl IntoIterator<Item = TokenTree>) -> bool {
    tokens.into_iter().any(|token| match token {
        TokenTree::Ident(ident) => plain(&ident) == "clippy",
        TokenTree::Punct(punct) => punct.as_char() == '$',
        TokenTree::Group(group) => names_clippy_or_a_fragment(group.stream()),
        TokenTree::Literal(_) => false,
    })
}

// A literal include! name and a name in a use item have their own refusals, so they are skipped.
fn include_names(stream: TokenStream) -> Vec<Refused> {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    let mut refused = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        if let Some(item) = use_item(&tokens, index) {
            index += 1 + item.len();
            continue;
        }
        match &tokens[index] {
            TokenTree::Ident(name) if plain(name) == "include" && !is_call(&tokens, index) => {
                let line = line_of(&tokens[index]);
                let rule = Rule::IncludeName;
                refused.push(Refused { line, rule });
            }
            TokenTree::Group(group) => refused.extend(include_names(group.stream())),
            _ => {}
        }
        index += 1;
    }
    refused
}

fn is_punct(token: Option<&TokenTree>, character: char) -> bool {
    matches!(token, Some(TokenTree::Punct(punct)) if punct.as_char() == character)
}

fn plain(ident: &Ident) -> String {
    let text = ident.to_string();
    match text.strip_prefix("r#") {
        Some(name) => name.to_owned(),
        None => text,
    }
}

fn located(ident: &Ident) -> Located {
    Located {
        line: ident.span().start().line,
        name: plain(ident),
    }
}

fn line_of(token: &TokenTree) -> usize {
    token.span().start().line
}

fn text(tokens: &[TokenTree]) -> String {
    tokens.iter().cloned().collect::<TokenStream>().to_string()
}

#[cfg(test)]
mod tests {
    use super::{Item, Located, Meta, Refused, Rule, Scan, scan};

    const TYPED_ID: &str = include_str!("../../crates/kernel/src/id.rs");

    fn scanned(text: &str) -> Scan {
        scan(text).unwrap()
    }

    fn refused(text: &str) -> Vec<(usize, Rule)> {
        refused_of(&scanned(text))
    }

    fn refused_of(found: &Scan) -> Vec<(usize, Rule)> {
        found
            .refused
            .iter()
            .map(|refused| (refused.line, refused.rule))
            .collect()
    }

    // Each form must draw `rule` on exactly its lines. Each clean text, and typed_id!, draws nothing.
    fn assert_rule(rule: Rule, forms: &[(&str, &[usize])], clean: &[&str]) {
        for &(text, lines) in forms {
            let found: Vec<usize> = refused(text)
                .into_iter()
                .filter(|&(_, found)| found == rule)
                .map(|(line, _)| line)
                .collect();
            assert_eq!(found, lines, "{text}");
        }
        for text in clean.iter().chain([&TYPED_ID]) {
            assert_eq!(refused(text), [], "{text}");
        }
    }

    fn meta(line: usize, path: &str, list: Vec<Item>) -> Meta {
        Meta {
            line,
            path: path.to_owned(),
            list: Some(list),
        }
    }

    fn paths(names: &[&str]) -> Vec<Item> {
        names
            .iter()
            .map(|name| Item::Path((*name).to_owned()))
            .collect()
    }

    fn named(found: &[Located], name: &str) -> Vec<usize> {
        found
            .iter()
            .filter(|located| located.name == name)
            .map(|located| located.line)
            .collect()
    }

    #[test]
    fn a_comment_before_the_arguments_does_not_hide_them() {
        let found = scanned("#[expect/*c*/(dead_code)]\nfn f() {}\n");
        assert_eq!(found.attributes, [meta(1, "expect", paths(&["dead_code"]))]);
    }

    #[test]
    fn a_comment_or_a_comma_inside_a_comment_does_not_split_a_lint_list() {
        let found = scanned("#[allow(dead_code /* a, b */, unused, reason = \"x\")]\nfn f() {}\n");
        let list = vec![
            Item::Path("dead_code".to_owned()),
            Item::Path("unused".to_owned()),
            Item::Assignment,
        ];
        assert_eq!(found.attributes, [meta(1, "allow", list)]);
    }

    #[test]
    fn a_raw_identifier_reads_as_the_plain_name() {
        let found = scanned("#[r#allow(r#dead_code)]\nfn f() { r#include!(\"x\"); r#unsafe(); }\n");
        assert_eq!(found.attributes, [meta(1, "allow", paths(&["dead_code"]))]);
        assert_eq!(named(&found.macro_calls, "include"), [2]);
        assert_eq!(named(&found.words, "unsafe"), [2]);
    }

    #[test]
    fn cfg_attr_is_unwrapped_at_every_depth() {
        let found = scanned("fn f() {}\n#![cfg_attr(a, cfg_attr(b, allow(x)), expect(y))]\n");
        assert_eq!(
            found.attributes,
            [
                meta(2, "allow", paths(&["x"])),
                meta(2, "expect", paths(&["y"]))
            ]
        );
    }

    #[test]
    fn attributes_in_macro_bodies_and_macro_arguments_are_found() {
        let found = scanned(
            "macro_rules! m { () => { #[allow(y)] fn f() {} }; }\nm! { #[allow(z)] fn g() {} }\n",
        );
        assert_eq!(
            found.attributes,
            [
                meta(1, "allow", paths(&["y"])),
                meta(2, "allow", paths(&["z"]))
            ]
        );
    }

    #[test]
    fn doc_comments_strings_and_comments_hold_no_attributes() {
        let found = scanned(
            "/// #[allow(a)]\n//! #![allow(b)]\n/** #[allow(c)] */\nconst D: &str = \"#[allow(d)]\";\n\
             const E: &str = r#\"#[allow(e)]\"#;\n// #[allow(f)]\n/* #[allow(g)] */\n",
        );
        let paths: Vec<&str> = found
            .attributes
            .iter()
            .map(|meta| meta.path.as_str())
            .collect();
        assert_eq!(paths, ["doc", "doc", "doc"]);
        assert!(found.attributes.iter().all(|meta| meta.list.is_none()));
    }

    #[test]
    fn a_fragment_passes_only_through_an_attribute_matcher() {
        let typed = "macro_rules! typed { ($(#[$doc:meta])* $name:ident) => { $(#[$doc])* struct $name; }; }\n";
        let found = scanned(typed);
        assert!(found.unreadable.is_empty());
        assert!(found.attributes.is_empty());
        let found = scanned(
            "macro_rules! quiet { ($a:meta) => { #[$a] fn f() {} }; }\n\
             macro_rules! loud { ($($t:tt)*) => { #[$($t)*] fn f() {} }; }\n\
             macro_rules! gated { ($a:meta) => { #[cfg_attr(x, $a)] fn f() {} }; }\n\
             macro_rules! bare { ($a:meta) => { [$a] }; ([$b:meta]) => { #[$b] fn f() {} }; }\n\
             #[$c]\nfn g() {}\n",
        );
        let lines: Vec<usize> = found
            .unreadable
            .iter()
            .map(|located| located.line)
            .collect();
        assert_eq!(lines, [1, 2, 3, 4, 5]);
        let found = scanned("macro_rules! named { ($l:ident) => { #[expect($l)] fn f() {} }; }\n");
        assert!(matches!(
            found.attributes[0].list.as_deref(),
            Some([Item::Unreadable(_)])
        ));
    }

    #[test]
    fn path_attributes_are_found_inside_cfg_attr() {
        let found = scanned(
            "#[path = \"b.txt\"]\nmod b;\n#[cfg_attr(x, path = \"c.txt\")]\nmod c;\nuse std::path::Path;\n",
        );
        let lines: Vec<usize> = found
            .attributes
            .iter()
            .filter(|meta| meta.path == "path")
            .map(|meta| meta.line)
            .collect();
        assert_eq!(lines, [1, 3]);
    }

    #[test]
    fn include_calls_and_renames_are_found_and_include_str_is_not() {
        let found = scanned(
            "include!(\"x\");\nstd::include!(\"y\");\nuse std::include as i;\nuse std::{include as j, fmt};\n\
             const S: &str = include_str!(\"z\");\nfn g() -> impl Sized + use<> {}\n",
        );
        assert_eq!(named(&found.macro_calls, "include"), [1, 2]);
        assert_eq!(named(&found.use_names, "include"), [3, 4]);
        assert_eq!(named(&found.macro_calls, "include_str"), [5]);
        assert!(found.use_names.iter().all(|located| located.line < 6));
    }

    #[test]
    fn unsafe_is_a_word_token_and_is_found_in_attributes() {
        let found = scanned(
            "fn f() { unsafe {} }\n#[unsafe(no_mangle)]\nconst S: &str = \"unsafe\"; // unsafe\n\
             #![forbid(unsafe_code)]\n",
        );
        assert_eq!(named(&found.words, "unsafe"), [1, 2]);
    }

    #[test]
    fn macro_rules_definitions_are_found_and_negation_is_not_a_call() {
        let found =
            scanned("macro_rules! raw { () => {} }\nfn f(x: bool) -> bool { return !x; }\n");
        assert_eq!(named(&found.macro_definitions, "raw"), [1]);
        assert!(found.macro_calls.is_empty());
    }

    #[test]
    fn lines_are_one_based_at_the_hash() {
        let found = scanned("\n\n#[allow(\n    a,\n)]\nfn f() {}\n");
        assert_eq!(found.attributes, [meta(3, "allow", paths(&["a"]))]);
    }

    #[test]
    fn a_file_that_does_not_tokenize_is_an_error() {
        assert!(scan("pub fn broken( {\n").is_err());
    }

    #[test]
    fn a_shebang_line_is_refused_after_a_byte_order_mark_and_the_rest_is_read() {
        let found = scanned("\u{feff}#!/usr/bin/env /*\n#![expect(dead_code)]\n// */\n");
        assert_eq!(
            found.refused,
            [Refused {
                line: 1,
                rule: Rule::Shebang
            }]
        );
        assert_eq!(found.attributes, [meta(2, "expect", paths(&["dead_code"]))]);
        assert_eq!(refused("#!"), [(1, Rule::Shebang)]);
        assert_eq!(refused("\u{feff}#![forbid(unsafe_code)]\nfn f() {}\n"), []);
        assert_eq!(refused(TYPED_ID), []);
    }

    #[test]
    fn a_hash_that_starts_no_attribute_is_refused() {
        assert_rule(
            Rule::LoneHash,
            &[
                (
                    "macro_rules! h { ($h:tt) => { $h[allow(dead_code)] fn f() {} }; }\nh!(#);\n",
                    &[2],
                ),
                ("macro_rules! g { ($b:meta) => { #$b fn f() {} }; }\n", &[1]),
                (
                    "macro_rules! j { ($h:tt, $b:tt) => { $h $b fn f() {} }; }\nj!(#, [allow(x)]);\n",
                    &[2],
                ),
            ],
            &[
                "/// a\n//! b\n#[a]\n#![b]\nconst C: char = '#';\nconst S: &str = r#\"#\"#;\n",
                "k!(# [allow(x)]);\n",
            ],
        );
    }

    #[test]
    fn a_macro_call_named_by_a_fragment_is_refused() {
        assert_rule(
            Rule::FragmentCall,
            &[
                ("macro_rules! a { ($n:ident) => { $n!(\"p\"); }; }\n", &[1]),
                (
                    "macro_rules! b { ($n:ident) => { std::$n!(\"p\"); }; }\n",
                    &[1],
                ),
                (
                    "macro_rules! c { ($($p:ident)*) => { $($p)*!(\"p\"); }; }\n",
                    &[1],
                ),
                (
                    "macro_rules! d { ($($p:ident)::*) => { $($p)::*!(\"p\"); }; }\n",
                    &[1],
                ),
                (
                    "macro_rules! e { ($m:ident) => { $m! inner { () => {} } }; }\n",
                    &[1],
                ),
            ],
            &[
                "macro_rules! f {\n    ($x:expr, $($y:expr),*) => {\n        \
               $crate::g!($x) && $x != 1 && !($x) $(&& !$y)*\n    };\n}\n\
               fn h(a: u8) -> bool { a * !(a) == 0 }\n",
            ],
        );
    }

    #[test]
    fn the_tt_fragment_is_refused() {
        let doc_split = "macro_rules! h {\n    ($h:tt $_d:tt) => {\n        $h[allow(dead_code)]\n        \
                         fn never_used() {}\n    };\n}\nh!(\n    /// harmless words\n);\n";
        assert_rule(
            Rule::TtFragment,
            &[
                ("macro_rules! h { ($h:tt) => {}; }\n", &[1]),
                (doc_split, &[2, 2]),
            ],
            &["macro_rules! p { ($x:ident, $t:ty) => { $x::tt::<$t>() }; }\n"],
        );
    }

    #[test]
    fn macro_rules_inside_a_macro_body_is_refused() {
        let define = "macro_rules! define {\n    ($name:ident, $body:item) => {\n        \
                      macro_rules! $name {\n            () => { $body };\n        }\n    };\n}\n";
        assert_rule(
            Rule::NestedDefinition,
            &[(define, &[1])],
            &["macro_rules! one { () => { two!(); }; }\nmacro_rules! two { () => {}; }\n"],
        );
    }

    #[test]
    fn a_fragment_in_a_use_item_is_refused() {
        let rename = "macro_rules! b {\n    ($name:ident) => {\n        use std::$name as inc;\n    };\n}\n\
                      b!(include);\ninc!(\"payload.txt\");\n";
        assert_rule(
            Rule::UseFragment,
            &[
                (rename, &[3]),
                (
                    "macro_rules! c { ($n:ident) => { use std::{fmt, $n as j}; }; }\n",
                    &[1],
                ),
            ],
            &[
                "macro_rules! d { () => { use $crate::x; }; }\n",
                "fn g() -> impl Sized + use<> {}\n",
            ],
        );
    }

    #[test]
    fn a_cfg_predicate_that_names_clippy_or_holds_a_fragment_is_refused() {
        assert_rule(
            Rule::Cfg,
            &[
                ("#[cfg(all(test, not(clippy)))]\nmod a {}\n", &[1]),
                ("#[cfg_attr(not(r#clippy), allow(x))]\nfn f() {}\n", &[1]),
                (
                    "#![cfg_attr(a, cfg_attr(clippy, x), y)]\n#[cfg_attr(a, cfg(clippy))]\n",
                    &[1, 2],
                ),
                (
                    "macro_rules! m { ($p:meta) => { #[cfg($p)] fn f() {} }; }\n",
                    &[1],
                ),
                (
                    "fn g() -> bool { cfg!(clippy) || std::cfg!(not(clippy)) }\n",
                    &[1, 1],
                ),
            ],
            &[
                "#[cfg(test)]\n#[cfg_attr(test, allow(clippy::x))]\n#[cfg(feature = \"clippy\")]\n\
               #[cfg_attr(docsrs, doc(cfg(feature = \"x\")))]\nfn f() -> bool { cfg!(test) }\n",
            ],
        );
    }

    #[test]
    fn every_include_name_is_refused_once() {
        let keyword = "macro_rules! b {\n    ($u:ident) => {\n        $u std::include as inc;\n    };\n}\nb!(use);\n";
        assert_rule(
            Rule::IncludeName,
            &[
                (keyword, &[3]),
                (
                    "macro_rules! r { ($n:ident) => { $n!(\"p.txt\"); }; }\nr!(include);\n",
                    &[2],
                ),
                ("macro_rules! include { () => {}; }\n", &[1]),
                ("fn f() { let r#include = 1; }\n", &[1]),
            ],
            &["const S: &str = include_str!(\"y\");\n"],
        );
        let literal = scanned("include!(\"x\");\nuse std::{fmt, include as j};\n");
        assert_eq!(literal.refused, []);
        assert_eq!(named(&literal.macro_calls, "include"), [1]);
        assert_eq!(named(&literal.use_names, "include"), [2]);
    }

    #[test]
    fn review_an_inner_meta_binding_passes() {
        let found = scan("macro_rules! m { (#![$a:meta]) => { #![$a] }; }\n").unwrap();
        assert!(found.unreadable.is_empty(), "{:?}", found.unreadable);
    }

    #[test]
    fn review_an_unparsable_macro_rules_is_unreadable() {
        let found = scan("macro_rules! bad { () => {} junk }\n").unwrap();
        assert_eq!(found.unreadable[0].name, "macro_rules! bad");
    }

    #[test]
    fn review_a_leading_colon_attribute_path_is_unreadable() {
        let found = scan("#[::allow(dead_code)]\nfn f() {}\n").unwrap();
        assert_eq!(found.unreadable.len(), 1);
    }

    #[test]
    fn review_an_attribute_from_a_hash_fragment_is_seen() {
        let found =
            scan("macro_rules! h { ($h:tt) => { $h[allow(dead_code)] fn f() {} }; }\nh!(#);\n")
                .unwrap();
        assert!(found.attributes.is_empty());
        assert_eq!(
            refused_of(&found),
            [(1, Rule::TtFragment), (2, Rule::LoneHash)],
            "rustc applies #[allow(dead_code)] here, and the scan reports nothing"
        );
    }

    #[test]
    fn review_an_attribute_from_a_group_fragment_is_seen() {
        let found =
            scan("macro_rules! g { ($b:tt) => { #$b fn f() {} }; }\ng!([allow(dead_code)]);\n")
                .unwrap();
        assert_eq!(
            refused_of(&found),
            [(1, Rule::TtFragment), (1, Rule::LoneHash)],
            "rustc applies #[allow(dead_code)] here, and the scan reports nothing"
        );
    }

    #[test]
    fn review_include_through_an_ident_fragment_is_seen() {
        let found =
            scan("macro_rules! r { ($n:ident) => { $n!(\"p.txt\"); }; }\nr!(include);\n").unwrap();
        assert_eq!(named(&found.macro_calls, "n"), [1]);
        assert_eq!(
            refused_of(&found),
            [(1, Rule::FragmentCall), (2, Rule::IncludeName)],
            "rustc expands include!(\"p.txt\") here, and the scan reports nothing"
        );
    }

    #[test]
    fn review_a_shebang_line_does_not_hide_an_attribute() {
        let found =
            scan("#!/usr/bin/env /*\n#![expect(dead_code)]\n// */\nfn never_used() {}\n").unwrap();
        assert!(
            found.attributes.iter().any(|meta| meta.path == "expect"),
            "rustc strips the shebang line and reads #![expect(dead_code)]"
        );
        assert_eq!(
            found.refused,
            [Refused {
                line: 1,
                rule: Rule::Shebang
            }]
        );
    }
}
