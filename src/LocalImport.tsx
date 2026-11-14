import { useEffect, useState } from "react";
import { InstallProgress, LocalImportCheck, LocalSite, VersionNeed, api } from "./api";
import { ProgressView, isBusy } from "./PhpManager";
import { Icon, Skeleton, Spinner } from "./ui";

function errMsg(e: unknown) {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}

const series = (v: string) => v.split(".").slice(0, 2).join(".");

/** "12/11 12:39" a partir de segundos Unix. */
function shortDate(secs: number) {
  const d = new Date(secs * 1000);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getDate())}/${p(d.getMonth() + 1)} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

type Choice = "install" | "use";
type Phase = "checking" | "choose" | "installing" | "importing";

/** Escolha para uma versão que não está instalada aqui. */
function NeedRow(props: {
  label: string;
  need: VersionNeed;
  choice: Choice | undefined;
  onChoose: (c: Choice) => void;
  olderWarning?: boolean;
  disabled: boolean;
}) {
  const { label, need, choice } = props;
  return (
    <div className="field">
      <p className="small">
        {need.wanted
          ? `O site usa ${label} ${need.wanted}, que não está instalado aqui.`
          : `O Local não informa a versão do ${label} deste site.`}
      </p>
      <div className="row">
        {need.installVersion && (
          <button
            type="button"
            className={`btn small ${choice === "install" ? "primary" : ""}`}
            disabled={props.disabled}
            onClick={() => props.onChoose("install")}
          >
            Instalar o {label} {need.wanted}
          </button>
        )}
        {need.fallback && (
          <button
            type="button"
            className={`btn small ${choice === "use" ? "primary" : ""}`}
            disabled={props.disabled}
            onClick={() => props.onChoose("use")}
          >
            Usar o {label} {need.fallback}
          </button>
        )}
      </div>
      {!need.installVersion && !need.fallback && (
        <p className="small err-text" role="alert">
          {need.wanted
            ? `Não há ${label} ${need.wanted} para instalar e nenhuma outra versão instalada. Instale uma em PHP e ferramentas.`
            : `Nenhum ${label} instalado. Instale um em PHP e ferramentas.`}
        </p>
      )}
      {props.olderWarning && need.fallbackOlder && choice === "use" && (
        <p className="small warn-text" role="alert">
          O banco pode não importar numa versão mais antiga.
        </p>
      )}
    </div>
  );
}

