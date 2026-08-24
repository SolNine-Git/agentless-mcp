"""Mermaid rendering: determinism, bounding, and label safety."""

import pytest

from agentless_mcp.core.communities import detect_communities
from agentless_mcp.core.graph import RefGraph, personalized_pagerank
from agentless_mcp.core.mermaid import (
    EDGE_LEGEND,
    ELISION_ID,
    MERMAID_RESERVED_IDS,
    DiagramOptions,
    render_flowchart,
    safe_label,
)

# A filename that is a mermaid injection attempt: closing the label, closing
# the node, ending the statement and opening a click handler. Nothing stops a
# repository from containing it, so nothing may stop the renderer either.
HOSTILE_NAME = 'end"]; click n0 href "x.py'


def chain_graph(count):
    """A path graph of ``count`` numbered modules, each referencing the next."""
    nodes = tuple(f"src/m{index:02d}.py" for index in range(count))
    edges = {(nodes[index], nodes[index + 1]): 1.0 for index in range(count - 1)}
    return RefGraph(nodes=nodes, edges=edges)


def uniform_rank(graph):
    """A ranking that gives every node the same score."""
    return dict.fromkeys(graph.nodes, 1.0)


def flowchart_text(*args, **kwargs):
    """The rendered text alone, for the assertions that read the diagram.

    `render_flowchart` returns the text together with the counts behind it, so
    the tests that read a bound rather than a line go through
    :class:`FlowchartExport` directly -- see `TestReportedBounds`.
    """
    return render_flowchart(*args, **kwargs).text


class TestShape:
    def test_the_header_names_the_direction(self):
        graph = chain_graph(3)

        assert flowchart_text(graph, uniform_rank(graph)).startswith("flowchart LR\n")

    def test_ids_are_generated_in_sorted_path_order(self):
        graph = RefGraph(nodes=("b.py", "a.py"), edges={})

        rendered = flowchart_text(graph, uniform_rank(graph))

        assert '    n0["a.py"]' in rendered
        assert '    n1["b.py"]' in rendered

    def test_edges_are_rendered_between_generated_ids(self):
        graph = RefGraph(nodes=("a.py", "b.py"), edges={("a.py", "b.py"): 1.0})

        assert "    n0 --> n1" in flowchart_text(graph, uniform_rank(graph))

    def test_edge_weights_are_never_printed(self):
        graph = RefGraph(nodes=("a.py", "b.py"), edges={("a.py", "b.py"): 3.5})

        assert "3.5" not in flowchart_text(graph, uniform_rank(graph))

    def test_edges_to_elided_nodes_are_dropped(self):
        graph = chain_graph(6)

        rendered = flowchart_text(graph, uniform_rank(graph), options=DiagramOptions(max_nodes=2))

        assert rendered.count(" --> ") == 1

    def test_output_carries_no_markdown_fence(self):
        graph = chain_graph(3)

        assert "```" not in flowchart_text(graph, uniform_rank(graph))

    def test_an_empty_graph_renders_only_the_header(self):
        assert flowchart_text(RefGraph(nodes=(), edges={}), {}) == "flowchart LR\n"

    def test_a_node_the_ranking_forgot_is_still_a_candidate(self):
        graph = RefGraph(nodes=("a.py", "b.py"), edges={})

        rendered = flowchart_text(graph, {"a.py": 1.0})

        assert '"b.py"' in rendered


