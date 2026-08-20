"""Self-contained HTML rendering for the bounded file-level graph.

The export is an on-demand human view, not stored graph state. Repository
paths and community labels enter one JSON payload with HTML-significant
characters escaped; the browser assigns them through ``textContent`` only.
No external scripts, styles, fonts, or network calls are present.
"""

import json
from collections.abc import Mapping, Set
from dataclasses import dataclass
from typing import Any

from agentless_mcp.core import mermaid
from agentless_mcp.core.communities import CommunityPartition
from agentless_mcp.core.graph import RefGraph

DEFAULT_MAX_NODES = 200
DEFAULT_MAX_EDGES = 600
MAX_NODES = 1_000
MAX_EDGES = 5_000


@dataclass(frozen=True)
class HtmlOptions:
    """Bounds applied before repository data reaches the document."""

    max_nodes: int = DEFAULT_MAX_NODES
    max_edges: int = DEFAULT_MAX_EDGES


@dataclass(frozen=True)
class HtmlExport:
    """One rendered document and an exact account of its bounds."""

    text: str
    nodes: int
    edges: int
    elided_nodes: int
    elided_edges: int
    communities: int


def render_html(
    graph: RefGraph,
    rank: Mapping[str, float],
    partition: CommunityPartition,
    *,
    imports: Set[tuple[str, str]],
    options: HtmlOptions | None = None,
) -> HtmlExport:
    """Render a searchable, clickable graph document with community colours."""
    settings = options if options is not None else HtmlOptions()
    _validate(settings)

    selection = mermaid.DiagramOptions(max_nodes=settings.max_nodes)
    selected = mermaid.selected_nodes(graph, rank, selection)
    selected_set = set(selected)
    identifiers = {path: f"n{position}" for position, path in enumerate(selected)}
    membership = partition.index_of()

    nodes = [
        {
            "id": identifiers[path],
            "path": path,
            "community": membership.get(path, -1),
            "rank": rank.get(path, 0.0),
        }
        for path in selected
    ]
    candidates = [
        {
            "source": identifiers[source],
            "target": identifiers[target],
            "kind": "import" if (source, target) in imports else "reference",
            "weight": weight,
        }
        for (source, target), weight in graph.edges.items()
        if source in selected_set and target in selected_set
    ]
    candidates.sort(
        key=lambda edge: (
            0 if edge["kind"] == "import" else 1,
            -float(edge["weight"]),
            str(edge["source"]),
            str(edge["target"]),
        )
    )
    edges = candidates[: settings.max_edges]
    represented = sorted({membership[path] for path in selected if path in membership})
    labels = {str(position): partition.communities[position].label for position in represented}
    elided_nodes = max(0, len(graph.nodes) - len(nodes))
    elided_edges = max(0, len(candidates) - len(edges))
    payload = {
        "nodes": nodes,
        "edges": edges,
        "communities": labels,
        "elidedNodes": elided_nodes,
        "elidedEdges": elided_edges,
    }
    document = _DOCUMENT.replace("__GRAPH_DATA__", _script_json(payload))
    return HtmlExport(
        text=document,
        nodes=len(nodes),
        edges=len(edges),
        elided_nodes=elided_nodes,
        elided_edges=elided_edges,
        communities=len(represented),
    )


def _validate(options: HtmlOptions) -> None:
    if not 1 <= options.max_nodes <= MAX_NODES:
        message = f"max_nodes must be between 1 and {MAX_NODES}"
        raise ValueError(message)
    if not 0 <= options.max_edges <= MAX_EDGES:
        message = f"max_edges must be between 0 and {MAX_EDGES}"
        raise ValueError(message)


def _script_json(value: Any) -> str:
    """Serialize data so repository text cannot close the script element."""
    encoded = json.dumps(value, ensure_ascii=True, separators=(",", ":"), sort_keys=True)
    return encoded.replace("<", "\\u003c").replace(">", "\\u003e").replace("&", "\\u0026")


