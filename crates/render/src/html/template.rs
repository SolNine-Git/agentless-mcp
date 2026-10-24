//! The document of the HTML export. The search results join on the JS escape `\n`, never on a
//! raw newline, so the script parses.

pub(super) const DOCUMENT: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta http-equiv="Content-Security-Policy"
    content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'">
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
        `${node.path}
community: ${community}
in: ${incoming}  out: ${outgoing}`;
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

    stats.textContent = `${DATA.nodes.length} nodes, ${DATA.edges.length} edges
` +
      `${DATA.elidedNodes} nodes elided (node bound ${DATA.nodeBound})
` +
      `${DATA.edgesWithoutBothNodes} edges elided with them
` +
      `${DATA.edgesOverBound} edges past the edge bound (${DATA.edgeBound})`;
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
"#;