class TestEdgeKinds:
    def test_import_and_reference_edges_are_told_apart(self):
        graph = RefGraph(
            nodes=("a.py", "b.py"),
            edges={("a.py", "b.py"): 4.0, ("b.py", "a.py"): 1.0},
        )

        rendered = flowchart_text(graph, uniform_rank(graph), imports=frozenset({("a.py", "b.py")}))

        assert "    n0 --> n1" in rendered
        assert "    n1 -.-> n0" in rendered

    def test_the_legend_names_the_encoding(self):
        graph = RefGraph(nodes=("a.py", "b.py"), edges={("a.py", "b.py"): 1.0})

        rendered = flowchart_text(graph, uniform_rank(graph), imports=frozenset())

        assert f"    {EDGE_LEGEND}" in rendered
        assert "solid: imports, dashed: references" in rendered

    def test_without_import_information_every_edge_is_solid_and_unlegended(self):
        graph = RefGraph(
            nodes=("a.py", "b.py"),
            edges={("a.py", "b.py"): 1.0, ("b.py", "a.py"): 1.0},
        )

        rendered = flowchart_text(graph, uniform_rank(graph))

        assert "-.->" not in rendered
        assert EDGE_LEGEND not in rendered
        assert rendered.count(" --> ") == 2

    def test_reference_edges_over_the_bound_are_counted_not_drawn(self):
        graph = chain_graph(6)

        rendered = flowchart_text(
            graph,
            uniform_rank(graph),
            options=DiagramOptions(max_edges=3),
            imports=frozenset(),
        )

        assert "-.->" not in rendered
        assert "    %% 5 reference edges not drawn (edge bound 3)" in rendered

    def test_import_edges_survive_the_edge_bound(self):
        graph = chain_graph(6)

        rendered = flowchart_text(
            graph,
            uniform_rank(graph),
            options=DiagramOptions(max_edges=1),
            imports=frozenset({("src/m00.py", "src/m01.py")}),
        )

        assert "    n0 --> n1" in rendered
        assert "-.->" not in rendered
        assert "%% 4 reference edges not drawn (edge bound 1)" in rendered

    def test_an_edgeless_diagram_carries_no_legend(self):
        graph = RefGraph(nodes=("a.py",), edges={})

        rendered = flowchart_text(graph, uniform_rank(graph), imports=frozenset())

        assert EDGE_LEGEND not in rendered

    def test_a_diagram_whose_every_edge_was_counted_carries_no_legend(self):
        # Every edge is a reference and none fit the bound, so the only edge
        # line is the comment counting them. A legend naming two arrow styles
        # would sit above a diagram that draws neither.
        graph = chain_graph(6)

        rendered = flowchart_text(
            graph,
            uniform_rank(graph),
            options=DiagramOptions(max_edges=1),
            imports=frozenset(),
        )

        assert "%% 5 reference edges not drawn (edge bound 1)" in rendered
        assert EDGE_LEGEND not in rendered

    def test_a_negative_edge_bound_is_refused(self):
        graph = chain_graph(3)

        with pytest.raises(ValueError, match="max_edges"):
            flowchart_text(graph, uniform_rank(graph), options=DiagramOptions(max_edges=-1))


class TestBounding:
    def test_the_top_ranked_nodes_are_the_ones_drawn(self):
        graph = chain_graph(5)
        rank = {node: float(index) for index, node in enumerate(graph.nodes)}

        rendered = flowchart_text(graph, rank, options=DiagramOptions(max_nodes=2))

        assert '"src/m04.py"' in rendered
        assert '"src/m00.py"' not in rendered

    def test_the_elision_line_counts_what_was_left_out(self):
        graph = chain_graph(10)

        rendered = flowchart_text(graph, uniform_rank(graph), options=DiagramOptions(max_nodes=4))

        assert f'{ELISION_ID}["... 6 more modules"]' in rendered

    def test_one_elided_module_is_counted_in_the_singular(self):
        graph = chain_graph(3)

        rendered = flowchart_text(graph, uniform_rank(graph), options=DiagramOptions(max_nodes=2))

        assert f'{ELISION_ID}["... 1 more module"]' in rendered

    def test_a_complete_diagram_carries_no_elision_line(self):
        graph = chain_graph(3)

        assert ELISION_ID not in flowchart_text(graph, uniform_rank(graph))

    def test_max_nodes_must_be_positive(self):
        graph = chain_graph(3)

        with pytest.raises(ValueError, match="max_nodes"):
            flowchart_text(graph, uniform_rank(graph), options=DiagramOptions(max_nodes=0))


class TestReportedBounds:
    """What the render hands back, so no caller has to derive it again."""

    def test_the_export_counts_the_nodes_it_drew(self):
        graph = chain_graph(5)

        drawn = render_flowchart(graph, uniform_rank(graph), options=DiagramOptions(max_nodes=2))

        assert drawn.nodes == 2
        assert drawn.elided_nodes == 3

    def test_the_reported_elision_is_the_number_in_the_picture(self):
        graph = chain_graph(10)

        drawn = render_flowchart(graph, uniform_rank(graph), options=DiagramOptions(max_nodes=4))

        assert f'{ELISION_ID}["... {drawn.elided_nodes} more modules"]' in drawn.text

    def test_the_elision_is_counted_against_the_focus_neighbourhood(self):
        """A focus cuts the candidate set before the rank bound sees it.

        Counted against the whole graph instead, a focused diagram that
        dropped nothing still reported modules elided, and a reader raised
        `max_nodes` for modules no bound had removed.
        """
        graph = chain_graph(6)
        options = DiagramOptions(focus="src/m00.py", focus_distance=1)

        drawn = render_flowchart(graph, uniform_rank(graph), options=options)

        assert drawn.nodes == 2
        assert drawn.elided_nodes == 0

    def test_the_export_counts_the_reference_edges_the_bound_cut(self):
        graph = chain_graph(4)

        drawn = render_flowchart(
            graph,
            uniform_rank(graph),
            options=DiagramOptions(max_edges=1),
            imports=frozenset(),
        )

        assert drawn.edges_over_bound == 3
        assert "3 reference edges not drawn (edge bound 1)" in drawn.text

    def test_a_diagram_inside_its_edge_bound_reports_no_edge_elision(self):
        graph = chain_graph(4)

        drawn = render_flowchart(graph, uniform_rank(graph), imports=frozenset())

        assert drawn.edges_over_bound == 0

    def test_an_edge_dropped_with_its_endpoint_is_not_an_edge_bound_cut(self):
        """The two cuts stay apart, so a reader raises the knob that cut.

        Three of these edges lose an endpoint to `max_nodes`. None of them
        was cut by `max_edges`, and reporting them together would send a
        reader to a bound that removed nothing.
        """
        graph = chain_graph(5)

        drawn = render_flowchart(
            graph,
            uniform_rank(graph),
            options=DiagramOptions(max_nodes=2),
            imports=frozenset(),
        )

        assert drawn.elided_nodes == 3
        assert drawn.edges_over_bound == 0


