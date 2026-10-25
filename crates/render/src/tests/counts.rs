use std::path::Path;

use app::{
    CallerRef, ChartGroup, ChartNode, CommunityReport, CommunityRow, CycleReport, CycleRow,
    DiagramView, ExpandResult, Flowchart, HealthComplexity, HealthComplexityRow, HealthReport,
    HealthSymbol, RefGroup, RefListing, RefSite, RefsResult, SharedCaller, Skipped,
};
use io::ProjectConfig;
use kernel::Bounded;
use serde_json::Value;

use crate::{
    Receipt, View, diagram_caveat, diagram_summary, expand_stderr_rows, json, mermaid, text,
};

fn receipt(config: &ProjectConfig) -> Receipt<'_> {
    Receipt {
        root: Path::new("/srv/app"),
        head: Some("1a2b3c4d"),
        tree: Some("5e6f7a8b"),
        dirty: Some(0),
        cache: "none",
        note: "",
        config,
        warnings: Vec::new(),
        counter: &app::Chars4,
        names_repo: false,
    }
}

// The text answer without its one receipt line.
fn body(view: &impl View) -> String {
    let config = ProjectConfig::default();
    let answer = text(&receipt(&config), view);
    let (_, body) = answer.split_once('\n').unwrap();
    body.to_owned()
}

fn symbol(name: &str, line: u32) -> HealthSymbol {
    HealthSymbol {
        stable_id: format!("py:a.py::{name}"),
        path: "a.py".to_owned(),
        line,
        label: name.to_owned(),
        kind: "function",
        in_degree: 0,
        out_degree: 0,
        discounted: Vec::new(),
    }
}

fn hotspot(name: &str, line: u32) -> HealthComplexityRow {
    HealthComplexityRow {
        path: "a.py".to_owned(),
        line,
        label: name.to_owned(),
        cc: 2,
        sloc: 4,
        mass: 4.0,
        stable_id: Some(format!("py:a.py::{name}")),
    }
}

fn skipped(path: &str) -> Skipped {
    Skipped {
        path: path.to_owned(),
        reason: "not UTF-8 text".to_owned(),
    }
}

#[test]
fn a_count_of_one_takes_the_singular_in_the_health_answer() {
    let report = HealthReport {
        orphans: Bounded::cut(vec![symbol("f", 1), symbol("g", 5)], 1),
        unused_exports: Bounded::cut(vec![symbol("h", 9)], 20),
        hubs: Bounded::cut(vec![symbol("d", 13), symbol("e", 17)], 1),
        complexity: HealthComplexity {
            threshold: 10,
            files: 4,
            walked: 3,
            units: 1,
            over_threshold: 0,
            erosion: 0.0,
            sloc: 4,
            clone_lines: 0,
            not_measured: Bounded::cut(vec![skipped("b.py"), skipped("c.py")], 1),
        },
        hotspots: Bounded::cut(vec![hotspot("f", 1), hotspot("g", 5)], 1),
        symbols: 1,
        excluded: 1,
    };
    assert_eq!(
        body(&report),
        "health over 1 symbol; 1 excluded as test or fixture paths\ndegree counts same-file and \
         resolved-via-import edges only; unique and name-only-ambiguous matches are discounted \
         and named per row\nmethods are ranked as hubs and never reported as orphans: a call \
         through a selector resolves to no edge, so every method would be a permanent \
         candidate\n\n2 orphan candidates (function, no counted edge in or out)\n  [py:a.py::f] @1  \
         f  function  in 0  out 0\n... 1 more orphan candidate not listed (limit 1)\n\n1 unused \
         export (public function outside a function body, no counted edge in)\n  [py:a.py::h] @9  \
         h  function  in 0  out 0\n\n2 symbols carry a counted edge (function, method or class, \
         highest counted degree first)\n  [py:a.py::d] @13  d  function  in 0  out 0\n... 1 more \
         hub not listed (limit 1)\n\ncomplexity over 1 unit in 1 file (threshold cc > 10): 0 over \
         the threshold, erosion 0.000, clone share 0.000\ncomplexity: 3 of 4 files walked before \
         the time budget\ncomplexity: 2 files not measured\n  b.py  not UTF-8 text\n... 1 more \
         unmeasured file not listed (limit 1)\n\n2 units ranked by mass (named functions and \
         methods, mass = cc * sqrt(sloc), highest first)\n  [py:a.py::f] @1  f  cc 2  sloc 4  \
         mass 4.0\n... 1 more hotspot not listed (limit 1)\n"
    );
}

