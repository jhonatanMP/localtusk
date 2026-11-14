import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import AccessPanel from "./Access";
import ApiTools from "./ApiTools";
import BackupPanel, { pickBackupFile } from "./Backup";
import BlueprintsView, { SaveBlueprintPanel } from "./Blueprint";
import { askConfirm } from "./confirm";
import LocalImport from "./LocalImport";
import {
  BlueprintInfo,
  DbEngine,
  ExtraStatus,
  InstallProgress,
  LOG_FILES,
  MysqlInstall,
  PhpInstall,
  PostgresInstall,
  Project,
  ToolId,
  ToolStatus,
  ValkeyInstall,
  api,
  resolveMysql,
  resolvePhp,
  resolvePostgres,
} from "./api";
import PhpManager, { ProgressView, isBusy } from "./PhpManager";
import RedisPanel from "./Redis";
import WordPressPanel from "./WordPress";
import { CodeMapTab } from "./CodeMap";
import { Copyable, Icon, IconName, Led, Logo, Skeleton, Spinner } from "./ui";
import "./App.css";

function errMsg(e: unknown) {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}

export default function App() {
  const [projects, setProjects] = useState<Project[]>([]);
  /** A primeira leitura da lista de projetos já terminou (evita piscar "nenhum projeto" ao abrir). */
  const [loaded, setLoaded] = useState(false);
  const lastProjects = useRef("");
  const mainRef = useRef<HTMLElement>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [view, setView] = useState<"project" | "new" | "php" | "blueprints" | "local-import">("project");
  const [tools, setTools] = useState<ToolStatus[]>([]);
  const [phps, setPhps] = useState<PhpInstall[]>([]);
  const [mysqls, setMysqls] = useState<MysqlInstall[]>([]);
  const [postgreses, setPostgreses] = useState<PostgresInstall[]>([]);
  const [valkeys, setValkeys] = useState<ValkeyInstall[]>([]);
  const [extras, setExtras] = useState<ExtraStatus[]>([]);
  const [progress, setProgress] = useState<Record<string, InstallProgress>>({});
  const [error, setError] = useState<string | null>(null);
  const [backupsDir, setBackupsDir] = useState("");
  const [blueprintsDir, setBlueprintsDir] = useState("");
  /** Criando um projeto novo a partir de um backup ou de um blueprint. */
  const [restoringNew, setRestoringNew] = useState<null | "backup" | "blueprint" | "local">(null);

  const loadPhps = useCallback(() => {
    api.listPhpVersions().then(setPhps);
    api.listMysqlVersions().then(setMysqls);
    api.listPostgresVersions().then(setPostgreses);
    api.listValkeyVersions().then(setValkeys);
    api.checkTools().then(setTools);
    api.extrasStatus().then(setExtras);
  }, []);

  const refresh = useCallback(async () => {
    try {
      const list = await api.listProjects();
      // A lista é relida a cada 2 s: só atualiza a tela quando algo mudou de verdade.
      const signature = JSON.stringify(list);
      if (signature !== lastProjects.current) {
        lastProjects.current = signature;
        setProjects(list);
      }
      setLoaded(true);
    } catch (e) {
      setError(errMsg(e));
    }
  }, []);

  useEffect(() => {
    refresh();
    loadPhps();
    const t = setInterval(refresh, 2000);
    return () => clearInterval(t);
  }, [refresh, loadPhps]);

  // Progresso das instalações do PHP (continua mesmo se o usuário trocar de tela).
  useEffect(() => {
    api.appInfo().then((info) => {
      setBackupsDir(info.backupsDir);
      setBlueprintsDir(info.blueprintsDir);
      setProgress((prev) => {
        const next = { ...prev };
        for (const v of info.installing) {
          next[v] ??= { id: v, stage: "download", downloaded: 0, total: 0, message: null };
        }
        return next;
      });
    });
    const un = listen<InstallProgress>("install-progress", (e) => {
      setProgress((prev) => ({ ...prev, [e.payload.id]: e.payload }));
      if (e.payload.stage === "done") loadPhps();
    });
    return () => {
      un.then((f) => f());
    };
  }, [loadPhps]);

  const selected = useMemo(
    () => projects.find((p) => p.id === selectedId) ?? null,
    [projects, selectedId],
  );

  // Ao trocar de tela ou de projeto, a nova tela começa no topo (e não onde a anterior parou).
  useEffect(() => {
    mainRef.current?.scrollTo({ top: 0, behavior: "instant" as ScrollBehavior });
  }, [view, selectedId]);

  const missing = tools.filter((t) => !t.found);
  const installingCount = Object.values(progress).filter(
    (p) => p.stage === "download" || p.stage === "extract" || p.stage === "verify",
  ).length;

  const restoreProgress = Object.values(progress).find((p) => p.id.startsWith("restore-") && isBusy(p));

  /** Cria um projeto novo a partir de um arquivo de backup. */
  async function restoreAsNew() {
    try {
      const path = await pickBackupFile(backupsDir);
      if (!path) return;
      setRestoringNew("backup");
      const r = await api.restoreBackup(path);
      await refresh();
      setSelectedId(r.project.id);
      setView("project");
      if (r.warning) setError(r.warning);
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setRestoringNew(null);
      refresh();
    }
  }

  /** Cria um projeto novo a partir de um blueprint e abre o projeto. */
  async function createFromBlueprint(b: BlueprintInfo, name: string) {
    try {
      setRestoringNew("blueprint");
      const r = await api.createProjectFromBlueprint(b.path, name);
      await refresh();
      setSelectedId(r.project.id);
      setView("project");
      if (r.warning) setError(r.warning);
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setRestoringNew(null);
      refresh();
    }
  }

  /** Importa um site do Local e abre o projeto criado. */
  async function importFromLocal(id: string, name: string, phpSeries: string, mysqlSeries: string) {
    try {
      setRestoringNew("local");
      const r = await api.importLocalSite(id, name, phpSeries, mysqlSeries);
      await refresh();
      setSelectedId(r.project.id);
      setView("project");
      if (r.warning) setError(r.warning);
    } catch (e) {
      setError(errMsg(e));
    } finally {
      setRestoringNew(null);
      refresh();
    }
  }

  const caddyReady = extras.find((x) => x.id === "caddy")?.installed ?? false;

  return (
    <div className="layout">
      <aside className="sidebar">
        <div className="brand">
          <Logo size={30} />
          <div>
            <div className="brand-name">LocalTusk</div>
            <div className="brand-sub">ambientes PHP locais</div>
          </div>
        </div>

        <div className="side-label">Projetos</div>
        <nav className="project-list" aria-label="Projetos">
          {!loaded && <Skeleton kind="row-item" count={3} />}
          {loaded && projects.length === 0 && <p className="muted small pad">Nenhum projeto ainda.</p>}
          {projects.map((p) => (
            <button
              key={p.id}
              className={`project-item ${p.id === selectedId && view === "project" ? "active" : ""}`}
              onClick={() => {
                setSelectedId(p.id);
                setView("project");
              }}
            >
              <Led state={p.busy ? "busy" : p.running ? "on" : "off"} />
              <span className="grow ellipsis">{p.name}</span>
              <span className="port">:{p.httpPort}</span>
            </button>
          ))}
        </nav>

        <div className="side-actions">
          <button className={`nav-btn ${view === "php" ? "active" : ""}`} onClick={() => setView("php")}>
            <Icon name="layers" />
            <span className="grow">PHP e ferramentas</span>
            {installingCount > 0 && <span className="badge">{installingCount}</span>}
          </button>
          <button className={`nav-btn ${view === "blueprints" ? "active" : ""}`} onClick={() => setView("blueprints")}>
            <Icon name="blueprint" />
            <span className="grow">Blueprints</span>
          </button>
          <button className="btn block" onClick={restoreAsNew} disabled={restoringNew !== null}>
            <Icon name="restore" /> Restaurar backup
          </button>
          <button className="btn primary block" onClick={() => setView("new")}>
            <Icon name="plus" /> Novo projeto
          </button>
        </div>
      </aside>

      <main className="main" ref={mainRef}>
        <div className="content">
          {missing.length > 0 && (
            <div className="banner warn">
              <Icon name="alert" />
              <div>
                <strong>Ferramentas ausentes:</strong>{" "}
                {missing.map((t) =>
                  t.name === "PHP-FPM" ? (
                    <span key={t.name} className="chip">
                      PHP →{" "}
                      <button className="link" onClick={() => setView("php")}>
                        instalar uma versão
                      </button>
                    </span>
                  ) : (
                    <span key={t.name} className="chip">
                      {t.name} → <code>{t.installHint}</code>
                    </span>
                  ),
                )}
              </div>
            </div>
          )}
          {restoringNew && (
            <div className="banner" role="status">
              <Icon name={restoringNew === "blueprint" ? "blueprint" : restoringNew === "local" ? "download" : "restore"} />
              <div className="grow">
                <strong>
                  {restoringNew === "blueprint"
                    ? "Criando o projeto a partir do blueprint…"
                    : restoringNew === "local"
                      ? "Importando o site do Local…"
                      : "Restaurando o backup em um novo projeto…"}
                </strong>
                {restoreProgress && <ProgressView p={restoreProgress} />}
              </div>
            </div>
          )}
          {error && (
            <button className="banner err" onClick={() => setError(null)}>
              <Icon name="alert" />
              <div>
                {error} <span className="muted">(clique para fechar)</span>
              </div>
            </button>
          )}

          {view === "php" ? (
            <PhpManager
              phps={phps}
              mysqls={mysqls}
              postgreses={postgreses}
              valkeys={valkeys}
              extras={extras}
              progress={progress}
              onChanged={loadPhps}
              onError={setError}
            />
          ) : view === "blueprints" ? (
            <BlueprintsView
              blueprintsDir={blueprintsDir}
              creating={restoringNew !== null}
              onCreate={createFromBlueprint}
              onError={setError}
            />
          ) : view === "local-import" ? (
            <LocalImport
              progress={progress}
              importing={restoringNew !== null}
              onImport={importFromLocal}
              onInstalled={loadPhps}
              onBack={() => setView("new")}
              onError={setError}
            />
          ) : view === "new" ? (
            <NewProject
              phps={phps}
              mysqls={mysqls}
              postgreses={postgreses}
              valkeys={valkeys}
              onManagePhp={() => setView("php")}
              onBlueprints={() => setView("blueprints")}
              onLocalImport={() => setView("local-import")}
              onCancel={() => setView("project")}
              onCreated={async (p) => {
                await refresh();
                setSelectedId(p.id);
                setView("project");
              }}
              onError={setError}
            />
          ) : !loaded ? (
            <div className="project" aria-busy="true" aria-label="Carregando">
              <Skeleton kind="title" />
              <Skeleton kind="card" count={2} />
            </div>
          ) : selected ? (
            <ProjectView
              key={selected.id}
              project={selected}
              phps={phps}
              mysqls={mysqls}
              postgreses={postgreses}
              valkeys={valkeys}
              extras={extras}
              progress={progress}
              backupsDir={backupsDir}
              onManage={() => setView("php")}
              onBlueprints={() => setView("blueprints")}
              onChanged={refresh}
              onDeleted={() => {
                setSelectedId(null);
                refresh();
              }}
              onError={setError}
            />
          ) : (
            <Welcome
              steps={[
                { done: caddyReady, label: "Servidor web (Caddy)" },
                { done: phps.length > 0, label: "Uma versão do PHP" },
                { done: mysqls.length > 0 || postgreses.length > 0, label: "Um banco de dados (MySQL ou PostgreSQL)" },
              ]}
              onManage={() => setView("php")}
              onNew={() => setView("new")}
            />
          )}
        </div>
      </main>
    </div>
  );
}