/** Tela para trazer um site do Local (by Flywheel) como projeto novo. */
export default function LocalImport(props: {
  progress: Record<string, InstallProgress>;
  /** Já há um projeto sendo criado (importação em andamento). */
  importing: boolean;
  onImport: (id: string, name: string, phpSeries: string, mysqlSeries: string) => Promise<void>;
  onInstalled: () => void;
  onBack: () => void;
  onError: (m: string) => void;
}) {
  const [sites, setSites] = useState<LocalSite[] | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [picked, setPicked] = useState<LocalSite | null>(null);
  const [name, setName] = useState("");
  const [phase, setPhase] = useState<Phase | null>(null);
  const [check, setCheck] = useState<LocalImportCheck | null>(null);
  const [phpChoice, setPhpChoice] = useState<Choice>();
  const [mysqlChoice, setMysqlChoice] = useState<Choice>();
  const [installIds, setInstallIds] = useState<string[]>([]);

  useEffect(() => {
    api
      .listLocalSites()
      .then(setSites)
      .catch((e) => {
        setListError(errMsg(e));
        setSites([]);
      });
  }, []);

  const working = phase !== null;

  function pick(s: LocalSite) {
    setPicked(s);
    setName(s.name);
    setCheck(null);
    setPhpChoice(undefined);
    setMysqlChoice(undefined);
    setPhase(null);
  }

  function cancel() {
    setPicked(null);
    setCheck(null);
    setPhpChoice(undefined);
    setMysqlChoice(undefined);
    setPhase(null);
  }

  async function finish(site: LocalSite, php: string, mysql: string) {
    setPhase("importing");
    try {
      await props.onImport(site.id, name.trim(), php, mysql);
    } finally {
      setPhase(null);
    }
  }

  async function start(e: React.FormEvent) {
    e.preventDefault();
    if (!picked || !name.trim()) return;
    setPhase("checking");
    try {
      const c = await api.checkLocalSite(picked.id);
      if (c.php.installed && c.mysql.installed) {
        await finish(picked, c.php.wanted, c.mysql.wanted);
        return;
      }
      setCheck(c);
      setPhpChoice(undefined);
      setMysqlChoice(undefined);
      setPhase("choose");
    } catch (err) {
      props.onError(errMsg(err));
      setPhase(null);
    }
  }

  async function confirmChoices() {
    if (!picked || !check) return;
    setPhase("installing");
    try {
      const ids: string[] = [];
      if (!check.php.installed && phpChoice === "install" && check.php.installVersion) ids.push(check.php.installVersion);
      if (!check.mysql.installed && mysqlChoice === "install" && check.mysql.installVersion) ids.push(`mysql-${check.mysql.installVersion}`);
      setInstallIds(ids);
      if (!check.php.installed && phpChoice === "install" && check.php.installVersion) {
        await api.installPhp(check.php.installVersion);
      }
      if (!check.mysql.installed && mysqlChoice === "install" && check.mysql.installVersion) {
        await api.installMysql(check.mysql.installVersion);
      }
      props.onInstalled();
      const php = check.php.installed || phpChoice === "install" ? check.php.wanted : (check.php.fallback ?? check.php.wanted);
      const mysql = check.mysql.installed || mysqlChoice === "install" ? check.mysql.wanted : (check.mysql.fallback ?? check.mysql.wanted);
      await finish(picked, php, mysql);
    } catch (err) {
      props.onError(errMsg(err));
      setPhase("choose");
    }
  }

  const phpDone = !check || check.php.installed || !!phpChoice;
  const mysqlDone = !check || check.mysql.installed || !!mysqlChoice;
  const installProgress = installIds.map((id) => props.progress[id]).filter((p) => isBusy(p));

  return (
    <div className="project">
      <header className="project-head">
        <div className="title">
          <h1>Importar do Local</h1>
          <div className="meta">
            Traz um site do Local (by Flywheel) como projeto novo, com os arquivos e o banco de dados. O Local não é alterado.{" "}
            <button type="button" className="link" onClick={props.onBack}>
              Voltar
            </button>
          </div>
        </div>
      </header>

      {sites === null ? (
        <Skeleton kind="card" count={2} />
      ) : listError ? (
        <section className="panel empty-state" role="alert">
          <Icon name="alert" size={28} />
          <p className="muted small">{listError}</p>
        </section>
      ) : sites.length === 0 ? (
        <section className="panel empty-state">
          <Icon name="download" size={28} />
          <p className="muted small">O Local não tem sites.</p>
        </section>
      ) : (
        <ul className="backup-list blueprint-list" aria-label="Sites do Local">
          {sites.map((s) => (
            <li key={s.id} className="blueprint-item">
              <div className="blueprint-row">
                <span className="release-icon">
                  <Icon name="download" />
                </span>
                <div className="grow">
                  <strong className="ellipsis">{s.name}</strong>
                  <div className="muted small mono">
                    PHP {series(s.phpVersion)}
                    {s.mysqlVersion ? ` · MySQL ${series(s.mysqlVersion)}` : ""}
                  </div>
                  <div className="muted small">
                    {s.dumpSavedAt > 0 ? `banco salvo em ${shortDate(s.dumpSavedAt)}` : "banco ainda não salvo pelo Local"}
                  </div>
                  {s.warnings.map((w) => (
                    <div key={w} className="muted small">
                      {w}
                    </div>
                  ))}
                  {s.blocker && (
                    <div className="small warn-text" role="alert">
                      {s.blocker}
                    </div>
                  )}
                </div>
                <button
                  className="btn small primary"
                  disabled={!!s.blocker || working || props.importing}
                  onClick={() => pick(s)}
                >
                  <Icon name="download" size={13} /> Importar
                </button>
              </div>
              {picked?.id === s.id && (
                <form className="blueprint-use" onSubmit={start}>
                  <label className="field-label" htmlFor="li-name">Nome no LocalTusk</label>
                  <div className="row">
                    <input
                      id="li-name"
                      autoFocus
                      className="grow"
                      value={name}
                      disabled={working}
                      onChange={(e) => setName(e.target.value)}
                    />
                    {(phase === null || phase === "checking") && (
                      <button className="btn small primary" disabled={working || !name.trim()}>
                        {phase === "checking" ? (
                          <>
                            <Spinner size={13} /> Verificando…
                          </>
                        ) : (
                          "Importar"
                        )}
                      </button>
                    )}
                    {(phase === null || phase === "choose") && (
                      <button type="button" className="btn small ghost" onClick={cancel}>
                        Cancelar
                      </button>
                    )}
                  </div>

                  {check && (phase === "choose" || phase === "installing" || phase === "importing") && (
                    <>
                      {!check.php.installed && (
                        <NeedRow
                          label="PHP"
                          need={check.php}
                          choice={phpChoice}
                          onChoose={setPhpChoice}
                          disabled={phase !== "choose"}
                        />
                      )}
                      {!check.mysql.installed && (
                        <NeedRow
                          label="MySQL"
                          need={check.mysql}
                          choice={mysqlChoice}
                          onChoose={setMysqlChoice}
                          olderWarning
                          disabled={phase !== "choose"}
                        />
                      )}
                      {phase === "choose" && (
                        <div className="row">
                          <button
                            type="button"
                            className="btn small primary"
                            disabled={!phpDone || !mysqlDone || !name.trim()}
                            onClick={confirmChoices}
                          >
                            Continuar
                          </button>
                        </div>
                      )}
                    </>
                  )}
                  {phase === "installing" && (
                    <div role="status">
                      <p className="small row nowrap">
                        <Spinner size={13} /> Instalando as versões escolhidas…
                      </p>
                      {installProgress.map((p) => (
                        <ProgressView key={p.id} p={p} />
                      ))}
                    </div>
                  )}
                </form>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