fn community(label: &str, members: &[&str], limit: usize) -> CommunityRow {
    CommunityRow {
        label: label.to_owned(),
        members: Bounded::cut(
            members.iter().map(|&member| member.to_owned()).collect(),
            limit,
        ),
        internal_weight: 1.0,
        total_weight: 1.0,
    }
}

fn communities(rows: Vec<CommunityRow>, limit: usize, modularity: f64) -> CommunityReport {
    CommunityReport {
        communities: Bounded::cut(rows, limit),
        modularity,
        standard_modularity: modularity,
        resolution: 1.0,
        files: 1,
        converged: true,
    }
}

#[test]
fn a_count_of_one_takes_the_singular_and_each_note_its_marker_in_the_communities() {
    let rows = vec![
        community("pkg around pkg/a.py", &["pkg/a.py", "pkg/b.py"], 1),
        community("c.py", &["c.py"], 12),
    ];
    let cut = CommunityReport {
        converged: false,
        ..communities(rows, 1, 0.0)
    };
    assert_eq!(
        body(&cut),
        "2 communities over 1 file (modularity 0.000 at resolution 1)\n// note: community \
         detection stopped at its pass limit before the partition settled, so these communities \
         are a partial answer\n// note: weak partition; use these communities as a hint, not a \
         boundary\n    1. pkg around pkg/a.py  (2 files)\n       pkg/a.py\n... 1 more file in \
         this community not listed (limit 1)\n... 1 more community not listed (limit 1)\n"
    );
    let one = communities(vec![community("c.py", &["c.py"], 12)], 20, 0.5);
    assert_eq!(
        body(&one),
        "1 community over 1 file (modularity 0.500 at resolution 1)\n    1. c.py  (1 file)\n       \
         c.py\n"
    );
    let none = communities(Vec::new(), 20, 0.0);
    assert_eq!(
        body(&none),
        "no communities: no file in this repository parsed\n"
    );
}

#[test]
fn the_unresolved_import_note_of_the_cycles_takes_its_marker_and_the_count_its_form() {
    let none = CycleReport {
        cycles: Bounded::cut(Vec::new(), 20),
        unresolved_imports: 1,
        unresolved_internal_imports: 1,
        skipped: Vec::new(),
    };
    assert_eq!(
        body(&none),
        "no import cycles\n// note: 1 import statement of this repository's own modules did not \
         resolve, so a cycle through it is not listed\n"
    );
    let ring = CycleRow {
        files: vec!["a.py".to_owned(), "b.py".to_owned()],
        members: Vec::new(),
    };
    let one = CycleReport {
        cycles: Bounded::cut(vec![ring], 20),
        unresolved_internal_imports: 2,
        ..none
    };
    assert_eq!(
        body(&one),
        "1 import cycle\n// note: 2 import statements of this repository's own modules did not \
         resolve, so a cycle through them is not listed\n    1. (2 files) a.py -> b.py -> a.py\n"
    );
}

fn diagram(focus: &str, grouped: bool) -> DiagramView {
    let node = ChartNode {
        id: "n0".to_owned(),
        label: "pkg/a.py".to_owned(),
    };
    let group = ChartGroup {
        id: "s0".to_owned(),
        title: "pkg around pkg/a.py".to_owned(),
        members: vec![node],
    };
    DiagramView {
        chart: Some(Flowchart {
            groups: vec![group],
            nodes: Vec::new(),
            edges: Vec::new(),
            edge_bound: 1,
        }),
        nodes: 1,
        elided: 1,
        grouped,
        focus: focus.to_owned(),
        message: String::new(),
        edges_over_bound: 1,
        rank_converged: false,
    }
}

