"""Mermaid rendering: determinism, bounding, and label safety."""

import pytest

from agentless_mcp.core.communities import detect_communities
from agentless_mcp.core.graph import RefGraph, personalized_pagerank
from agentless_mcp.core.mermaid import (
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


class TestShape:
    def test_the_header_names_the_direction(self):
        graph = chain_graph(3)

        assert render_flowchart(graph, uniform_rank(graph)).startswith("flowchart LR\n")

    def test_ids_are_generated_in_sorted_path_order(self):
        graph = RefGraph(nodes=("b.py", "a.py"), edges={})

        rendered = render_flowchart(graph, uniform_rank(graph))

        assert '    n0["a.py"]' in rendered
        assert '    n1["b.py"]' in rendered

    def test_edges_are_rendered_between_generated_ids(self):
        graph = RefGraph(nodes=("a.py", "b.py"), edges={("a.py", "b.py"): 1.0})

        assert "    n0 --> n1" in render_flowchart(graph, uniform_rank(graph))

    def test_edge_weights_are_never_printed(self):
        graph = RefGraph(nodes=("a.py", "b.py"), edges={("a.py", "b.py"): 3.5})

        assert "3.5" not in render_flowchart(graph, uniform_rank(graph))

    def test_edges_to_elided_nodes_are_dropped(self):
        graph = chain_graph(6)

        rendered = render_flowchart(graph, uniform_rank(graph), options=DiagramOptions(max_nodes=2))

        assert rendered.count(" --> ") == 1

    def test_output_carries_no_markdown_fence(self):
        graph = chain_graph(3)

        assert "```" not in render_flowchart(graph, uniform_rank(graph))

    def test_an_empty_graph_renders_only_the_header(self):
        assert render_flowchart(RefGraph(nodes=(), edges={}), {}) == "flowchart LR\n"

    def test_a_node_the_ranking_forgot_is_still_a_candidate(self):
        graph = RefGraph(nodes=("a.py", "b.py"), edges={})

        rendered = render_flowchart(graph, {"a.py": 1.0})

        assert '"b.py"' in rendered


class TestBounding:
    def test_the_top_ranked_nodes_are_the_ones_drawn(self):
        graph = chain_graph(5)
        rank = {node: float(index) for index, node in enumerate(graph.nodes)}

        rendered = render_flowchart(graph, rank, options=DiagramOptions(max_nodes=2))

        assert '"src/m04.py"' in rendered
        assert '"src/m00.py"' not in rendered

    def test_the_elision_line_counts_what_was_left_out(self):
        graph = chain_graph(10)

        rendered = render_flowchart(graph, uniform_rank(graph), options=DiagramOptions(max_nodes=4))

        assert f'{ELISION_ID}["... 6 more modules"]' in rendered

    def test_a_complete_diagram_carries_no_elision_line(self):
        graph = chain_graph(3)

        assert ELISION_ID not in render_flowchart(graph, uniform_rank(graph))

    def test_max_nodes_must_be_positive(self):
        graph = chain_graph(3)

        with pytest.raises(ValueError, match="max_nodes"):
            render_flowchart(graph, uniform_rank(graph), options=DiagramOptions(max_nodes=0))

    def test_an_unknown_direction_is_refused(self):
        graph = chain_graph(3)

        with pytest.raises(ValueError, match="direction"):
            render_flowchart(graph, uniform_rank(graph), options=DiagramOptions(direction="UP"))

    def test_a_valid_direction_reaches_the_header(self):
        graph = chain_graph(2)

        rendered = render_flowchart(
            graph, uniform_rank(graph), options=DiagramOptions(direction="TD")
        )

        assert rendered.startswith("flowchart TD\n")


class TestFocus:
    def test_only_the_seed_neighbourhood_is_drawn(self):
        graph = chain_graph(6)

        rendered = render_flowchart(
            graph,
            uniform_rank(graph),
            options=DiagramOptions(focus="src/m00.py", focus_distance=2),
        )

        assert '"src/m02.py"' in rendered
        assert '"src/m03.py"' not in rendered

    def test_focus_reaches_backwards_along_edges_too(self):
        graph = chain_graph(6)

        rendered = render_flowchart(
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

        rendered = render_flowchart(
            graph,
            rank,
            options=DiagramOptions(focus="src/m00.py", max_nodes=1),
        )

        assert '"src/m00.py"' in rendered

    def test_the_elision_counts_only_the_neighbourhood(self):
        graph = chain_graph(20)

        rendered = render_flowchart(
            graph,
            uniform_rank(graph),
            options=DiagramOptions(focus="src/m10.py", focus_distance=2, max_nodes=3),
        )

        assert f'{ELISION_ID}["... 2 more modules"]' in rendered

    def test_a_focus_outside_the_graph_is_refused(self):
        graph = chain_graph(3)

        with pytest.raises(ValueError, match="focus"):
            render_flowchart(graph, uniform_rank(graph), options=DiagramOptions(focus="nowhere.py"))


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

        rendered = render_flowchart(graph, personalized_pagerank(graph), partition=partition)

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

        rendered = render_flowchart(graph, personalized_pagerank(graph), partition=partition)

        assert 'subgraph s0["repository root"]' in rendered
        assert 'subgraph s1["repository root 2"]' in rendered

    def test_a_distinct_label_takes_no_ordinal(self):
        graph = RefGraph(
            nodes=("src/a/one.py", "src/a/two.py"),
            edges={("src/a/one.py", "src/a/two.py"): 9.0},
        )
        partition = detect_communities(graph)

        rendered = render_flowchart(graph, personalized_pagerank(graph), partition=partition)

        assert 'subgraph s0["src/a"]' in rendered
        assert "src/a 2" not in rendered

    def test_a_community_with_no_drawn_member_is_skipped(self):
        graph = chain_graph(4)
        partition = detect_communities(graph)
        rank = {node: float(index) for index, node in enumerate(graph.nodes)}

        rendered = render_flowchart(
            graph, rank, partition=partition, options=DiagramOptions(max_nodes=1)
        )

        assert rendered.count("subgraph") <= 1

    def test_ungrouped_nodes_are_drawn_at_the_top_level(self):
        graph = RefGraph(nodes=("a.py", "b.py"), edges={("a.py", "b.py"): 1.0})
        partition = detect_communities(RefGraph(nodes=("a.py",), edges={}))

        rendered = render_flowchart(graph, uniform_rank(graph), partition=partition)

        assert '    n1["b.py"]' in rendered


class TestDeterminism:
    def test_two_renders_are_byte_identical(self):
        graph = chain_graph(12)
        rank = personalized_pagerank(graph)
        options = DiagramOptions(max_nodes=8)

        first = render_flowchart(graph, rank, options=options)
        second = render_flowchart(graph, rank, options=options)

        assert first == second

    def test_node_and_edge_insertion_order_do_not_move_a_byte(self):
        forward = chain_graph(6)
        backward = RefGraph(
            nodes=tuple(reversed(forward.nodes)),
            edges=dict(reversed(list(forward.edges.items()))),
        )

        assert render_flowchart(backward, uniform_rank(backward)) == render_flowchart(
            forward, uniform_rank(forward)
        )

    def test_grouped_renders_are_byte_identical(self):
        graph = chain_graph(9)
        rank = personalized_pagerank(graph)
        partition = detect_communities(graph)

        first = render_flowchart(graph, rank, partition=partition)
        second = render_flowchart(graph, rank, partition=partition)

        assert first == second


class TestLabelSafety:
    def test_an_injected_filename_renders_as_an_inert_label(self):
        graph = RefGraph(nodes=(HOSTILE_NAME,), edges={})

        rendered = render_flowchart(graph, uniform_rank(graph))

        assert rendered == 'flowchart LR\n    n0["end_ click n0 href _x.py"]\n'

    def test_no_statement_begins_with_a_mermaid_directive(self):
        graph = RefGraph(nodes=(HOSTILE_NAME, "click.py", "style.py"), edges={})

        rendered = render_flowchart(graph, uniform_rank(graph))

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

        rendered = render_flowchart(graph, uniform_rank(graph), partition=partition)

        assert '"' not in rendered.replace('["', "").replace('"]', "")

    def test_generated_ids_are_never_mermaid_reserved_words(self):
        graph = RefGraph(nodes=tuple(f"m{index}.py" for index in range(50)), edges={})

        rendered = render_flowchart(
            graph, uniform_rank(graph), options=DiagramOptions(max_nodes=50)
        )

        identifiers = [line.strip().split("[")[0] for line in rendered.splitlines()[1:]]
        assert not [name for name in identifiers if name.lower() in MERMAID_RESERVED_IDS]