_DOCUMENT = """<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>agentless-mcp graph</title>
  <style>
    :root { color-scheme: light dark; font-family: system-ui, sans-serif; }
    body { margin: 0; display: grid; grid-template-rows: auto 1fr; min-height: 100vh; }
    header { padding: 0.8rem 1rem; border-bottom: 1px solid #8888; display: flex;
      flex-wrap: wrap; gap: 0.8rem; align-items: center; }
    h1 { margin: 0; font-size: 1.05rem; }
    input { min-width: min(28rem, 70vw); padding: 0.45rem 0.6rem; }
    main { display: grid; grid-template-columns: minmax(0, 1fr) minmax(14rem, 22rem); }
    svg { width: 100%; height: calc(100vh - 4rem); min-height: 34rem; }
    aside { padding: 1rem; border-left: 1px solid #8888; overflow: auto; }
    #stats, #details { white-space: pre-wrap; overflow-wrap: anywhere; }
    #legend { display: grid; gap: 0.35rem; margin-top: 1rem; }
    .legend-row { display: flex; gap: 0.45rem; align-items: center; overflow-wrap: anywhere; }
    .swatch { width: 0.9rem; height: 0.9rem; border-radius: 50%; flex: 0 0 auto; }
    .edge { stroke: #778; stroke-opacity: 0.35; stroke-width: 1; }
    .edge.import { stroke: #444; stroke-opacity: 0.7; stroke-width: 1.6; }
    .node { cursor: pointer; outline: none; }
    .node circle { stroke: CanvasText; stroke-width: 1.2; }
    .node text { fill: CanvasText; font-size: 11px; pointer-events: none; }
    .node.dim, .edge.dim { opacity: 0.08; }
    .node.selected circle { stroke-width: 4; }
    .edge.selected { stroke-opacity: 1; stroke-width: 3; }
    @media (max-width: 800px) {
      main { grid-template-columns: 1fr; }
      aside { border-left: 0; border-top: 1px solid #8888; }
    }
  </style>
</head>
<body>
  <header>
    <h1>agentless-mcp graph</h1>
    <label>Search <input id="search" type="search" placeholder="file path"></label>
  </header>
  <main>
    <svg id="graph" viewBox="0 0 1200 800" role="img" aria-label="Repository graph">
      <defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5"
        markerWidth="6" markerHeight="6" orient="auto-start-reverse">
        <path d="M 0 0 L 10 5 L 0 10 z" fill="context-stroke"></path>
      </marker></defs>
    </svg>
    <aside>
      <strong>Selection</strong>
      <p id="details">Click a node or search for a path.</p>
      <strong>Bounds</strong>
      <p id="stats"></p>
      <strong>Communities</strong>
      <div id="legend"></div>
    </aside>
  </main>
  <script>
    "use strict";
    const DATA = __GRAPH_DATA__;
    const SVG_NS = "http://www.w3.org/2000/svg";
    const svg = document.getElementById("graph");
    const search = document.getElementById("search");
    const details = document.getElementById("details");
    const stats = document.getElementById("stats");
    const legend = document.getElementById("legend");
    const byId = new Map(DATA.nodes.map(node => [node.id, node]));
    const nodeElements = new Map();
    const edgeElements = [];

    function colour(community) {
      if (community < 0) return "hsl(0 0% 55%)";
      return `hsl(${(community * 137.508) % 360} 62% 55%)`;
    }

    function svgElement(name, attributes = {}) {
      const element = document.createElementNS(SVG_NS, name);
      for (const [key, value] of Object.entries(attributes)) element.setAttribute(key, value);
      return element;
    }

    function shortLabel(path) {
      return path.length > 34 ? `...${path.slice(-31)}` : path;
    }

    function draw() {
      const count = Math.max(DATA.nodes.length, 1);
      const radius = Math.min(330, 90 + count * 3.2);
      const positions = new Map();
      DATA.nodes.forEach((node, index) => {
        const angle = (Math.PI * 2 * index / count) - Math.PI / 2;
        positions.set(node.id, [600 + radius * Math.cos(angle), 400 + radius * Math.sin(angle)]);
      });
      DATA.edges.forEach(edge => {
        const [x1, y1] = positions.get(edge.source);
        const [x2, y2] = positions.get(edge.target);
        const line = svgElement("line", {x1, y1, x2, y2, "marker-end": "url(#arrow)"});
        line.classList.add("edge", edge.kind);
        line.dataset.source = edge.source;
        line.dataset.target = edge.target;
        svg.appendChild(line);
        edgeElements.push(line);
      });
      DATA.nodes.forEach(node => {
        const [x, y] = positions.get(node.id);
        const group = svgElement("g", {transform: `translate(${x} ${y})`, tabindex: "0",
          role: "button", "aria-label": node.path});
        group.classList.add("node");
        group.dataset.id = node.id;
        const circle = svgElement("circle", {r: "8", fill: colour(node.community)});
        const label = svgElement("text", {x: "12", y: "4"});
        label.textContent = shortLabel(node.path);
        const title = svgElement("title");
        title.textContent = node.path;
        group.append(circle, label, title);
        group.addEventListener("click", () => select(node.id));
        group.addEventListener("keydown", event => {
          if (event.key === "Enter" || event.key === " ") select(node.id);
        });
        svg.appendChild(group);
        nodeElements.set(node.id, group);
      });
    }

    function select(id) {
      const node = byId.get(id);
      const neighbours = new Set([id]);
      DATA.edges.forEach(edge => {
        if (edge.source === id) neighbours.add(edge.target);
        if (edge.target === id) neighbours.add(edge.source);
      });
      nodeElements.forEach((element, nodeId) => {
        element.classList.toggle("selected", nodeId === id);
        element.classList.toggle("dim", !neighbours.has(nodeId));
      });
      edgeElements.forEach(element => {
        const connected = element.dataset.source === id || element.dataset.target === id;
        element.classList.toggle("selected", connected);
        element.classList.toggle("dim", !connected);
      });
      const outgoing = DATA.edges.filter(edge => edge.source === id).length;
      const incoming = DATA.edges.filter(edge => edge.target === id).length;
      const community = DATA.communities[String(node.community)] || "unassigned";
      details.textContent =
        `${node.path}\ncommunity: ${community}\nin: ${incoming}  out: ${outgoing}`;
    }

    search.addEventListener("input", () => {
      const query = search.value.trim().toLowerCase();
      if (!query) {
        nodeElements.forEach(element => element.classList.remove("dim", "selected"));
        edgeElements.forEach(element => element.classList.remove("dim", "selected"));
        details.textContent = "Click a node or search for a path.";
        return;
      }
      const matches = DATA.nodes.filter(node => node.path.toLowerCase().includes(query));
      const matchedIds = new Set(matches.map(node => node.id));
      nodeElements.forEach((element, id) => element.classList.toggle("dim", !matchedIds.has(id)));
      edgeElements.forEach(element => element.classList.add("dim"));
      details.textContent = matches.length
        ? matches.map(node => node.path).join("\n") : "No matches";
    });

    stats.textContent = `${DATA.nodes.length} nodes, ${DATA.edges.length} edges\n` +
      `${DATA.elidedNodes} nodes elided, ${DATA.elidedEdges} edges elided`;
    Object.entries(DATA.communities).forEach(([id, label]) => {
      const row = document.createElement("div");
      row.className = "legend-row";
      const swatch = document.createElement("span");
      swatch.className = "swatch";
      swatch.style.background = colour(Number(id));
      const text = document.createElement("span");
      text.textContent = label;
      row.append(swatch, text);
      legend.appendChild(row);
    });
    draw();
  </script>
</body>
</html>
"""