#[test]
fn a_count_of_one_takes_the_singular_in_the_diagram_and_its_caveat_marks_each_note() {
    let view = diagram("", true);
    assert_eq!(
        mermaid(&view),
        "flowchart LR\n    subgraph s0[\"pkg around pkg/a.py\"]\n        n0[\"pkg/a.py\"]\n    \
         end\n    %% 1 reference edge not drawn (edge bound 1)\n    n_elided[\"... 1 more \
         module\"]\n"
    );
    let summaries = [
        diagram_summary(&diagram("", false)),
        diagram_summary(&view),
        diagram_summary(&diagram("pkg/a.py", false)),
        diagram_summary(&diagram("pkg/a.py", true)),
    ];
    assert_eq!(
        summaries,
        [
            "diagram of 1 module; 1 elided",
            "diagram of 1 module, grouped by community; 1 elided",
            "diagram of 1 module around pkg/a.py; 1 elided",
            "diagram of 1 module around pkg/a.py, grouped by community; 1 elided",
        ]
    );
    let notes = "the ranking that chose these modules did not converge, so which ones the bound \
                 kept is a partial answer\nsubgraph titles name whole communities, including the 1 \
                 module the rank bound left out of this diagram";
    let marked = notes.replace("the ranking", "// note: the ranking");
    let marked = marked.replace("\nsubgraph", "\n// note: subgraph");
    assert_eq!(diagram_caveat(&view).as_deref(), Some(marked.as_str()));
    assert!(body(&view).ends_with(&format!("```\n\n{marked}\n")));
    let config = ProjectConfig::default();
    let document: Value = serde_json::from_str(&json(&receipt(&config), &view).unwrap()).unwrap();
    assert_eq!(document["caveat"], notes);
}

fn site(line: u32) -> RefSite {
    RefSite {
        line,
        enclosing: "(module level)".to_owned(),
        stable_id: None,
    }
}

fn refs(groups: Vec<RefGroup>, total: usize, files: usize) -> RefsResult {
    RefsResult {
        target: "py:lib.py::target".to_owned(),
        groups: RefListing {
            rows: groups,
            total,
            limit: 1,
            files,
        },
        shared: Bounded::cut(Vec::new(), 20),
        shared_callers: false,
        target_resolved: true,
        definition: Some("py:lib.py::target".to_owned()),
        definitions: 1,
        data_key: None,
        unresolved: None,
        skipped: Vec::new(),
        scanned: Some(1),
    }
}

fn candidate(name: &str, callers: usize) -> SharedCaller {
    let caller = |at: usize| CallerRef {
        qualname: format!("c{at}"),
        path: "a.py".to_owned(),
        line: u32::try_from(at).unwrap(),
    };
    SharedCaller {
        stable_id: format!("py:lib.py::{name}"),
        path: "lib.py".to_owned(),
        line: 5,
        overlap: 2,
        shared_files: 1,
        score: 0.5,
        callers: (1..=callers).map(caller).collect(),
        in_tests: false,
    }
}

#[test]
fn a_count_of_one_takes_the_singular_in_the_fan_in() {
    let group = RefGroup {
        path: "a.py".to_owned(),
        sites: vec![site(3)],
        tier: "resolved",
        tier_label: "resolved-via-import",
        scope_checked: true,
    };
    assert_eq!(
        body(&refs(vec![group], 2, 2)),
        "2 references to py:lib.py::target\n\na.py  (1 reference, resolved-via-import)\n  (module \
         level) @3\n\n... 1 more reference not listed (limit 1), including every reference in 1 \
         more file\n"
    );
    assert_eq!(
        body(&refs(Vec::new(), 0, 0)),
        "no references to py:lib.py::target outside its own definition (complete scan: 1 file, \
         0 skipped)\n"
    );
    let shared = RefsResult {
        shared_callers: true,
        shared: Bounded::cut(vec![candidate("other", 6), candidate("more", 2)], 1),
        ..refs(Vec::new(), 0, 0)
    };
    assert_eq!(
        body(&shared),
        "symbols sharing callers with py:lib.py::target\n  [py:lib.py::other] @5  (2 shared \
         callers in 1 file, score 0.500)\n      c1    a.py:1\n      c2    a.py:2\n      c3    \
         a.py:3\n      c4    a.py:4\n      c5    a.py:5\n... 1 more caller not listed (limit \
         5)\n... 1 more candidate not listed (limit 1)\n"
    );
}

#[test]
fn the_row_that_counts_the_unlisted_misses_takes_the_singular_at_one() {
    let misses = (1..=20).map(|at| {
        (
            format!("py:a.py::f{at}"),
            "a.py does not define f".to_owned(),
        )
    });
    let result = ExpandResult {
        cards: Vec::new(),
        unresolved: misses.collect(),
        budget: 12_000,
        unresolved_omitted: 1,
    };
    let rows = expand_stderr_rows(&result);
    assert_eq!(
        rows.last().map(String::as_str),
        Some("unresolved: 1 further id is not listed; 21 of the ids requested did not resolve")
    );
}
