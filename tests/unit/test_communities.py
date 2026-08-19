"""Greedy-modularity community detection over the file-level graph."""

import pytest

from agentless_mcp.core.communities import (
    ROOT_LABEL,
    community_label,
    detect_communities,
)
from agentless_mcp.core.graph import RefGraph

# Two directory-shaped clusters joined by a single light edge. The bridge is
# what makes the fixture worth having: a detector that merges on any
# connection at all returns one community here, and a detector that splits on
# every weak tie returns six.
CLUSTER_A = ("src/a/one.py", "src/a/two.py", "src/a/three.py")
CLUSTER_B = ("src/b/one.py", "src/b/two.py", "src/b/three.py")


def clustered_edges():
    """Two dense triangles plus one bridge edge between them."""
    edges = {}
    for cluster in (CLUSTER_A, CLUSTER_B):
        for source in cluster:
            for target in cluster:
                if source != target:
                    edges[(source, target)] = 5.0
    edges[("src/a/one.py", "src/b/one.py")] = 1.0
    return edges


def clustered_graph(nodes=None):
    """The fixture graph, optionally with its node tuple in another order."""
    return RefGraph(nodes=nodes or (*CLUSTER_A, *CLUSTER_B), edges=clustered_edges())


class TestDetection:
    def test_the_two_clusters_are_found(self):
        partition = detect_communities(clustered_graph())

        assert [community.members for community in partition.communities] == [
            tuple(sorted(CLUSTER_A)),
            tuple(sorted(CLUSTER_B)),
        ]

    def test_the_partition_scores_as_real_structure(self):
        partition = detect_communities(clustered_graph())

        assert partition.modularity > 0.3

    def test_communities_are_labelled_from_their_member_paths(self):
        partition = detect_communities(clustered_graph())

        assert [community.label for community in partition.communities] == ["src/a", "src/b"]

    def test_the_bridge_does_not_merge_the_clusters(self):
        partition = detect_communities(clustered_graph())

        index = partition.index_of()
        assert index["src/a/one.py"] != index["src/b/one.py"]

    def test_weights_are_reported_per_community(self):
        partition = detect_communities(clustered_graph())

        first = partition.communities[0]
        # Six directed edges of weight 5 inside the triangle, symmetrised to
        # 12 endpoint contributions, plus this cluster's half of the bridge.
        assert first.internal_weight == 60.0
        assert first.total_weight == 61.0

    def test_a_singleton_graph_is_one_community(self):
        partition = detect_communities(RefGraph(nodes=("only.py",), edges={}))

        assert partition.communities[0].members == ("only.py",)
        assert partition.modularity == 0.0

    def test_an_empty_graph_partitions_into_nothing(self):
        partition = detect_communities(RefGraph(nodes=(), edges={}))

        assert partition.communities == ()
        assert partition.modularity == 0.0

    def test_an_edgeless_graph_becomes_singletons_not_a_blob(self):
        graph = RefGraph(nodes=("b.py", "a.py", "c.py"), edges={})

        partition = detect_communities(graph)

        assert [community.members for community in partition.communities] == [
            ("a.py",),
            ("b.py",),
            ("c.py",),
        ]

    def test_communities_are_ordered_largest_first(self):
        edges = dict(clustered_edges())
        edges[("src/a/one.py", "src/a/four.py")] = 5.0
        edges[("src/a/four.py", "src/a/two.py")] = 5.0
        graph = RefGraph(nodes=(*CLUSTER_A, "src/a/four.py", *CLUSTER_B), edges=edges)

        sizes = [community.size for community in detect_communities(graph).communities]

        assert sizes == sorted(sizes, reverse=True)

    def test_edges_naming_unknown_nodes_are_refused_at_construction(self):
        # This graph used to be constructible, and the detector quietly
        # dropped the dangling edge. RefGraph now refuses it, so the detector
        # can never be handed one -- the assertion moved from "the partition
        # ignored it" to "the graph could not exist".
        with pytest.raises(ValueError, match=r"vendor/gone\.py"):
            RefGraph(
                nodes=("a.py", "b.py"),
                edges={("a.py", "b.py"): 1.0, ("a.py", "vendor/gone.py"): 99.0},
            )


class TestDeterminism:
    def test_two_runs_produce_the_identical_partition(self):
        graph = clustered_graph()

        assert detect_communities(graph).as_dict() == detect_communities(graph).as_dict()

    def test_node_order_does_not_change_the_answer(self):
        shuffled = ("src/b/two.py", "src/a/three.py", "src/b/one.py")
        rest = tuple(node for node in (*CLUSTER_A, *CLUSTER_B) if node not in shuffled)

        expected = detect_communities(clustered_graph()).as_dict()
        actual = detect_communities(clustered_graph(nodes=(*shuffled, *rest))).as_dict()

        assert actual == expected

    def test_edge_insertion_order_does_not_change_the_answer(self):
        forward = clustered_edges()
        reversed_insertion = dict(reversed(list(forward.items())))

        expected = detect_communities(clustered_graph()).as_dict()
        actual = detect_communities(
            RefGraph(nodes=(*CLUSTER_A, *CLUSTER_B), edges=reversed_insertion)
        ).as_dict()

        assert actual == expected


class TestResolution:
    def test_a_higher_resolution_never_yields_fewer_communities(self):
        graph = clustered_graph()

        coarse = detect_communities(graph, resolution=0.5)
        fine = detect_communities(graph, resolution=4.0)

        assert len(fine.communities) >= len(coarse.communities)

    def test_the_resolution_used_is_reported(self):
        assert detect_communities(clustered_graph(), resolution=0.5).resolution == 0.5


class TestLabels:
    def test_the_deepest_shared_directory_wins(self):
        assert community_label(["src/adapters/cli.py", "src/adapters/mcp.py"]) == "src/adapters"

    def test_one_outlier_does_not_collapse_the_label(self):
        members = ["src/adapters/cli.py", "src/adapters/mcp.py", "README.md"]

        assert community_label(members) == "src/adapters"

    def test_top_level_files_get_the_root_label(self):
        assert community_label(["setup.py", "README.md"]) == ROOT_LABEL

    def test_an_empty_community_gets_the_root_label(self):
        assert community_label([]) == ROOT_LABEL

    def test_an_even_split_falls_back_to_the_shared_parent(self):
        members = ["src/a/one.py", "src/b/one.py", "src/a/two.py", "src/b/two.py"]

        assert community_label(members) == "src"

    def test_an_even_split_with_no_shared_parent_gets_the_root_label(self):
        assert community_label(["z/one.py", "a/one.py"]) == ROOT_LABEL

    def test_a_majority_outvotes_a_minority_at_the_same_depth(self):
        assert community_label(["a/one.py", "a/two.py", "z/one.py"]) == "a"
