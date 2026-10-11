use std::collections::BTreeSet;
use std::str::FromStr;

use proc_macro2::{Delimiter, Group, Ident, TokenStream, TokenTree};

type Bound = BTreeSet<String>;

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

/// Everything the gates read from one file's tokens, at every depth.
#[derive(Debug, Default)]
pub struct Scan {
    pub attributes: Vec<Meta>,
    pub unreadable: Vec<Located>,
    pub macro_calls: Vec<Located>,
    pub macro_definitions: Vec<Located>,
    pub use_names: Vec<Located>,
    pub words: Vec<Located>,
}

enum Shape {
    Binding,
    Fragment(String),
    Meta { path: String, list: Option<Group> },
    Unreadable,
}

/// Tokenize `text` as rustc does and collect its attributes, macros, use names and words.
pub fn scan(text: &str) -> Result<Scan, String> {
    let stream = TokenStream::from_str(text).map_err(|error| {
        let at = error.span().start();
        format!("{}:{}: {error}", at.line, at.column + 1)
    })?;
    let mut scan = Scan::default();
    walk(stream, &Bound::new(), &mut scan);
    Ok(scan)
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
    let line = line_of(&tokens[index]);
    let mut next = index + 1;
    if is_punct(tokens.get(next), '!') {
        next += 1;
    }
    match tokens.get(next) {
        Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Bracket => {
            let content: Vec<TokenTree> = group.stream().into_iter().collect();
            item(&content, line, bound, scan);
            walk(group.stream(), bound, scan);
            next + 1
        }
        _ => index + 1,
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
    if is_punct(tokens.get(index + 1), '!')
        && matches!(tokens.get(index + 2), Some(TokenTree::Group(_)))
    {
        scan.macro_calls.push(word.clone());
    }
    if word.name == "use" && !is_punct(tokens.get(index + 1), '<') {
        for token in tokens[index + 1..]
            .iter()
            .take_while(|token| !is_punct(Some(token), ';'))
        {
            identifiers(token, &mut scan.use_names);
        }
    }
    scan.words.push(word);
    index + 1
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
    use super::{Item, Located, Meta, Scan, scan};

    fn scanned(text: &str) -> Scan {
        scan(text).unwrap()
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
}
