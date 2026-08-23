"""The HTML graph export is bounded, deterministic, and inert to path text."""

import pytest

from agentless_mcp.core import communities, htmlgraph
from agentless_mcp.core.graph import RefGraph

# Every way this document could reach the network. The old gate asserted that
# `https://` was absent, which the document never contained: it names
# `http://www.w3.org/2000/svg`, so the assertion checked a scheme the text
# provably never uses and would have passed against a remote script tag.
NETWORK_SINKS = (
    "src=",
    "href=",
    "@import",
    "url(http",
    "fetch(",
    "XMLHttpRequest",
    "WebSocket",
    "EventSource",
    "importScripts",
)


def test_export_is_self_contained_searchable_and_clickable():
    graph = RefGraph(
        nodes=("src/api.py", "src/model.py"),
        edges={("src/api.py", "src/model.py"): 3.0},
    )
    partition = communities.detect_communities(graph)

    exported = htmlgraph.render_html(
        graph,
        {"src/api.py": 0.6, "src/model.py": 0.4},
        partition,
        imports={("src/api.py", "src/model.py")},
    )

    assert exported.nodes == 2
    assert exported.edges == 1
    assert 'id="search"' in exported.text
    assert 'group.addEventListener("click"' in exported.text
    assert "function colour(community)" in exported.text


def test_no_sink_in_the_document_can_reach_the_network():
    graph = RefGraph(nodes=("a.py",), edges={})

    exported = htmlgraph.render_html(
        graph,
        {"a.py": 1.0},
        communities.detect_communities(graph),
        imports=set(),
    )

    for sink in NETWORK_SINKS:
        assert sink not in exported.text, sink


def test_the_document_carries_a_policy_that_refuses_a_fetch():
    # The assertion above is a text check over the document this module
    # writes. The policy is the gate that holds when someone edits it.
    graph = RefGraph(nodes=("a.py",), edges={})

    exported = htmlgraph.render_html(
        graph,
        {"a.py": 1.0},
        communities.detect_communities(graph),
        imports=set(),
    )

    assert 'http-equiv="Content-Security-Policy"' in exported.text
    assert "default-src 'none'" in exported.text


def test_repository_text_cannot_close_the_script_element():
    hostile = "</script><script>alert(1)</script>.py"
    graph = RefGraph(nodes=(hostile,), edges={})

    exported = htmlgraph.render_html(
        graph,
        {hostile: 1.0},
        communities.detect_communities(graph),
        imports=set(),
    )

    assert hostile not in exported.text
    assert r"\u003c/script\u003e" in exported.text
    assert exported.text.count("<script>") == 1


def test_node_and_edge_bounds_are_reported():
    graph = RefGraph(
        nodes=("a.py", "b.py", "c.py"),
        edges={
            ("a.py", "b.py"): 3.0,
            ("a.py", "c.py"): 2.0,
            ("b.py", "a.py"): 1.0,
        },
    )

    exported = htmlgraph.render_html(
        graph,
        {"a.py": 0.5, "b.py": 0.3, "c.py": 0.2},
        communities.detect_communities(graph),
        imports=set(),
        options=htmlgraph.HtmlOptions(max_nodes=2, max_edges=1),
    )

    assert exported.nodes == 2
    assert exported.edges == 1
    assert exported.elided_nodes == 1
    assert exported.elided_edges == 2


def test_the_two_edge_cuts_are_counted_apart():
    # `a.py -> c.py` went with the node bound; one of the two edges between
    # the drawn pair went to the edge bound. One number for both sends a
    # reader to the wrong flag.
    graph = RefGraph(
        nodes=("a.py", "b.py", "c.py"),
        edges={
            ("a.py", "b.py"): 3.0,
            ("a.py", "c.py"): 2.0,
            ("b.py", "a.py"): 1.0,
        },
    )

    exported = htmlgraph.render_html(
        graph,
        {"a.py": 0.5, "b.py": 0.3, "c.py": 0.2},
        communities.detect_communities(graph),
        imports=set(),
        options=htmlgraph.HtmlOptions(max_nodes=2, max_edges=1),
    )

    assert exported.edges_without_both_nodes == 1
    assert exported.edges_over_bound == 1
    assert "edges past the edge bound (${DATA.edgeBound})" in exported.text


def test_a_self_reference_is_not_drawn():
    graph = RefGraph(
        nodes=("a.py", "b.py"),
        edges={("a.py", "a.py"): 4.0, ("a.py", "b.py"): 1.0},
    )

    exported = htmlgraph.render_html(
        graph,
        {"a.py": 0.6, "b.py": 0.4},
        communities.detect_communities(graph),
        imports=set(),
    )

    assert exported.edges == 1
    assert exported.edges_without_both_nodes == 1


def test_the_payload_ships_no_field_the_script_never_reads():
    graph = RefGraph(nodes=("a.py", "b.py"), edges={("a.py", "b.py"): 1.0})

    exported = htmlgraph.render_html(
        graph,
        {"a.py": 0.6, "b.py": 0.4},
        communities.detect_communities(graph),
        imports=set(),
    )

    assert '"rank"' not in exported.text


def test_a_partition_from_another_graph_is_refused():
    graph = RefGraph(nodes=("a.py",), edges={})
    elsewhere = communities.detect_communities(RefGraph(nodes=("z.py",), edges={}))

    with pytest.raises(ValueError, match="partition does not cover"):
        htmlgraph.render_html(graph, {"a.py": 1.0}, elsewhere, imports=set())


@pytest.mark.parametrize(
    ("options", "message"),
    [
        (htmlgraph.HtmlOptions(max_nodes=0), "max_nodes"),
        (htmlgraph.HtmlOptions(max_nodes=htmlgraph.MAX_HTML_NODES + 1), "max_nodes"),
        (htmlgraph.HtmlOptions(max_edges=-1), "max_edges"),
        (htmlgraph.HtmlOptions(max_edges=htmlgraph.MAX_HTML_EDGES + 1), "max_edges"),
    ],
    ids=["nodes-zero", "nodes-over", "edges-negative", "edges-over"],
)
def test_a_bound_outside_the_ceiling_is_refused(options, message):
    # The ceilings are the only limit on how large a document a caller can
    # ask for. The CLI re-checks the same range today, so this is the gate
    # that stops the ceiling being deleted here with the suite still green.
    graph = RefGraph(nodes=("a.py",), edges={})

    with pytest.raises(ValueError, match=message):
        htmlgraph.render_html(
            graph,
            {"a.py": 1.0},
            communities.detect_communities(graph),
            imports=set(),
            options=options,
        )


def test_two_exports_of_one_graph_are_byte_identical():
    graph = RefGraph(nodes=("a.py", "b.py"), edges={("a.py", "b.py"): 1.0})
    partition = communities.detect_communities(graph)
    rank = {"a.py": 0.5, "b.py": 0.5}

    first = htmlgraph.render_html(graph, rank, partition, imports=set())
    second = htmlgraph.render_html(graph, rank, partition, imports=set())

    assert first == second