function Welcome(props: {
  steps: { done: boolean; label: string }[];
  onManage: () => void;
  onNew: () => void;
}) {
  const ready = props.steps.every((s) => s.done);
  return (
    <div className="welcome">
      <Logo size={56} />
      <h1>Bem-vindo ao LocalTusk</h1>
      <p className="muted">O melhor ambiente para desenvolver em PHP.</p>
      <ol className="steps">
        {props.steps.map((s, i) => (
          <li key={s.label} className={s.done ? "done" : ""}>
            <span className="step-mark">{s.done ? <Icon name="check" size={14} /> : i + 1}</span>
            <span className="grow">{s.label}</span>
            <span className="small muted">{s.done ? "pronto" : "faltando"}</span>
          </li>
        ))}
      </ol>
      <div className="row center">
        {!ready && (
          <button className="btn primary" onClick={props.onManage}>
            <Icon name="download" /> Instalar o que falta
          </button>
        )}
        <button className={`btn ${ready ? "primary" : ""}`} onClick={props.onNew}>
          <Icon name="plus" /> Novo projeto
        </button>
      </div>
    </div>
  );
}

function NewProject(props: {
  phps: PhpInstall[];
  mysqls: MysqlInstall[];
  postgreses: PostgresInstall[];
  valkeys: ValkeyInstall[];
  onManagePhp: () => void;
  onBlueprints: () => void;
  onCancel: () => void;
  onLocalImport: () => void;
  onCreated: (p: Project) => void;
  onError: (m: string) => void;
}) {
  const [name, setName] = useState("");
  const [php, setPhp] = useState("");
  const [engine, setEngine] = useState<DbEngine>("mysql");
  const [db, setDb] = useState("");
  const [redis, setRedis] = useState(false);
  const [docroot, setDocroot] = useState("");
  const [saving, setSaving] = useState(false);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setSaving(true);
    try {
      props.onCreated(await api.createProject(name, php, db, docroot, engine, redis && props.valkeys.length > 0));
    } catch (err) {
      props.onError(errMsg(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="project">
      <header className="project-head">
        <div className="title">
          <h1>Novo projeto</h1>
          <div className="meta">
            Escolha as versões e o LocalTusk cuida do resto. Prefere partir de um modelo?{" "}
            <button type="button" className="link" onClick={props.onBlueprints}>
              Usar um blueprint
            </button>{" "}
            ou{" "}
            <button type="button" className="link" onClick={props.onLocalImport}>
              Importar do Local
            </button>
          </div>
        </div>
      </header>
      <form className="panel form" onSubmit={submit}>
        <div className="field">
          <label className="field-label" htmlFor="np-name">Nome</label>
          <input id="np-name" autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder="meu-projeto" />
        </div>
        <div className="field-row">
          <div className="field">
            <label className="field-label" htmlFor="np-php">Versão do PHP</label>
            <select id="np-php" value={php} onChange={(e) => setPhp(e.target.value)}>
              <option value="">Mais recente instalada</option>
              {props.phps.map((p) => (
                <option key={p.fpm} value={p.fullVersion}>
                  PHP {p.fullVersion}
                  {p.managed ? "" : " (sistema)"}
                </option>
              ))}
            </select>
            <span className="hint">
              {props.phps.length === 0 ? "Nenhuma versão instalada. " : "Precisa de outra? "}
              <button type="button" className="link" onClick={props.onManagePhp}>
                Gerenciar versões
              </button>
            </span>
          </div>
          <div className="field">
            <label className="field-label" htmlFor="np-engine">Banco de dados</label>
            <select
              id="np-engine"
              value={engine}
              onChange={(e) => {
                setEngine(e.target.value as DbEngine);
                setDb("");
              }}
            >
              <option value="mysql">MySQL</option>
              <option value="postgres">PostgreSQL</option>
            </select>
            <span className="hint">Cada projeto usa um: MySQL ou PostgreSQL.</span>
          </div>
        </div>
        <div className="field-row">
          <div className="field">
            <label className="field-label" htmlFor="np-db">Versão do {engine === "postgres" ? "PostgreSQL" : "MySQL"}</label>
            <select id="np-db" value={db} onChange={(e) => setDb(e.target.value)}>
              <option value="">Mais recente instalada</option>
              {engine === "postgres"
                ? props.postgreses.map((m) => (
                    <option key={m.postgres} value={m.fullVersion}>
                      PostgreSQL {m.fullVersion}
                    </option>
                  ))
                : props.mysqls.map((m) => (
                    <option key={m.mysqld} value={m.fullVersion}>
                      MySQL {m.fullVersion}
                      {m.managed ? "" : " (sistema)"}
                    </option>
                  ))}
            </select>
            <span className="hint">
              {(engine === "postgres" ? props.postgreses : props.mysqls).length === 0
                ? "Nenhuma versão instalada. "
                : "Tipo e versão ficam fixos depois de criado o banco. "}
              <button type="button" className="link" onClick={props.onManagePhp}>
                Gerenciar versões
              </button>
            </span>
          </div>
          <div className="field">
            <span className="field-label" id="np-redis-label">Redis</span>
            <label className="check" style={{ marginTop: 0, height: 36 }}>
              <input
                type="checkbox"
                aria-labelledby="np-redis-label"
                checked={redis && props.valkeys.length > 0}
                disabled={props.valkeys.length === 0}
                onChange={(e) => setRedis(e.target.checked)}
              />{" "}
              Ligar o Redis neste projeto
            </label>
            <span className="hint">
              {props.valkeys.length === 0 ? "O Redis (Valkey) ainda não está instalado. " : "Dá para ligar ou desligar depois. "}
              {props.valkeys.length === 0 && (
                <button type="button" className="link" onClick={props.onManagePhp}>
                  Instalar
                </button>
              )}
            </span>
          </div>
        </div>
        <div className="field">
          <label className="field-label" htmlFor="np-docroot">
            Pasta pública <span className="muted">(opcional)</span>
          </label>
          <input id="np-docroot" value={docroot} onChange={(e) => setDocroot(e.target.value)} placeholder="raiz do projeto, ex.: public" />
          <span className="hint">Onde fica o index.php do site, dentro da pasta do projeto.</span>
        </div>
        <div className="row">
          <button className="btn primary" disabled={saving || !name.trim()}>
            {saving ? "Criando…" : "Criar projeto"}
          </button>
          <button type="button" className="btn ghost" onClick={props.onCancel}>
            Cancelar
          </button>
        </div>
      </form>
    </div>
  );
}

function ProjectView(props: {
  project: Project;
  phps: PhpInstall[];
  mysqls: MysqlInstall[];
  postgreses: PostgresInstall[];
  valkeys: ValkeyInstall[];
  extras: ExtraStatus[];
  progress: Record<string, InstallProgress>;
  backupsDir: string;
  onManage: () => void;
  onBlueprints: () => void;
  onChanged: () => void;
  onDeleted: () => void;
  onError: (m: string) => void;
}) {
  const { project: p } = props;
  const [tab, setTab] = useState<"overview" | "api" | "logs" | "map">("overview");
  const [working, setWorking] = useState<null | "start" | "stop">(null);

  async function run<T>(kind: "start" | "stop", fn: () => Promise<T>) {
    setWorking(kind);
    try {
      await fn();
    } catch (e) {
      props.onError(errMsg(e));
    } finally {
      setWorking(null);
      props.onChanged();
    }
  }

  const status = working === "start" ? "Iniciando…" : working === "stop" ? "Parando…" : p.running ? "Rodando" : "Parado";
  const disabled = working !== null || p.busy;

  return (
    <div className="project">
      <header className="project-head">
        <div className="title">
          <div className="title-row">
            <Led state={working || p.busy ? "busy" : p.running ? "on" : "off"} />
            <h1>{p.name}</h1>
            <span className={`pill ${p.running ? "on" : ""}`}>{status}</span>
          </div>
          <div className="meta">
            <Copyable value={p.url} label="endereço" />
            <span className="sep">·</span>
            <span>
              pasta pública <code>app/{p.docroot}</code>
            </span>
          </div>
        </div>
        <div className="actions">
          {p.running ? (
            <>
              <button className="btn primary" onClick={() => api.openSite(p.id).catch((e) => props.onError(errMsg(e)))}>
                <Icon name="external" /> Abrir site
              </button>
              {p.hasAdminneo && <ToolButton project={p} tool="adminneo" label="Adminer Neo" icon="database" onError={props.onError} />}
              {p.hasMailpit && <ToolButton project={p} tool="mailpit" label="Mailpit" icon="mail" onError={props.onError} />}
              <button className="btn danger" disabled={disabled} onClick={() => run("stop", () => api.stopProject(p.id))}>
                {working === "stop" ? <Spinner /> : <Icon name="stop" />} {working === "stop" ? "Parando…" : "Parar"}
              </button>
            </>
          ) : (
            <button className="btn primary" disabled={disabled} onClick={() => run("start", () => api.startProject(p.id))}>
              {working === "start" ? <Spinner /> : <Icon name="play" />} {working === "start" ? "Iniciando…" : "Iniciar"}
            </button>
          )}
        </div>
      </header>

      <nav className="tabs" role="tablist">
        <button role="tab" aria-selected={tab === "overview"} className={tab === "overview" ? "active" : ""} onClick={() => setTab("overview")}>
          Visão geral
        </button>
        <button role="tab" aria-selected={tab === "api"} className={tab === "api" ? "active" : ""} onClick={() => setTab("api")}>
          API
        </button>
        <button role="tab" aria-selected={tab === "logs"} className={tab === "logs" ? "active" : ""} onClick={() => setTab("logs")}>
          Logs
        </button>
        <button role="tab" aria-selected={tab === "map"} className={tab === "map" ? "active" : ""} onClick={() => setTab("map")}>
          Mapa
        </button>
      </nav>

      {tab === "overview" ? (
        <Overview {...props} locked={disabled || p.running} />
      ) : tab === "api" ? (
        <ApiTools project={p} onError={props.onError} />
      ) : tab === "map" ? (
        <CodeMapTab project={p} onError={props.onError} />
      ) : (
        <Logs id={p.id} />
      )}
    </div>
  );
}

function ToolButton(props: {
  project: Project;
  tool: ToolId;
  label: string;
  icon: IconName;
  onError: (m: string) => void;
}) {
  return (
    <button
      className="btn"
      onClick={() => api.openTool(props.project.id, props.tool).catch((e) => props.onError(errMsg(e)))}
    >
      <Icon name={props.icon} /> {props.label}
    </button>
  );
}

/** Leitura de um componente do ambiente (PHP, MySQL, servidor), em destaque. */
function Tile(props: {
  icon: IconName;
  label: string;
  value: string;
  sub?: string;
  children?: React.ReactNode;
  hint?: React.ReactNode;
}) {
  return (
    <section className="tile">
      <div className="tile-label">
        <Icon name={props.icon} size={15} /> {props.label}
      </div>
      <div className="readout">{props.value}</div>
      {props.sub && <div className="tile-sub">{props.sub}</div>}
      {props.children}
      {props.hint && <div className="hint">{props.hint}</div>}
    </section>
  );
}

function Overview(props: {
  project: Project;
  phps: PhpInstall[];
  mysqls: MysqlInstall[];
  postgreses: PostgresInstall[];
  valkeys: ValkeyInstall[];
  extras: ExtraStatus[];
  progress: Record<string, InstallProgress>;
  backupsDir: string;
  onManage: () => void;
  onBlueprints: () => void;
  locked: boolean;
  onChanged: () => void;
  onDeleted: () => void;
  onError: (m: string) => void;
}) {
  const { project: p } = props;
  const resolved = resolvePhp(props.phps, p.phpVersion);
  const isPg = p.dbEngine === "postgres";
  const dbName = isPg ? "PostgreSQL" : "MySQL";
  const resolvedDb = isPg
    ? resolvePostgres(props.postgreses, p.dbVersion)
    : resolveMysql(props.mysqls, p.dbVersion);
  const dbInstalls: { key: string; version: string; managed: boolean }[] = isPg
    ? props.postgreses.map((m) => ({ key: m.postgres, version: m.fullVersion, managed: true }))
    : props.mysqls.map((m) => ({ key: m.mysqld, version: m.fullVersion, managed: m.managed }));
  const dbLocked = props.locked || p.dbInitialized;
  const changeDb = (engine: DbEngine, version: string) =>
    api.setDatabase(p.id, engine, version).then(props.onChanged).catch((err) => props.onError(errMsg(err)));

  async function del() {
    const ok = await askConfirm({
      title: `Apagar "${p.name}"?`,
      message: "Os arquivos e o banco de dados deste projeto serão removidos para sempre. Isso não pode ser desfeito.",
      confirmLabel: "Apagar projeto",
      danger: true,
    });
    if (!ok) return;
    try {
      await api.deleteProject(p.id);
      props.onDeleted();
    } catch (e) {
      props.onError(errMsg(e));
    }
  }

  return (
    <div className="overview">
      <div className="tiles">
        <Tile
          icon="code"
          label="PHP"
          value={resolved ? resolved.fullVersion : "—"}
          sub={resolved ? (resolved.managed ? "instalado pelo LocalTusk" : "do sistema (Homebrew)") : undefined}
          hint={
            !resolved ? (
              <span className="err-text">
                Nenhum PHP instalado.{" "}
                <button className="link" onClick={props.onManage}>
                  Instalar
                </button>
              </span>
            ) : resolved && p.phpVersion && !props.phps.some((v) => v.fullVersion === p.phpVersion) ? (
              <span className="err-text">O PHP {p.phpVersion} não está mais instalado.</span>
            ) : props.locked ? (
              "Pare o projeto para trocar a versão."
            ) : null
          }
        >
          <select
            aria-label="Versão do PHP"
            disabled={props.locked}
            value={props.phps.some((v) => v.fullVersion === p.phpVersion) ? p.phpVersion : ""}
            onChange={(e) =>
              api.setPhpVersion(p.id, e.target.value).then(props.onChanged).catch((err) => props.onError(errMsg(err)))
            }
          >
            <option value="">Mais recente</option>
            {props.phps.map((v) => (
              <option key={v.fpm} value={v.fullVersion}>
                PHP {v.fullVersion}
                {v.managed ? "" : " (sistema)"}
              </option>
            ))}
          </select>
        </Tile>

        <Tile
          icon="database"
          label={dbName}
          value={resolvedDb ? resolvedDb.fullVersion : "—"}
          sub={`porta ${p.dbPort}`}
          hint={
            !resolvedDb ? (
              <span className="err-text">
                Nenhum {dbName} instalado.{" "}
                <button className="link" onClick={props.onManage}>
                  Instalar
                </button>
              </span>
            ) : p.dbInitialized ? (
              "Banco já criado: o tipo e a versão não podem mais ser trocados."
            ) : props.locked ? (
              "Pare o projeto para trocar o banco."
            ) : null
          }
        >
          <select
            aria-label="Tipo de banco de dados"
            disabled={dbLocked}
            value={p.dbEngine}
            onChange={(e) => changeDb(e.target.value as DbEngine, "")}
          >
            <option value="mysql">MySQL</option>
            <option value="postgres">PostgreSQL</option>
          </select>
          <select
            aria-label={`Versão do ${dbName}`}
            disabled={dbLocked}
            value={dbInstalls.some((m) => m.version === p.dbVersion) ? p.dbVersion : ""}
            onChange={(e) => changeDb(p.dbEngine, e.target.value)}
          >
            <option value="">Mais recente</option>
            {dbInstalls.map((m) => (
              <option key={m.key} value={m.version}>
                {dbName} {m.version}
                {m.managed ? "" : " (sistema)"}
              </option>
            ))}
          </select>
        </Tile>

        <Tile
          icon="globe"
          label="Servidor web"
          value={`:${p.httpPort}`}
          sub={`Caddy · ${p.https ? `HTTPS :${p.httpsPort} · ` : "HTTP · "}${p.lan ? "rede local" : "só este computador"}`}
        />
      </div>

      <div className="cols">
        <section className="panel">
          <h3>
            <Icon name="database" /> Conexão com o banco
          </h3>
          <dl className="kv">
            <dt>Host</dt>
            <dd><Copyable value="127.0.0.1" label="host" /></dd>
            <dt>Porta</dt>
            <dd><Copyable value={String(p.dbPort)} label="porta" /></dd>
            <dt>Banco</dt>
            <dd><Copyable value={p.dbName} label="nome do banco" /></dd>
            <dt>Usuário</dt>
            <dd><Copyable value={p.dbUser} label="usuário" /></dd>
            <dt>Senha</dt>
            <dd><Copyable value={p.dbPassword} label="senha" /></dd>
          </dl>
          <p className="hint">
            No PHP: <code>DB_HOST</code> <code>DB_PORT</code> <code>DB_NAME</code> <code>DB_USER</code>{" "}
            <code>DB_PASSWORD</code>, mais <code>DB_ENGINE</code> e <code>DB_CONNECTION</code> ({isPg ? "pgsql" : "mysql"}).
            {isPg && " Use pdo_pgsql ou pgsql; no terminal do projeto, o psql já conecta."}
          </p>
        </section>

        <section className="panel">
          <h3>
            <Icon name="server" /> Ferramentas
          </h3>
          {props.extras
            .filter((x) => x.id === "adminneo" || x.id === "mailpit")
            .map((x) => {
              const available = x.id === "adminneo" ? p.hasAdminneo : p.hasMailpit;
              return (
                <div className="release" key={x.id}>
                  <span className="release-icon">
                    <Icon name={x.id === "adminneo" ? "database" : "mail"} />
                  </span>
                  <div className="grow">
                    <strong>{x.label}</strong>
                    <div className="muted small">{x.description}</div>
                  </div>
                  {!x.installed ? (
                    <button className="btn small" onClick={props.onManage}>
                      <Icon name="download" size={13} /> Instalar
                    </button>
                  ) : available ? (
                    <button
                      className="btn small"
                      onClick={() => api.openTool(p.id, x.id as ToolId).catch((e) => props.onError(errMsg(e)))}
                    >
                      Abrir <Icon name="external" size={13} />
                    </button>
                  ) : (
                    <span className="tag nowrap">{p.running ? "reinicie o projeto" : "inicie o projeto"}</span>
                  )}
                </div>
              );
            })}
          <p className="hint">
            E-mails do PHP (<code>mail()</code> ou SMTP em <code>MAIL_HOST</code>:<code>MAIL_PORT</code>) caem no Mailpit
            do projeto.
          </p>
        </section>
      </div>

      <div className="cols">
        <AccessPanel project={p} onChanged={props.onChanged} onError={props.onError} />
      </div>

      <div className="cols">
        <RedisPanel
          project={p}
          valkeys={props.valkeys}
          onManage={props.onManage}
          onChanged={props.onChanged}
          onError={props.onError}
        />
      </div>

      <div className="cols">
        <WordPressPanel
          project={p}
          extras={props.extras}
          progress={props.progress[`wordpress-${p.id}`]}
          onChanged={props.onChanged}
          onError={props.onError}
        />
      </div>

      <BackupPanel
        project={p}
        backupsDir={props.backupsDir}
        progress={props.progress}
        onChanged={props.onChanged}
        onError={props.onError}
      />

      <SaveBlueprintPanel
        project={p}
        progress={props.progress}
        onOpenBlueprints={props.onBlueprints}
        onError={props.onError}
      />

      <section className="panel folder-panel">
        <div className="grow">
          <h3>
            <Icon name="folder" /> Pasta do projeto
          </h3>
          <Copyable value={p.path} label="caminho" />
        </div>
        <button className="btn" onClick={() => api.openFolder(p.id).catch((e) => props.onError(errMsg(e)))}>
          <Icon name="folder" /> Abrir pasta
        </button>
      </section>

      <section className="danger-zone">
        <div className="grow">
          <strong>Apagar projeto</strong>
          <div className="muted small">
            Remove os arquivos e o banco de dados deste projeto.{p.running ? " Ele será parado antes." : ""}
          </div>
        </div>
        <button className="btn danger" onClick={del}>
          <Icon name="trash" /> Apagar
        </button>
      </section>
    </div>
  );
}

function Logs({ id }: { id: string }) {
  const [file, setFile] = useState<string>(LOG_FILES[0]);
  const [text, setText] = useState("");

  useEffect(() => {
    let alive = true;
    const load = () => api.readLog(id, file).then((t) => alive && setText(t)).catch(() => {});
    load();
    const t = setInterval(load, 1500);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [id, file]);

  return (
    <div className="panel logs">
      <div className="log-files" role="tablist" aria-label="Arquivos de log">
        {LOG_FILES.map((f) => (
          <button key={f} role="tab" aria-selected={f === file} className={`chip-btn ${f === file ? "active" : ""}`} onClick={() => setFile(f)}>
            {f}
          </button>
        ))}
      </div>
      <pre aria-live="polite">{text || "(vazio)"}</pre>
    </div>
  );
}