class TestFocus:
    def test_only_the_seed_neighbourhood_is_drawn(self):
        graph = chain_graph(6)

        rendered = flowchart_text(
            graph,
            uniform_rank(graph),
            options=DiagramOptions(focus="src/m00.py", focus_distance=2),
        )

        assert '"src/m02.py"' in rendered
        assert '"src/m03.py"' not in rendered

    def test_focus_reaches_backwards_along_edges_too(self):
        graph = chain_graph(6)

        rendered = flowchart_text(
            graph,
            uniform_rank(graph),
            options=DiagramOptions(focus="src/m03.py", focus_distance=1),
        )

        assert '"src/m02.py"' in rendered
        assert '"src/m04.py"' in rendered

    def test_the_seed_survives_the_rank_bound(self):
        graph = chain_graph(6)
        rank = dict.fromkeys(graph.nodes, 0.0)
        rank["src/m02.py"] = 5.0

        rendered = flowchart_text(
            graph,
            rank,
            options=DiagramOptions(focus="src/m00.py", max_nodes=1),
        )

        assert '"src/m00.py"' in rendered

    def test_the_elision_counts_only_the_neighbourhood(self):
        graph = chain_graph(20)

        rendered = flowchart_text(
            graph,
            uniform_rank(graph),
            options=DiagramOptions(focus="src/m10.py", focus_distance=2, max_nodes=3),
        )

        assert f'{ELISION_ID}["... 2 more modules"]' in rendered

    def test_a_negative_focus_distance_is_refused(self):
        graph = chain_graph(3)

        with pytest.raises(ValueError, match="focus_distance"):
            flowchart_text(graph, uniform_rank(graph), options=DiagramOptions(focus_distance=-1))

    def test_a_focus_outside_the_graph_is_refused(self):
        graph = chain_graph(3)

        with pytest.raises(ValueError, match="focus"):
            flowchart_text(graph, uniform_rank(graph), options=DiagramOptions(focus="nowhere.py"))


class TestCommunities:
    def test_each_community_becomes_a_subgraph(self):
        graph = RefGraph(
            nodes=("src/a/one.py", "src/a/two.py", "src/b/one.py", "src/b/two.py"),
            edges={
                ("src/a/one.py", "src/a/two.py"): 9.0,
                ("src/a/two.py", "src/a/one.py"): 9.0,
                ("src/b/one.py", "src/b/two.py"): 9.0,
                ("src/b/two.py", "src/b/one.py"): 9.0,
                ("src/a/one.py", "src/b/one.py"): 1.0,
            },
        )
        partition = detect_communities(graph)

        rendered = flowchart_text(graph, personalized_pagerank(graph).rank, partition=partition)

        assert 'subgraph s0["src/a"]' in rendered
        assert 'subgraph s1["src/b"]' in rendered
        assert rendered.count("\n    end\n") == 2

    def test_communities_sharing_a_label_are_told_apart(self):
        # Two disconnected pairs, each straddling `a/` and `b/`, so neither
        # reaches a majority prefix and both label as the repository root.
        graph = RefGraph(
            nodes=("a/one.py", "b/one.py", "a/two.py", "b/two.py"),
            edges={
                ("a/one.py", "b/one.py"): 9.0,
                ("b/one.py", "a/one.py"): 9.0,
                ("a/two.py", "b/two.py"): 9.0,
                ("b/two.py", "a/two.py"): 9.0,
            },
        )
        partition = detect_communities(graph)

        rendered = flowchart_text(graph, personalized_pagerank(graph).rank, partition=partition)

        assert 'subgraph s0["repository root"]' in rendered
        assert 'subgraph s1["repository root 2"]' in rendered

    def test_a_distinct_label_takes_no_ordinal(self):
        graph = RefGraph(
            nodes=("src/a/one.py", "src/a/two.py"),
            edges={("src/a/one.py", "src/a/two.py"): 9.0},
        )
        partition = detect_communities(graph)

        rendered = flowchart_text(graph, personalized_pagerank(graph).rank, partition=partition)

        assert 'subgraph s0["src/a"]' in rendered
        assert "src/a 2" not in rendered

    def test_a_community_with_no_drawn_member_is_skipped(self):
        graph = chain_graph(4)
        partition = detect_communities(graph)
        rank = {node: float(index) for index, node in enumerate(graph.nodes)}

        rendered = flowchart_text(
            graph, rank, partition=partition, options=DiagramOptions(max_nodes=1)
        )

        assert rendered.count("subgraph") <= 1

    def test_ungrouped_nodes_are_drawn_at_the_top_level(self):
        graph = RefGraph(nodes=("a.py", "b.py"), edges={("a.py", "b.py"): 1.0})
        partition = detect_communities(RefGraph(nodes=("a.py",), edges={}))

        rendered = flowchart_text(graph, uniform_rank(graph), partition=partition)

        assert '    n1["b.py"]' in rendered


