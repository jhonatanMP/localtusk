import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { CodeFile, CodeMap, FileKind, Project, api } from "./api";
import { Icon, Spinner } from "./ui";

function errMsg(e: unknown) {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}

// O desenho (React Flow + dagre) só é baixado quando alguém abre o Grafo.
const CodeGraph = lazy(() => import("./CodeGraph").then((m) => ({ default: m.CodeGraph })));

const KIND_LABEL: Record<FileKind, string> = {
  entry: "Entrada",
  normal: "Em uso",
  orphan: "Sem uso",
  unreachable: "Só usado por órfãos",
};

type Filter = "all" | FileKind;

function splitPath(path: string) {
  const i = path.lastIndexOf("/");
  return i < 0 ? { dir: "", name: path } : { dir: path.slice(0, i + 1), name: path.slice(i + 1) };
}

function Badge({ kind }: { kind: FileKind }) {
  return <span className={`map-badge ${kind}`}>{KIND_LABEL[kind]}</span>;
}

/** Aba Mapa: mostra quem chama quem e quais arquivos ninguém usa. */
export function CodeMapTab({ project, onError }: { project: Project; onError: (m: string) => void }) {
  const [map, setMap] = useState<CodeMap | null>(null);
  const [loading, setLoading] = useState(true);
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [view, setView] = useState<"list" | "graph">("list");
  // `onError` muda a cada renderização do pai; guardá-lo aqui evita reler o projeto sem necessidade.
  const report = useRef(onError);
  report.current = onError;

  const load = useCallback(() => {
    setLoading(true);
    return api
      .analyzeProject(project.id)
      .then((m) => {
        setMap(m);
        setSelected((cur) => (cur && m.files.some((f) => f.path === cur) ? cur : null));
      })
      .catch((e) => report.current(errMsg(e)))
      .finally(() => setLoading(false));
  }, [project.id]);

  useEffect(() => {
    setMap(null);
    setSelected(null);
    load();
  }, [load]);

  const counts = useMemo(() => {
    const c: Record<Filter, number> = { all: 0, entry: 0, normal: 0, orphan: 0, unreachable: 0 };
    for (const f of map?.files ?? []) {
      c.all++;
      c[f.kind]++;
    }
    return c;
  }, [map]);

  const shown = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (map?.files ?? []).filter(
      (f) => (filter === "all" || f.kind === filter) && (!q || f.path.toLowerCase().includes(q) || f.classes.some((c) => c.toLowerCase().includes(q))),
    );
  }, [map, filter, query]);

  const byPath = useMemo(() => new Map((map?.files ?? []).map((f) => [f.path, f])), [map]);
  const current = selected ? byPath.get(selected) ?? null : null;

  if (!map) {
    return (
      <div className="panel map-loading" aria-live="polite">
        <Spinner /> Lendo os arquivos do projeto…
      </div>
    );
  }

  const detailPanel = (
    <aside className="panel map-detail" aria-live="polite">
      {current ? (
        <Detail file={current} onPick={setSelected} />
      ) : (
        <>
          <h3>
            <Icon name="layers" /> Como ler o mapa
          </h3>
          <p className="muted small">
            Escolha um arquivo para ver o que ele chama e quem o chama. <b>Sem uso</b> é um arquivo que nenhum outro do projeto carrega.{" "}
            <b>Só usados por órfãos</b> só é chamado por arquivos sem uso, ou seja, também está parado.
          </p>
          <p className="muted small">
            A leitura é estática: ela segue <code>require</code>/<code>include</code> com caminho fixo e classes ligadas por{" "}
            <code>use</code>, <code>new</code>, <code>extends</code> e afins, e hooks do WordPress com nome e callback escritos no código. Arquivos chamados por variável, hook com nome montado na hora, rota ou tarefa agendada
            podem aparecer como <b>sem uso</b> por engano. Trate a lista como pistas, não como ordem para apagar.
          </p>
          <p className="muted small">
            São <b>entradas</b>: arquivos da pasta pública, <code>index.php</code>, o autoload do Composer, <code>bin/</code> e <code>tests/</code>.
          </p>
        </>
      )}
    </aside>
  );

  const filters: { id: Filter; label: string }[] = [
    { id: "all", label: "Todos" },
    { id: "orphan", label: "Sem uso" },
    { id: "unreachable", label: "Só usados por órfãos" },
    { id: "entry", label: "Entradas" },
  ];

  return (
    <div className="map">
      <div className="map-summary">
        <div className="map-stat">
          <b>{map.files.length}</b>
          <span>arquivos PHP</span>
        </div>
        <div className="map-stat">
          <b>{map.totalLinks}</b>
          <span>ligações</span>
        </div>
        <div className="map-stat">
          <b>{map.entries}</b>
          <span>entradas</span>
        </div>
        <div className={`map-stat ${map.orphans ? "warn" : ""}`}>
          <b>{map.orphans}</b>
          <span>sem uso</span>
        </div>
        <div className={`map-stat ${map.unreachable ? "warn" : ""}`}>
          <b>{map.unreachable}</b>
          <span>só usados por órfãos</span>
        </div>
        <button className="btn small" onClick={() => load()} disabled={loading} title="Ler os arquivos de novo">
          {loading ? <Spinner size={13} /> : <Icon name="restore" size={13} />} Atualizar
        </button>
      </div>

      {map.notes.map((n) => (
        <div key={n} className="banner warn">
          <Icon name="alert" />
          <span>{n}</span>
        </div>
      ))}

      <div className="map-bar" role="tablist" aria-label="Forma de ver o mapa">
        <button role="tab" aria-selected={view === "list"} className={view === "list" ? "active" : ""} onClick={() => setView("list")}>
          <Icon name="layers" size={14} /> Lista
        </button>
        <button role="tab" aria-selected={view === "graph"} className={view === "graph" ? "active" : ""} onClick={() => setView("graph")}>
          <Icon name="blueprint" size={14} /> Grafo
        </button>
      </div>

      <div className={`map-body ${view === "graph" ? "graph-view" : ""}`}>
        {view === "graph" ? (
          <div className="panel map-canvas">
            <Suspense
              fallback={
                <div className="map-loading">
                  <Spinner /> Preparando o grafo…
                </div>
              }
            >
              <CodeGraph files={map.files} selected={selected} onSelect={setSelected} query={query} onQuery={setQuery} />
            </Suspense>
          </div>
        ) : (
        <div className="panel map-list">
          <div className="map-tools">
            <input
              type="search"
              placeholder="Buscar arquivo ou classe…"
              aria-label="Buscar arquivo ou classe"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            <div className="log-files" role="tablist" aria-label="Filtro">
              {filters.map((f) => (
                <button key={f.id} role="tab" aria-selected={filter === f.id} className={`chip-btn ${filter === f.id ? "active" : ""}`} onClick={() => setFilter(f.id)}>
                  {f.label} <span className="map-n">{counts[f.id]}</span>
                </button>
              ))}
            </div>
          </div>

          {map.files.length === 0 ? (
            <p className="muted small">Nenhum arquivo PHP encontrado na pasta do projeto.</p>
          ) : shown.length === 0 ? (
            <p className="muted small">Nenhum arquivo neste filtro.</p>
          ) : (
            <ul className="map-files">
              {shown.map((f) => {
                const { dir, name } = splitPath(f.path);
                return (
                  <li key={f.path}>
                    <button className={`map-file ${f.kind} ${selected === f.path ? "sel" : ""}`} onClick={() => setSelected(f.path)}>
                      <Icon name="file" size={14} />
                      <span className="map-name">
                        <span className="dir">{dir}</span>
                        {name}
                      </span>
                      <span className="map-io" title={`chama ${f.uses.length}, chamado por ${f.usedBy.length}`}>
                        {f.uses.length}→ ←{f.usedBy.length}
                      </span>
                      {f.kind !== "normal" && <Badge kind={f.kind} />}
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
        )}

        {detailPanel}
      </div>
    </div>
  );
}

function Detail({ file, onPick }: { file: CodeFile; onPick: (path: string) => void }) {
  const { dir, name } = splitPath(file.path);
  return (
    <>
      <h3 className="map-title">
        <Icon name="file" />
        <span>
          <span className="dir">{dir}</span>
          {name}
        </span>
      </h3>
      <div className="map-meta">
        <Badge kind={file.kind} />
        {file.entryReason && <span className="muted small">{file.entryReason}</span>}
        <span className="muted small">{file.lines} linhas</span>
      </div>

      {file.classes.length > 0 && (
        <div className="map-classes">
          {file.classes.map((c) => (
            <code key={c}>{c}</code>
          ))}
        </div>
      )}

      {file.hooks.length > 0 && (
        <div className="map-classes">
          <span className="muted small">Hooks que registra:</span>
          {file.hooks.map((h) => (
            <code key={h}>{h}</code>
          ))}
        </div>
      )}

      {file.kind === "orphan" && (
        <div className="banner warn">
          <Icon name="alert" />
          <span>Nenhum arquivo do projeto chama este. Pode estar parado, ou ser chamado de um jeito que a leitura não enxerga (variável, hook com nome montado na hora, rota, tarefa agendada).</span>
        </div>
      )}
      {file.kind === "unreachable" && (
        <div className="banner warn">
          <Icon name="alert" />
          <span>Só é chamado por arquivos sem uso. Se eles estiverem parados, este também está.</span>
        </div>
      )}
      {file.dynamicIncludes > 0 && (
        <p className="muted small">
          Tem {file.dynamicIncludes} <code>require</code>/<code>include</code> com variável, que a leitura não consegue seguir.
        </p>
      )}

      <LinkList title="Chama" empty="Não chama nenhum outro arquivo do projeto." links={file.uses} onPick={onPick} />
      <LinkList title="Chamado por" empty="Nenhum arquivo do projeto chama este." links={file.usedBy} onPick={onPick} />
    </>
  );
}

function LinkList(props: { title: string; empty: string; links: CodeFile["uses"]; onPick: (path: string) => void }) {
  return (
    <section className="map-links">
      <h4>
        {props.title} <span className="count">{props.links.length}</span>
      </h4>
      {props.links.length === 0 ? (
        <p className="muted small">{props.empty}</p>
      ) : (
        <ul>
          {props.links.map((l) => {
            const { dir, name } = splitPath(l.path);
            return (
              <li key={`${l.path}:${l.via}`}>
                <button className="map-link" onClick={() => props.onPick(l.path)}>
                  <Icon name="chevronRight" size={12} />
                  <span className="map-name">
                    <span className="dir">{dir}</span>
                    {name}
                  </span>
                  <span className="map-via">{l.via === "include" ? "require" : l.via}</span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
