"""The HTML graph export is bounded, deterministic, and inert to path text."""

from agentless_mcp.core import communities, htmlgraph
from agentless_mcp.core.graph import RefGraph


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
    assert "https://" not in exported.text


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


def test_two_exports_of_one_graph_are_byte_identical():
    graph = RefGraph(nodes=("a.py", "b.py"), edges={("a.py", "b.py"): 1.0})
    partition = communities.detect_communities(graph)
    rank = {"a.py": 0.5, "b.py": 0.5}

    first = htmlgraph.render_html(graph, rank, partition, imports=set())
    second = htmlgraph.render_html(graph, rank, partition, imports=set())

    assert first == second