class TestDeterminism:
    def test_two_renders_are_byte_identical(self):
        graph = chain_graph(12)
        rank = personalized_pagerank(graph).rank
        options = DiagramOptions(max_nodes=8)

        first = flowchart_text(graph, rank, options=options)
        second = flowchart_text(graph, rank, options=options)

        assert first == second

    def test_node_and_edge_insertion_order_do_not_move_a_byte(self):
        forward = chain_graph(6)
        backward = RefGraph(
            nodes=tuple(reversed(forward.nodes)),
            edges=dict(reversed(list(forward.edges.items()))),
        )

        assert flowchart_text(backward, uniform_rank(backward)) == flowchart_text(
            forward, uniform_rank(forward)
        )

    def test_grouped_renders_are_byte_identical(self):
        graph = chain_graph(9)
        rank = personalized_pagerank(graph).rank
        partition = detect_communities(graph)

        first = flowchart_text(graph, rank, partition=partition)
        second = flowchart_text(graph, rank, partition=partition)

        assert first == second


class TestLabelSafety:
    def test_an_injected_filename_renders_as_an_inert_label(self):
        graph = RefGraph(nodes=(HOSTILE_NAME,), edges={})

        rendered = flowchart_text(graph, uniform_rank(graph))

        assert rendered == 'flowchart LR\n    n0["end_ click n0 href _x.py"]\n'

    def test_no_statement_begins_with_a_mermaid_directive(self):
        graph = RefGraph(nodes=(HOSTILE_NAME, "click.py", "style.py"), edges={})

        rendered = flowchart_text(graph, uniform_rank(graph))

        first_words = [line.split(" ")[0] for line in rendered.splitlines()[1:]]
        assert not [word for word in first_words if word.lower() in MERMAID_RESERVED_IDS - {"end"}]

    def test_quotes_brackets_and_semicolons_never_survive_into_a_label(self):
        assert safe_label('a"b]c;d`e{f}g') == "a_b_c_d_e_f_g"

    def test_newlines_and_tabs_are_flattened(self):
        assert safe_label("a\nb\tc") == "a_b_c"

    def test_html_entity_syntax_is_stripped(self):
        assert safe_label("#quot;x#35;") == "_quot_x_35_"

    def test_an_all_punctuation_name_still_gets_a_label(self):
        assert safe_label("###") == "_"

    def test_an_empty_name_gets_a_placeholder(self):
        assert safe_label("") == "unnamed"

    def test_a_very_long_name_is_truncated_with_a_marker(self):
        label = safe_label("a" * 200)

        assert len(label) == 60
        assert label.endswith("...")

    def test_ordinary_paths_pass_through_untouched(self):
        assert safe_label("src/agentless_mcp/core/graph.py") == "src/agentless_mcp/core/graph.py"

    def test_a_community_label_is_sanitised_too(self):
        graph = RefGraph(nodes=(f"{HOSTILE_NAME}/inner.py",), edges={})
        partition = detect_communities(graph)

        rendered = flowchart_text(graph, uniform_rank(graph), partition=partition)

        assert '"' not in rendered.replace('["', "").replace('"]', "")

    def test_generated_ids_are_never_mermaid_reserved_words(self):
        graph = RefGraph(nodes=tuple(f"m{index}.py" for index in range(50)), edges={})

        rendered = flowchart_text(graph, uniform_rank(graph), options=DiagramOptions(max_nodes=50))

        identifiers = [line.strip().split("[")[0] for line in rendered.splitlines()[1:]]
        assert not [name for name in identifiers if name.lower() in MERMAID_RESERVED_IDS]
