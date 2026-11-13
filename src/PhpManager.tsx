import { useCallback, useEffect, useMemo, useState } from "react";
import { askConfirm } from "./confirm";
import {
  AppInfo,
  ExtraStatus,
  InstallProgress,
  MysqlInstall,
  MysqlRelease,
  PhpInstall,
  PhpRelease,
  PostgresInstall,
  PostgresRelease,
  ValkeyInstall,
  ValkeyRelease,
  api,
  formatBytes,
} from "./api";
import { Copyable, Icon, IconName } from "./ui";

function errMsg(e: unknown) {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}

const EXTRA_ICONS: Record<string, IconName> = {
  caddy: "globe",
  adminneo: "database",
  mailpit: "mail",
  wpcli: "terminal",
};

export const isBusy = (p?: InstallProgress) =>
  !!p && (p.stage === "download" || p.stage === "extract" || p.stage === "verify");

export default function PhpManager(props: {
  phps: PhpInstall[];
  mysqls: MysqlInstall[];
  postgreses: PostgresInstall[];
  valkeys: ValkeyInstall[];
  extras: ExtraStatus[];
  progress: Record<string, InstallProgress>;
  onChanged: () => void;
  onError: (m: string) => void;
}) {
  const [phpCatalog, setPhpCatalog] = useState<PhpRelease[] | null>(null);
  const [phpError, setPhpError] = useState<string | null>(null);
  const [myCatalog, setMyCatalog] = useState<MysqlRelease[] | null>(null);
  const [myError, setMyError] = useState<string | null>(null);
  const [pgCatalog, setPgCatalog] = useState<PostgresRelease[] | null>(null);
  const [pgError, setPgError] = useState<string | null>(null);
  const [vkCatalog, setVkCatalog] = useState<ValkeyRelease[] | null>(null);
  const [vkError, setVkError] = useState<string | null>(null);
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [tab, setTab] = useState<"tools" | "php" | "mysql" | "postgres" | "redis">("tools");

  const loadPhp = useCallback(() => {
    setPhpError(null);
    setPhpCatalog(null);
    api.phpCatalog().then(setPhpCatalog).catch((e) => setPhpError(errMsg(e)));
  }, []);

  const loadMysql = useCallback(() => {
    setMyError(null);
    setMyCatalog(null);
    api.mysqlCatalog().then(setMyCatalog).catch((e) => setMyError(errMsg(e)));
  }, []);

  const loadPostgres = useCallback(() => {
    setPgError(null);
    setPgCatalog(null);
    api.postgresCatalog().then(setPgCatalog).catch((e) => setPgError(errMsg(e)));
  }, []);

  const loadValkey = useCallback(() => {
    setVkError(null);
    setVkCatalog(null);
    api.valkeyCatalog().then(setVkCatalog).catch((e) => setVkError(errMsg(e)));
  }, []);

  useEffect(() => {
    loadPhp();
    loadMysql();
    loadPostgres();
    loadValkey();
    api.appInfo().then(setInfo);
  }, [loadPhp, loadMysql, loadPostgres, loadValkey]);

  // Quando uma instalação termina, atualiza os catálogos (marca como instalada).
  const finished = Object.values(props.progress).filter((p) => p.stage === "done").length;
  useEffect(() => {
    if (finished > 0) {
      api.phpCatalog().then(setPhpCatalog).catch(() => {});
      api.mysqlCatalog().then(setMyCatalog).catch(() => {});
      api.postgresCatalog().then(setPgCatalog).catch(() => {});
      api.valkeyCatalog().then(setVkCatalog).catch(() => {});
    }
  }, [finished]);

  async function run(job: () => Promise<void>) {
    try {
      await job();
    } catch (e) {
      props.onError(errMsg(e));
    } finally {
      props.onChanged();
      api.phpCatalog().then(setPhpCatalog).catch(() => {});
      api.mysqlCatalog().then(setMyCatalog).catch(() => {});
      api.postgresCatalog().then(setPgCatalog).catch(() => {});
      api.valkeyCatalog().then(setVkCatalog).catch(() => {});
    }
  }

  const essentials = props.extras.filter((x) => x.essential);
  const optionals = props.extras.filter((x) => !x.essential);
  const installedTools = props.extras.filter((x) => x.installed).length;

  return (
    <div className="project">
      <header className="project-head">
        <div className="title">
          <h1>PHP e ferramentas</h1>
          <div className="meta">
            Tudo é baixado para:
            <Copyable value={info?.dataDir ?? "…"} label="pasta de dados" />
          </div>
        </div>
        <div className="actions">
          <button className="btn" onClick={() => api.openDataDir().catch((e) => props.onError(errMsg(e)))}>
            <Icon name="folder" /> Abrir pasta
          </button>
        </div>
      </header>

      <nav className="tabs" role="tablist">
        {(
          [
            ["tools", "Ferramentas", `${installedTools}/${props.extras.length}`],
            ["php", "PHP", String(props.phps.length)],
            ["mysql", "MySQL", String(props.mysqls.length)],
            ["postgres", "PostgreSQL", String(props.postgreses.length)],
            ["redis", "Redis", String(props.valkeys.length)],
          ] as const
        ).map(([id, label, count]) => (
          <button key={id} role="tab" aria-selected={tab === id} className={tab === id ? "active" : ""} onClick={() => setTab(id)}>
            {label} <span className="count">{count}</span>
          </button>
        ))}
      </nav>

      {tab === "tools" && (
        <>
          <section className="panel">
            <h3><Icon name="globe" /> Servidor web</h3>
            {essentials.map((x) => (
              <ExtraRow key={x.id} extra={x} progress={props.progress[x.id]} onChanged={props.onChanged} onError={props.onError} />
            ))}
            {essentials.some((x) => !x.installed) && <p className="hint">Necessário para iniciar qualquer projeto.</p>}
          </section>

          <section className="panel">
            <h3><Icon name="layers" /> Ferramentas opcionais</h3>
            {optionals.map((x) => (
              <ExtraRow key={x.id} extra={x} progress={props.progress[x.id]} onChanged={props.onChanged} onError={props.onError} />
            ))}
            <p className="hint">Depois de instalar, reinicie os projetos que já estavam rodando para ativar a ferramenta.</p>
          </section>
        </>
      )}

      {tab === "php" && (
        <div className="cols">
          <section className="panel">
            <h3><Icon name="code" /> Instaladas <span className="count">{props.phps.length}</span></h3>
            {props.phps.length === 0 && (
              <p className="muted">Nenhuma versão instalada ainda. Escolha uma ao lado — recomendamos a mais recente.</p>
            )}
            {props.phps.map((p) => (
              <InstalledRow
                key={p.fpm}
                title={`PHP ${p.fullVersion}`}
                managed={p.managed}
                onRemove={async () => {
                  const ok = await askConfirm({
                    title: `Remover o PHP ${p.fullVersion}?`,
                    message: "Você pode instalá-lo de novo quando quiser.",
                    confirmLabel: "Remover",
                    danger: true,
                  });
                  if (ok) run(() => api.uninstallPhp(p.fullVersion));
                }}
              />
            ))}
          </section>

          <section className="panel">
            <h3><Icon name="download" /> Disponíveis</h3>
            <Catalog
              releases={phpCatalog}
              error={phpError}
              onRetry={loadPhp}
              groupOf={(r) => (r as PhpRelease).minor}
              title={(g) => `PHP ${g}`}
              rowLabel={(r) => `PHP ${r.version}`}
              progressId={(r) => r.version}
              progress={props.progress}
              onInstall={(v) => run(() => api.installPhp(v))}
            />
            {phpCatalog && (
              <p className="hint">
                Binários do projeto static-php-cli (PHP 8.0 em diante), com mysqli, pdo_mysql, intl, gd, imagick,
                opcache, curl, sodium e outras extensões. Versões 7.x e anteriores não têm binários disponíveis.
              </p>
            )}
          </section>
        </div>
      )}

      {tab === "mysql" && (
        <div className="cols">
          <section className="panel">
            <h3><Icon name="database" /> Instaladas <span className="count">{props.mysqls.length}</span></h3>
            {props.mysqls.length === 0 && (
              <p className="muted">Nenhuma versão instalada ainda. Para começar, recomendamos o MySQL 8.4 (LTS).</p>
            )}
            {props.mysqls.map((m) => (
              <InstalledRow
                key={m.mysqld}
                title={`MySQL ${m.fullVersion}`}
                managed={m.managed}
                onRemove={async () => {
                  const ok = await askConfirm({
                    title: `Remover o MySQL ${m.fullVersion}?`,
                    message: "Os bancos dos projetos não são apagados.",
                    confirmLabel: "Remover",
                    danger: true,
                  });
                  if (ok) run(() => api.uninstallMysql(m.fullVersion));
                }}
              />
            ))}
          </section>

          <section className="panel">
            <h3><Icon name="download" /> Disponíveis</h3>
            <Catalog
              releases={myCatalog}
              error={myError}
              onRetry={loadMysql}
              groupOf={(r) => (r as MysqlRelease).series}
              title={(g) => `MySQL ${g}`}
              rowLabel={(r) => `MySQL ${r.version}`}
              progressId={(r) => `mysql-${r.version}`}
              progress={props.progress}
              onInstall={(v) => run(() => api.installMysql(v))}
            />
            {myCatalog && (
              <p className="hint">
                Binários oficiais da Oracle (Community Server). O banco de um projeto é criado na primeira
                inicialização e depois não pode trocar de versão — para usar outra, crie um novo projeto.
              </p>
            )}
          </section>
        </div>
      )}

      {tab === "postgres" && (
        <div className="cols">
          <section className="panel">
            <h3><Icon name="database" /> Instaladas <span className="count">{props.postgreses.length}</span></h3>
            {props.postgreses.length === 0 && (
              <p className="muted">Nenhuma versão instalada ainda. Para começar, recomendamos o PostgreSQL 17 ou 16.</p>
            )}
            {props.postgreses.map((m) => (
              <InstalledRow
                key={m.postgres}
                title={`PostgreSQL ${m.fullVersion}`}
                managed
                onRemove={async () => {
                  const ok = await askConfirm({
                    title: `Remover o PostgreSQL ${m.fullVersion}?`,
                    message: "Os bancos dos projetos não são apagados.",
                    confirmLabel: "Remover",
                    danger: true,
                  });
                  if (ok) run(() => api.uninstallPostgres(m.fullVersion));
                }}
              />
            ))}
          </section>

          <section className="panel">
            <h3><Icon name="download" /> Disponíveis</h3>
            <Catalog
              releases={pgCatalog}
              error={pgError}
              onRetry={loadPostgres}
              groupOf={(r) => (r as PostgresRelease).major}
              title={(g) => `PostgreSQL ${g}`}
              rowLabel={(r) => `PostgreSQL ${r.version}`}
              progressId={(r) => `postgres-${r.version}`}
              progress={props.progress}
              onInstall={(v) => run(() => api.installPostgres(v))}
            />
            {pgCatalog && (
              <p className="hint">
                Binários do projeto theseus-rs/postgresql-binaries. Escolha o PostgreSQL ao criar o projeto: as variáveis{" "}
                <code>DB_*</code> e o Adminer Neo funcionam como no MySQL. Na primeira vez, o app também baixa o PHP com{" "}
                <code>pdo_pgsql</code>. O banco não troca de tipo nem de versão depois de criado.
              </p>
            )}
          </section>
        </div>
      )}

      {tab === "redis" && (
        <div className="cols">
          <section className="panel">
            <h3><Icon name="database" /> Instalado <span className="count">{props.valkeys.length}</span></h3>
            {props.valkeys.length === 0 && (
              <p className="muted">Ainda não instalado. Depois, ligue o Redis em cada projeto que precisar de cache ou filas.</p>
            )}
            {props.valkeys.map((m) => (
              <InstalledRow
                key={m.server}
                title={`Valkey ${m.fullVersion}`}
                managed
                onRemove={async () => {
                  const ok = await askConfirm({
                    title: `Remover o Valkey ${m.fullVersion}?`,
                    message: "Os dados salvos dos projetos não são apagados.",
                    confirmLabel: "Remover",
                    danger: true,
                  });
                  if (ok) run(() => api.uninstallValkey(m.fullVersion));
                }}
              />
            ))}
          </section>

          <section className="panel">
            <h3><Icon name="download" /> Disponível</h3>
            <Catalog
              releases={vkCatalog}
              error={vkError}
              onRetry={loadValkey}
              groupOf={() => "Redis"}
              title={() => "Redis (Valkey)"}
              rowLabel={(r) => `Valkey ${r.version}`}
              progressId={(r) => `valkey-${r.version}`}
              progress={props.progress}
              onInstall={(v) => run(() => api.installValkey(v))}
            />
            {vkCatalog &&
              (navigator.userAgent.includes("Windows") ? (
                <p className="hint">
                  No Windows o app usa o Redis do projeto comunitário <strong>redis-windows</strong> (build do Redis
                  oficial), porque o Valkey não tem versão para Windows. Ele fala o mesmo protocolo (<code>redis-cli</code>,
                  phpredis, Predis) e escuta só em 127.0.0.1.
                </p>
              ) : (
                <p className="hint">
                  O Valkey é o sucessor livre do Redis e fala o mesmo protocolo (<code>redis-cli</code>, phpredis, Predis).
                  O app baixa o código-fonte e o compila na sua máquina (cerca de 15 segundos); isso pode exigir as
                  ferramentas de compilação do sistema (no macOS, <code>xcode-select --install</code>; no Linux,{" "}
                  <code>build-essential</code>). O Redis escuta só em 127.0.0.1.
                </p>
              ))}
          </section>
        </div>
      )}
    </div>
  );
}

