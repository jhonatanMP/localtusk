import { useEffect, useMemo, useRef, useState } from "react";
import {
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  MarkerType,
  MiniMap,
  Position,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
  type Edge,
  type Node,
  type NodeProps,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import dagre from "@dagrejs/dagre";
import type { CodeFile, FileKind } from "./api";

const W = 210;
const H = 44;
const GAP = 14;
/** Acima disso o desenho vira uma sopa de linhas; mostramos só um recorte. */
const MAX_NODES = 400;

const COLOR: Record<FileKind, string> = {
  entry: "#3ecf8e",
  normal: "#8892bf",
  orphan: "#f5b84a",
  unreachable: "#ff6b7a",
};
const LABEL: Record<FileKind, string> = {
  entry: "Entrada",
  normal: "Em uso",
  orphan: "Sem uso",
  unreachable: "Só usado por órfãos",
};

type FileNode = Node<{ file: CodeFile; dim: boolean; picked: boolean }, "file">;
type LabelNode = Node<{ text: string }, "label">;
type GraphNode = FileNode | LabelNode;

function splitPath(path: string) {
  const i = path.lastIndexOf("/");
  return i < 0 ? { dir: "", name: path } : { dir: path.slice(0, i + 1), name: path.slice(i + 1) };
}

function FileNodeView({ data }: NodeProps<FileNode>) {
  const { file, dim, picked } = data;
  const { dir, name } = splitPath(file.path);
  return (
    <div className={`gnode ${file.kind} ${dim ? "dim" : ""} ${picked ? "picked" : ""}`} title={`${file.path}\n${LABEL[file.kind]}`}>
      <Handle type="target" position={Position.Left} isConnectable={false} />
      <span className="gdir">{dir || "/"}</span>
      <span className="gname">{name}</span>
      <Handle type="source" position={Position.Right} isConnectable={false} />
    </div>
  );
}

function LabelNodeView({ data }: NodeProps<LabelNode>) {
  return <div className="glabel">{data.text}</div>;
}

const nodeTypes = { file: FileNodeView, label: LabelNodeView };

/** Arquivos até `depth` passos de distância, nos dois sentidos. */
function neighborhood(byPath: Map<string, CodeFile>, start: string, depth: number) {
  const seen = new Set([start]);
  let edge = [start];
  for (let d = 0; d < depth && edge.length; d++) {
    const next: string[] = [];
    for (const p of edge) {
      const f = byPath.get(p);
      if (!f) continue;
      for (const l of [...f.uses, ...f.usedBy]) {
        if (!seen.has(l.path) && byPath.has(l.path)) {
          seen.add(l.path);
          next.push(l.path);
        }
      }
    }
    edge = next;
  }
  return seen;
}

interface Visible {
  keep: Set<string>;
  trimmed: boolean;
}

function pickVisible(files: CodeFile[], byPath: Map<string, CodeFile>, focus: string | null, depth: number, withIsolated: boolean): Visible {
  if (focus && byPath.has(focus)) {
    const keep = neighborhood(byPath, focus, depth);
    if (keep.size <= MAX_NODES) return { keep, trimmed: false };
    return { keep: new Set([...keep].slice(0, MAX_NODES)), trimmed: true };
  }
  const list = files.filter((f) => withIsolated || f.uses.length + f.usedBy.length > 0);
  if (list.length <= MAX_NODES) return { keep: new Set(list.map((f) => f.path)), trimmed: false };
  // Projeto grande: as entradas e o que elas chamam diretamente.
  const keep = new Set<string>();
  for (const f of list) {
    if (f.kind !== "entry") continue;
    keep.add(f.path);
    for (const l of f.uses) keep.add(l.path);
    if (keep.size >= MAX_NODES) break;
  }
  if (keep.size === 0) {
    [...list]
      .sort((a, b) => b.uses.length + b.usedBy.length - (a.uses.length + a.usedBy.length))
      .slice(0, MAX_NODES)
      .forEach((f) => keep.add(f.path));
  }
  return { keep: new Set([...keep].slice(0, MAX_NODES)), trimmed: true };
}

interface Layout {
  pos: Map<string, { x: number; y: number }>;
  edges: { from: string; to: string; via: "include" | "classe" | "hook" }[];
  isolated: number;
  labelAt: { x: number; y: number } | null;
}

function buildLayout(files: CodeFile[], keep: Set<string>): Layout {
  const edgeMap = new Map<string, Layout["edges"][number]>();
  for (const f of files) {
    if (!keep.has(f.path)) continue;
    for (const l of f.uses) {
      if (l.path === f.path || !keep.has(l.path)) continue;
      const k = `${f.path}\n${l.path}`;
      const cur = edgeMap.get(k);
      if (!cur) edgeMap.set(k, { from: f.path, to: l.path, via: l.via });
      else if (l.via === "include") cur.via = "include";
    }
  }
  const edges = [...edgeMap.values()];
  const linked = new Set<string>();
  for (const e of edges) {
    linked.add(e.from);
    linked.add(e.to);
  }

  const g = new dagre.graphlib.Graph();
  g.setGraph({ rankdir: "LR", nodesep: GAP, ranksep: 70, marginx: 0, marginy: 0 });
  g.setDefaultEdgeLabel(() => ({}));
  for (const p of linked) g.setNode(p, { width: W, height: H });
  for (const e of edges) g.setEdge(e.from, e.to);
  dagre.layout(g);

  const pos = new Map<string, { x: number; y: number }>();
  for (const p of linked) {
    const n = g.node(p);
    pos.set(p, { x: n.x - W / 2, y: n.y - H / 2 });
  }
  const gw = linked.size ? (g.graph().width ?? 0) : 0;
  const gh = linked.size ? (g.graph().height ?? 0) : 0;

  // Arquivos sem nenhuma ligação ficam numa grade embaixo, para não bagunçar o desenho.
  const isolated = files.filter((f) => keep.has(f.path) && !linked.has(f.path)).map((f) => f.path);
  const cols = Math.min(8, Math.max(3, Math.floor(gw / (W + GAP))));
  const top = linked.size ? gh + 80 : 36;
  isolated.forEach((p, i) => pos.set(p, { x: (i % cols) * (W + GAP), y: top + Math.floor(i / cols) * (H + GAP) }));

  return { pos, edges, isolated: isolated.length, labelAt: isolated.length ? { x: 0, y: top - 34 } : null };
}

export interface CodeGraphProps {
  files: CodeFile[];
  selected: string | null;
  onSelect: (path: string | null) => void;
  query: string;
  onQuery: (q: string) => void;
}

export function CodeGraph(props: CodeGraphProps) {
  return (
    <ReactFlowProvider>
      <GraphInner {...props} />
    </ReactFlowProvider>
  );
}

function GraphInner({ files, selected, onSelect, query, onQuery }: CodeGraphProps) {
  const { fitView, setCenter, getZoom } = useReactFlow();
  const [focusOn, setFocusOn] = useState(false);
  const [depth, setDepth] = useState(2);
  const [withIsolated, setWithIsolated] = useState(true);
  const fromGraph = useRef(false);

  const byPath = useMemo(() => new Map(files.map((f) => [f.path, f])), [files]);
  const focus = focusOn && selected ? selected : null;

  const visible = useMemo(() => pickVisible(files, byPath, focus, depth, withIsolated), [files, byPath, focus, depth, withIsolated]);
  const layout = useMemo(() => buildLayout(files, visible.keep), [files, visible]);

  const matches = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return null;
    const set = new Set<string>();
    for (const p of layout.pos.keys()) {
      const f = byPath.get(p);
      if (f && (p.toLowerCase().includes(q) || f.classes.some((c) => c.toLowerCase().includes(q)))) set.add(p);
    }
    return set;
  }, [query, layout, byPath]);

  const related = useMemo(() => {
    const f = selected ? byPath.get(selected) : null;
    if (!f) return null;
    return new Set([f.path, ...f.uses.map((l) => l.path), ...f.usedBy.map((l) => l.path)]);
  }, [selected, byPath]);

  const nodes = useMemo<GraphNode[]>(() => {
    const out: GraphNode[] = [];
    for (const [p, at] of layout.pos) {
      const file = byPath.get(p);
      if (!file) continue;
      const dim = matches ? !matches.has(p) : related ? !related.has(p) : false;
      out.push({ id: p, type: "file", position: at, width: W, height: H, data: { file, dim, picked: p === selected } });
    }
    if (layout.labelAt) {
      out.push({
        id: "__isolados",
        type: "label",
        position: layout.labelAt,
        selectable: false,
        draggable: false,
        data: { text: `Sem ligações com outros arquivos (${layout.isolated})` },
      });
    }
    return out;
  }, [layout, byPath, matches, related, selected]);

  const edges = useMemo<Edge[]>(
    () =>
      layout.edges.map((e) => {
        const hot = selected !== null && (e.from === selected || e.to === selected);
        const color = hot ? "#b9c0ff" : "#565b80";
        return {
          id: `${e.from}>${e.to}`,
          source: e.from,
          target: e.to,
          zIndex: hot ? 10 : 0,
          markerEnd: { type: MarkerType.ArrowClosed, color, width: 14, height: 14 },
          style: {
            stroke: color,
            strokeWidth: hot ? 2 : 1.2,
            strokeDasharray: e.via === "classe" ? "5 4" : e.via === "hook" ? "2 3" : undefined,
            opacity: selected && !hot ? 0.22 : 0.9,
          },
        };
      }),
    [layout, selected],
  );

  // Ajusta o enquadramento sempre que o conjunto desenhado muda.
  useEffect(() => {
    const t = setTimeout(() => fitView({ padding: 0.12, maxZoom: 1, duration: 200 }), 60);
    return () => clearTimeout(t);
  }, [layout, fitView]);

  // Escolher um arquivo fora do grafo (lista, links do painel) leva a vista até ele.
  useEffect(() => {
    if (fromGraph.current) {
      fromGraph.current = false;
      return;
    }
    const at = selected ? layout.pos.get(selected) : null;
    if (at && !focus) setCenter(at.x + W / 2, at.y + H / 2, { zoom: Math.max(getZoom(), 0.8), duration: 250 });
  }, [selected]); // eslint-disable-line react-hooks/exhaustive-deps

  const linkCount = layout.edges.length;
  // Projetos pequenos não precisam de uma tela alta e vazia; os grandes ganham até 720 px.
  let bottom = 0;
  for (const at of layout.pos.values()) bottom = Math.max(bottom, at.y + H);
  const canvasHeight = Math.min(720, Math.max(300, bottom + 60));

  return (
    <div className="graph">
      <div className="graph-bar">
        <input
          type="search"
          placeholder="Buscar…"
          aria-label="Buscar arquivo ou classe"
          value={query}
          onChange={(e) => onQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && matches && matches.size) onSelect([...matches][0]);
          }}
        />
        <div className="log-files" role="group" aria-label="O que desenhar">
          <button className={`chip-btn ${!focusOn ? "active" : ""}`} onClick={() => setFocusOn(false)}>
            Projeto todo
          </button>
          <button
            className={`chip-btn ${focusOn ? "active" : ""}`}
            disabled={!selected}
            title={selected ? "Só o que está ligado ao arquivo escolhido" : "Escolha um arquivo primeiro"}
            onClick={() => setFocusOn(true)}
          >
            Ao redor do escolhido
          </button>
          {focusOn && (
            <span className="graph-depth" role="group" aria-label="Distância">
              {[1, 2, 3].map((d) => (
                <button key={d} className={`chip-btn ${depth === d ? "active" : ""}`} onClick={() => setDepth(d)} title={`${d} ${d === 1 ? "passo" : "passos"} de distância`}>
                  {d}
                </button>
              ))}
            </span>
          )}
        </div>
        <label className="graph-check">
          <input type="checkbox" checked={withIsolated} onChange={(e) => setWithIsolated(e.target.checked)} /> Mostrar sem ligações
        </label>
      </div>

      <div className="graph-legend" aria-label="Legenda">
        {(Object.keys(COLOR) as FileKind[]).map((k) => (
          <span key={k}>
            <i style={{ background: COLOR[k] }} /> {LABEL[k]}
          </span>
        ))}
        <span>
          <svg width="26" height="8" aria-hidden="true">
            <line x1="0" y1="4" x2="26" y2="4" stroke="#8a90b8" strokeWidth="1.5" />
          </svg>{" "}
          require/include
        </span>
        <span>
          <svg width="26" height="8" aria-hidden="true">
            <line x1="0" y1="4" x2="26" y2="4" stroke="#8a90b8" strokeWidth="1.5" strokeDasharray="5 4" />
          </svg>{" "}
          classe
        </span>
        <span>
          <svg width="26" height="8" aria-hidden="true">
            <line x1="0" y1="4" x2="26" y2="4" stroke="#8a90b8" strokeWidth="1.5" strokeDasharray="2 3" />
          </svg>{" "}
          hook do WordPress
        </span>
        <span className="graph-count">
          {layout.pos.size} arquivos · {linkCount} ligações
        </span>
      </div>

      {visible.trimmed && (
        <div className="banner warn">
          <span>
            {focus
              ? `Muitos arquivos ligados a este: mostrando os primeiros ${MAX_NODES}. Diminua a distância.`
              : `Projeto grande demais para desenhar inteiro. Mostrando as entradas e o que elas chamam. Clique num arquivo e use “Ao redor do escolhido” para ir mais fundo.`}
          </span>
        </div>
      )}

      <div className="graph-canvas" style={{ height: canvasHeight }}>
        <ReactFlow
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          colorMode="dark"
          fitView
          fitViewOptions={{ padding: 0.12, maxZoom: 1 }}
          minZoom={0.05}
          maxZoom={1.6}
          nodesDraggable={false}
          nodesConnectable={false}
          onlyRenderVisibleElements
          onNodeClick={(_, n) => {
            if (n.type !== "file") return;
            if (n.id !== selected) fromGraph.current = true;
            onSelect(n.id);
          }}
          onPaneClick={() => {
            if (!focusOn) onSelect(null);
          }}
        >
          <Background variant={BackgroundVariant.Dots} gap={22} size={1} color="#2b2e42" />
          <Controls showInteractive={false} />
          <MiniMap pannable zoomable nodeColor={(n) => (n.type === "file" ? COLOR[(n as FileNode).data.file.kind] : "transparent")} maskColor="rgba(19,20,28,0.7)" />
        </ReactFlow>
      </div>
    </div>
  );
}
