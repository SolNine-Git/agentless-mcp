use serde::Deserialize;
use serde_json::{Value, json};

use super::corpus::{Corpus, edge, endpoint, ids, reference};
use crate::cycles::import_cycles;
use crate::names::NameIndex;
use crate::path::{PathBounds, PathPolicy, shortest_path};
use crate::resolver::Resolver;
use crate::scan::FileFacts;
use crate::scopes::{ImportScope, Scopes};
use crate::symbols::SymbolGraph;

fn with_resolver(check: impl Fn(&Corpus, &[FileFacts], &Resolver<'_>)) {
    for corpus in Corpus::all() {
        let files = corpus.files();
        let index = NameIndex::build(&files);
        let scopes = Scopes::build(&files);
        check(&corpus, &files, &Resolver::new(&index, &scopes));
    }
}

fn scope_value(path: &str, scope: &ImportScope<'_>) -> Value {
    let bindings = |table: &std::collections::BTreeMap<&str, std::collections::BTreeSet<&str>>| {
        table
            .iter()
            .map(|(name, paths)| json!([name, paths]))
            .collect::<Vec<_>>()
    };
    let members: Vec<Value> = scope
        .members
        .iter()
        .map(|(name, pairs)| {
            json!([
                name,
                pairs
                    .iter()
                    .map(|(file, member)| json!([file, member]))
                    .collect::<Vec<_>>()
            ])
        })
        .collect();
    let statements: Vec<Value> = scope
        .statements
        .iter()
        .map(|row| json!([row.module, row.line, row.resolved, row.relative]))
        .collect();
    let edges: Vec<Value> = scope
        .resolved_edges(path)
        .map(|(module, line, target)| json!([module, line, target]))
        .collect();
    json!({
        "path": path,
        "wholesale": scope.wholesale,
        "module_bindings": bindings(&scope.module_bindings),
        "named": bindings(&scope.named),
        "members": members,
        "statements": statements,
        "resolved_edges": edges,
    })
}

#[test]
fn import_scopes_match_the_reference() {
    with_resolver(|corpus, _, resolver| {
        let scopes = resolver.scopes();
        let found: Vec<Value> = scopes
            .iter()
            .map(|(path, scope)| scope_value(path, scope))
            .collect();
        assert_eq!(found, corpus.cases::<Value>("scopes"), "{}", corpus.name);
        let pairs: std::collections::BTreeSet<(String, String)> = found
            .iter()
            .flat_map(|scope| {
                let path = scope["path"].as_str().unwrap_or_default().to_owned();
                let edges = scope["resolved_edges"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                edges.into_iter().map(move |edge| {
                    (
                        path.clone(),
                        edge[2].as_str().unwrap_or_default().to_owned(),
                    )
                })
            })
            .collect();
        let imports: std::collections::BTreeSet<(String, String)> = scopes
            .import_pairs()
            .into_iter()
            .map(|(from, to)| (from.to_owned(), to.to_owned()))
            .collect();
        assert_eq!(imports, pairs, "{}", corpus.name);
    });
}

fn graph_value(graph: &SymbolGraph<'_>) -> Value {
    let degrees = graph.ambiguous_degrees();
    let pairs = |counts: &std::collections::BTreeMap<String, u64>| {
        counts
            .iter()
            .map(|(node, count)| json!([node, count]))
            .collect::<Vec<_>>()
    };
    json!({
        "definitions": graph.definitions().keys().collect::<Vec<_>>(),
        "files": graph.files(),
        "unresolved_imports": graph.unresolved_imports(),
        "unresolved_internal_imports": graph.unresolved_internal_imports(),
        "arriving": pairs(&degrees.arriving),
        "leaving": pairs(&degrees.leaving),
    })
}

#[test]
fn the_symbol_graph_matches_the_reference() {
    with_resolver(|corpus, _, resolver| {
        let graph = SymbolGraph::build(resolver, None).expect("no deadline stops it");
        let edges: Vec<Value> = graph.edges().iter().map(edge).collect();
        assert_eq!(
            edges,
            corpus.cases::<Value>("symbol_edges"),
            "{}",
            corpus.name
        );
        assert_eq!(
            vec![graph_value(&graph)],
            corpus.cases::<Value>("symbol_graph"),
            "{}",
            corpus.name
        );
        let ambiguous: Vec<Value> = graph.ambiguous().iter().map(reference).collect();
        assert_eq!(
            ambiguous,
            corpus.cases::<Value>("ambiguous"),
            "{}",
            corpus.name
        );
        let candidates: Vec<Value> = graph
            .candidates
            .iter()
            .map(|(name, targets)| json!([name, targets.iter().map(endpoint).collect::<Vec<_>>()]))
            .collect();
        assert_eq!(
            candidates,
            corpus.cases::<Value>("candidates"),
            "{}",
            corpus.name
        );
        let imports = SymbolGraph::imports(resolver.scopes());
        let import_edges: Vec<Value> = imports.edges().iter().map(edge).collect();
        assert_eq!(
            import_edges,
            corpus.cases::<Value>("import_edges"),
            "{}",
            corpus.name
        );
        assert_eq!(
            vec![graph_value(&imports)],
            corpus.cases::<Value>("import_graph"),
            "{}",
            corpus.name
        );
        let cycles: Vec<Value> = import_cycles(&imports)
            .iter()
            .map(|cycle| json!([cycle.files, cycle.members]))
            .collect();
        assert_eq!(cycles, corpus.cases::<Value>("cycles"), "{}", corpus.name);
    });
}

#[derive(Deserialize)]
struct AdjacencyCase {
    node: String,
    outgoing: Vec<usize>,
    incoming: Vec<usize>,
    references_from: Value,
    candidates_at: Value,
    ambiguous_from: Value,
    ambiguous_into: Value,
}

#[test]
fn adjacency_matches_the_reference() {
    with_resolver(|corpus, _, resolver| {
        let graph = SymbolGraph::build(resolver, None).expect("no deadline stops it");
        let position = |found: &crate::symbols::SymbolEdge<'_>| {
            graph
                .edges()
                .iter()
                .position(|held| held == found)
                .expect("an edge of the graph")
        };
        for case in corpus
            .section::<AdjacencyCase>("adjacency")
            .unwrap_or_default()
        {
            let node = case.node.as_str();
            let outgoing: Vec<usize> = graph.outgoing(node).iter().map(position).collect();
            let incoming: Vec<usize> = graph.incoming(node).map(position).collect();
            let from: Vec<Value> = graph.references_from(node).iter().map(reference).collect();
            let at: Vec<Value> = graph
                .candidates_at(node)
                .iter()
                .map(|(name, target)| json!([name, endpoint(target)]))
                .collect();
            let context = format!("{} {node}", corpus.name);
            assert_eq!(
                (outgoing, incoming),
                (case.outgoing, case.incoming),
                "{context}"
            );
            assert_eq!(
                (Value::from(from), Value::from(at)),
                (case.references_from, case.candidates_at),
                "{context}"
            );
            let leaving: Vec<Value> = graph
                .ambiguous_from(node)
                .map(|found| edge(&found))
                .collect();
            let arriving: Vec<Value> = graph
                .ambiguous_into(node)
                .map(|found| edge(&found))
                .collect();
            assert_eq!(
                (Value::from(leaving), Value::from(arriving)),
                (case.ambiguous_from, case.ambiguous_into),
                "{context}"
            );
        }
    });
}

// source, target, include_unique, include_ambiguous, max_visited, found, visited, exhausted, hops
type PathCase = (String, String, bool, bool, usize, bool, usize, bool, Value);

#[test]
fn paths_match_the_reference() {
    with_resolver(|corpus, _, resolver| {
        let graph = SymbolGraph::build(resolver, None).expect("no deadline stops it");
        for (source, target, unique, ambiguous, bound, found, visited, exhausted, hops) in
            corpus.cases::<PathCase>("paths")
        {
            let policy = PathPolicy {
                include_unique: unique,
                include_ambiguous: ambiguous,
            };
            let bounds = PathBounds {
                max_visited: bound,
                deadline: None,
            };
            let result = shortest_path(&graph, &source, &target, policy, bounds)
                .expect("no deadline stops it");
            let walked: Vec<Value> = result
                .hops
                .iter()
                .map(|hop| json!([edge(&hop.edge), hop.forward]))
                .collect();
            let context = format!("{} {source} -> {target} {policy:?} {bound}", corpus.name);
            assert_eq!(Value::from(walked), hops, "{context}");
            assert_eq!(
                (result.found, result.visited, result.exhausted),
                (found, visited, exhausted),
                "{context}"
            );
        }
    });
}

#[derive(Deserialize)]
struct SitesCase {
    path: String,
    sites: Vec<(usize, String, Value)>,
}

#[test]
fn resolver_answers_match_the_reference() {
    with_resolver(|corpus, files, resolver| {
        for case in corpus.cases::<SitesCase>("sites") {
            let file = files
                .iter()
                .find(|file| file.path == case.path)
                .expect("a corpus file");
            let found: Vec<(usize, String, Value)> = resolver
                .resolved_sites(file, None)
                .map(|(site, resolution)| {
                    let at = file
                        .facts
                        .refs
                        .iter()
                        .position(|held| std::ptr::eq(held, site))
                        .expect("a ref of the file");
                    (
                        at,
                        resolution.tier.as_str().to_owned(),
                        ids(&resolution.candidates),
                    )
                })
                .collect();
            assert_eq!(found, case.sites, "{} {}", corpus.name, case.path);
        }
        let grid = corpus.section::<(String, String, Option<String>, Value)>("resolve");
        for (path, name, tier, candidates) in grid.unwrap_or_default() {
            let found = resolver.resolve(&name, &path);
            let tier_found = found
                .as_ref()
                .map(|resolution| resolution.tier.as_str().to_owned());
            let candidates_found = found.map_or(Value::from(Vec::<String>::new()), |resolution| {
                ids(&resolution.candidates)
            });
            assert_eq!(
                (tier_found, candidates_found),
                (tier, candidates),
                "{} {path} {name}",
                corpus.name
            );
        }
        for (path, name, expected) in corpus.cases::<(String, String, Value)>("through_imports") {
            assert_eq!(
                ids(&resolver.through_imports(&name, &path)),
                expected,
                "{} {path} {name}",
                corpus.name
            );
        }
    });
}