function InstalledRow(props: { title: string; managed: boolean; onRemove: () => void }) {
  return (
    <div className="release">
      <div className="grow">
        <strong className="mono-strong">{props.title}</strong>{" "}
        <span className={`tag ${props.managed ? "managed" : ""}`}>
          {props.managed ? "LocalTusk" : "sistema (Homebrew)"}
        </span>
      </div>
      {props.managed ? (
        <button className="btn small danger" onClick={props.onRemove}>
          <Icon name="trash" size={13} /> Remover
        </button>
      ) : (
        <span className="muted small">gerenciado fora do app</span>
      )}
    </div>
  );
}

interface Release {
  version: string;
  size: string;
  installed: boolean;
}

/** Lista de versões agrupadas por série (8.4, 8.3…), com a mais nova em destaque. */
function Catalog(props: {
  releases: Release[] | null;
  error: string | null;
  onRetry: () => void;
  groupOf: (r: Release) => string;
  title: (group: string) => string;
  rowLabel: (r: Release) => string;
  progressId: (r: Release) => string;
  progress: Record<string, InstallProgress>;
  onInstall: (version: string) => void;
}) {
  const [expanded, setExpanded] = useState<string | null>(null);
  const groups = useMemo(() => {
    const map = new Map<string, Release[]>();
    for (const r of props.releases ?? []) {
      const g = props.groupOf(r);
      map.set(g, [...(map.get(g) ?? []), r]);
    }
    return [...map.entries()];
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.releases]);

  return (
    <>
      {props.error && (
        <div className="banner err">
          {props.error}{" "}
          <button className="btn small" onClick={props.onRetry}>
            Tentar de novo
          </button>
        </div>
      )}
      {!props.releases && !props.error && <p className="muted">Consultando versões…</p>}
      {groups.map(([group, releases]) => {
        const latest = releases[0];
        const open = expanded === group;
        return (
          <div key={group} className="series">
            <ReleaseRow
              label={props.title(group)}
              release={latest}
              progress={props.progress[props.progressId(latest)]}
              onInstall={props.onInstall}
              suffix={`último: ${latest.version}`}
            />
            {releases.length > 1 && (
              <button className="link more" onClick={() => setExpanded(open ? null : group)} aria-expanded={open}>
                <Icon name={open ? "chevronDown" : "chevronRight"} size={13} /> outras versões {group}.x ({releases.length - 1})
              </button>
            )}
            {open && (
              <div className="patches">
                {releases.slice(1).map((r) => (
                  <ReleaseRow
                    key={r.version}
                    label={props.rowLabel(r)}
                    release={r}
                    progress={props.progress[props.progressId(r)]}
                    onInstall={props.onInstall}
                  />
                ))}
              </div>
            )}
          </div>
        );
      })}
    </>
  );
}

