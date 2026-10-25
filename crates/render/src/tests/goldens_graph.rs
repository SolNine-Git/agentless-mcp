use app::{
    CommunitiesRequest, DiagramRequest, ExplainRequest, HealthRequest, HtmlRequest, IntArg, Noted,
    PathPolicy, PathRequest,
};

use super::goldens_read::{Pinned, golden, int};
use crate::{html_document, mermaid, strip_fence};

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

// One case per repository: the symbol explained, and the pair a path is traced between.
const CASES: [(&str, &str, &str, &str); 3] = [
    (
        "repo_py",
        "reorder_report",
        "reorder_report",
        "format_money",
    ),
    ("repo_ts", "reorderReport", "reorderReport", "formatMoney"),
    (
        "repo_nested",
        "format_amount",
        "render_invoice",
        "format_amount",
    ),
];

fn answered<T>(repo: &str, noted: Noted<Result<T, app::Error>>) -> T {
    assert!(
        noted.diagnostics.is_empty(),
        "{repo}: {:?}",
        noted.diagnostics
    );
    noted
        .value
        .unwrap_or_else(|error| panic!("{repo}: {error}"))
}

#[test]
fn the_explain_goldens_come_out_byte_for_byte() {
    for (repo, target, _, _) in CASES {
        let pinned = Pinned::new(repo);
        let request = ExplainRequest {
            target: target.to_owned(),
            limit: int("20"),
        };
        let noted = app::explain(&pinned.services, &pinned.reading(), &request);
        let explained = answered(repo, noted);
        let text = pinned.text(&explained);
        assert_eq!(text, golden("graph", repo, "explain.txt"), "{repo}");
        let json = pinned.json(&explained);
        assert_eq!(json, golden("graph", repo, "explain.json"), "{repo}");
    }
}

// The explain text after its receipt line.
fn explain_body(repo: &str, target: &str, limit: &str) -> String {
    let pinned = Pinned::new(repo);
    let request = ExplainRequest {
        target: target.to_owned(),
        limit: int(limit),
    };
    let explained = answered(
        repo,
        app::explain(&pinned.services, &pinned.reading(), &request),
    );
    let text = pinned.text(&explained);
    text.split_once('\n')
        .map_or(text.clone(), |(_, body)| body.to_owned())
}

#[test]
fn the_limit_cuts_the_other_definitions_and_counts_the_rest() {
    let expected = "ambiguous: 8 definitions match price equally; this explains the first in path \
                    order. Pass a stable id to explain another.\n\
                    [kt:Pricing.kt::Priceable.price] @13\n  method\n  fun price(): Double\n  \
                    also defined at kt:Pricing.kt::Invoice.price\n  \
                    also defined at scala:Pricing.scala::Invoice.price\n\
                    ... 5 more definitions not listed (limit 2)\n\n\
                    references (fan-out): none\n\n\
                    referenced by (fan-in): 1\n  name-only-ambiguous (1)\n    \
                    referenced by Invoice.price    [lua:pricing.lua::Invoice.price] @12\n\n\
                    imports\n    declares  none resolved inside this repository\n    \
                    imported by  nothing in this repository\n";
    assert_eq!(explain_body("tier2", "price", "2"), expected);
}

#[test]
fn a_data_key_explains_its_card_and_says_a_key_has_no_fan_in() {
    let expected = "[json:config.json::service.port] @3\n  key\n  port = 8080\n\n\
                    json:config.json::service.port is a data key, and a data key has no fan-in: \
                    Grep finds the spellings of port.\n";
    let body = explain_body("tier2", "json:config.json::service.port", "20");
    assert_eq!(body, expected);
}

#[test]
fn the_path_goldens_come_out_byte_for_byte() {
    for (repo, _, source, target) in CASES {
        let pinned = Pinned::new(repo);
        let request = PathRequest {
            source: source.to_owned(),
            target: target.to_owned(),
            policy: PathPolicy::default(),
            max_visited: int("20000"),
        };
        let traced = answered(
            repo,
            app::path(&pinned.services, &pinned.reading(), &request),
        );
        let text = pinned.text(&traced);
        assert_eq!(text, golden("graph", repo, "path.txt"), "{repo}");
    }
}

#[test]
fn the_communities_goldens_come_out_byte_for_byte() {
    for (repo, _, _, _) in CASES {
        let pinned = Pinned::new(repo);
        let request = CommunitiesRequest {
            resolution: None,
            limit: int("20"),
            members: int("12"),
        };
        let noted = app::communities(&pinned.services, &pinned.reading(), &request);
        let report = answered(repo, noted);
        let text = pinned.text(&report);
        assert_eq!(text, golden("graph", repo, "communities.txt"), "{repo}");
    }
}

#[test]
fn the_health_goldens_come_out_byte_for_byte() {
    for (repo, _, _, _) in CASES {
        let pinned = Pinned::new(repo);
        let request = HealthRequest { limit: int("20") };
        let report = answered(
            repo,
            app::health(&pinned.services, &pinned.reading(), &request),
        );
        assert_eq!(
            pinned.text(&report),
            golden("graph", repo, "health.txt"),
            "{repo}"
        );
        assert_eq!(
            pinned.json(&report),
            golden("graph", repo, "health.json"),
            "{repo}"
        );
    }
}

#[test]
fn the_fence_rule_strips_what_the_reference_strips() {
    let cases: Vec<(String, String)> =
        io::testing::cases(MANIFEST_DIR, "read_graph/fences", "fences");
    assert!(cases.len() > 10);
    for (text, expected) in cases {
        assert_eq!(strip_fence(&text), expected, "{text:?}");
    }
}

// The one byte difference is the ledger's: this build joins the search results on the JS escape.
#[test]
fn every_html_export_equals_the_reference_document_but_for_the_escape() {
    let cases: Vec<(String, usize, usize, String)> =
        io::testing::cases(MANIFEST_DIR, "read_graph/html", "documents");
    assert_eq!(cases.len(), 12);
    for (repo, nodes, edges, document) in cases {
        let pinned = Pinned::new(&repo);
        let request = HtmlRequest {
            max_nodes: IntArg::from_usize(nodes),
            max_edges: IntArg::from_usize(edges),
            resolution: None,
        };
        let graph = answered(
            &repo,
            app::html(&pinned.services, &pinned.reading(), &request),
        );
        let built = html_document(&graph).unwrap_or_else(|error| panic!("{error}"));
        let expected = document.replace(".join(\"\n\")", ".join(\"\\n\")");
        assert_eq!(built, expected, "{repo} {nodes} {edges}");
    }
}

// The diagram travels without an envelope, as both doors emit it.
#[test]
fn the_grouped_diagram_goldens_come_out_byte_for_byte() {
    for (repo, _, _, _) in CASES {
        let pinned = Pinned::new(repo);
        let request = DiagramRequest {
            focus: None,
            max_nodes: int("40"),
            max_edges: int("40"),
            group_by_communities: true,
            resolution: None,
        };
        let view = answered(
            repo,
            app::diagram(&pinned.services, &pinned.reading(), &request),
        );
        assert_eq!(
            mermaid(&view),
            golden("graph", repo, "diagram.mmd"),
            "{repo}"
        );
    }
}