export function ProgressView({ p }: { p: InstallProgress }) {
  const pct = p.total > 0 ? Math.min(100, Math.round((p.downloaded / p.total) * 100)) : 0;
  return (
    <div className="progress-wrap">
      <div className="progress">
        <div
          className={`bar ${p.stage !== "download" ? "indeterminate" : ""}`}
          style={{ width: p.stage === "download" ? `${pct}%` : "100%" }}
        />
      </div>
      <span className="muted small">
        {p.stage === "download"
          ? `Baixando ${formatBytes(p.downloaded)}${p.total ? ` / ${formatBytes(p.total)}` : ""}`
          : (p.message ?? (p.stage === "extract" ? "Extraindo…" : "Verificando…"))}
      </span>
    </div>
  );
}

function ReleaseRow(props: {
  label: string;
  release: Release;
  progress?: InstallProgress;
  onInstall: (v: string) => void;
  suffix?: string;
}) {
  const { release: r, progress: p } = props;
  const busy = isBusy(p);

  return (
    <div className="release">
      <div className="grow">
        <strong>{props.label}</strong>{" "}
        {props.suffix && <span className="muted small">{props.suffix}</span>}{" "}
        <span className="muted small">{r.size && `· ${/\d[KMG]$/.test(r.size) ? `${r.size}B` : r.size}`}</span>
        {busy && p && <ProgressView p={p} />}
        {p?.stage === "error" && <div className="small err-text">{p.message}</div>}
      </div>
      {r.installed ? (
        <span className="tag managed"><Icon name="check" size={12} /> instalada</span>
      ) : (
        <button className="btn small primary" disabled={busy} onClick={() => props.onInstall(r.version)}>
          <Icon name="download" size={13} /> {busy ? "Instalando…" : "Instalar"}
        </button>
      )}
    </div>
  );
}

function ExtraRow(props: {
  extra: ExtraStatus;
  progress?: InstallProgress;
  onChanged: () => void;
  onError: (m: string) => void;
}) {
  const { extra: x, progress: p } = props;
  const busy = isBusy(p);

  async function install() {
    try {
      await api.installExtra(x.id);
    } catch (e) {
      props.onError(errMsg(e));
    } finally {
      props.onChanged();
    }
  }

  async function remove() {
    const ok = await askConfirm({
      title: `Remover ${x.label}?`,
      message: "Você pode instalar de novo quando quiser.",
      confirmLabel: "Remover",
      danger: true,
    });
    if (!ok) return;
    try {
      await api.uninstallExtra(x.id);
    } catch (e) {
      props.onError(errMsg(e));
    } finally {
      props.onChanged();
    }
  }

  return (
    <div className="release">
      <span className="release-icon">
        <Icon name={EXTRA_ICONS[x.id] ?? "layers"} />
      </span>
      <div className="grow">
        <strong>{x.label}</strong>{" "}
        {x.installed && x.version && <span className="tag managed">v{x.version}</span>}
        <div className="muted small">{x.description}</div>
        {busy && p && <ProgressView p={p} />}
        {p?.stage === "error" && <div className="small err-text">{p.message}</div>}
      </div>
      {x.installed ? (
        <button className="btn small danger" onClick={remove}>
          <Icon name="trash" size={13} /> Remover
        </button>
      ) : (
        <button className="btn small primary" disabled={busy} onClick={install}>
          <Icon name="download" size={13} /> {busy ? "Instalando…" : "Instalar"}
        </button>
      )}
    </div>
  );
}
