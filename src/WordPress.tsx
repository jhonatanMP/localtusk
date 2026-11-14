import { useEffect, useState } from "react";
import { ExtraStatus, InstallProgress, Project, api } from "./api";
import { ProgressView, isBusy } from "./PhpManager";
import { Copyable, Icon, Switch } from "./ui";

function errMsg(e: unknown) {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}

const LOCALES: [string, string][] = [
  ["pt_BR", "Português do Brasil"],
  ["en_US", "English (US)"],
  ["es_ES", "Español"],
  ["fr_FR", "Français"],
  ["de_DE", "Deutsch"],
  ["it_IT", "Italiano"],
];

function randomPassword(length = 16) {
  const chars = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
  const bytes = new Uint32Array(length);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => chars[b % chars.length]).join("");
}

interface Credentials {
  user: string;
  password: string;
}

/** Instalação do WordPress (via WP-CLI) e terminal do projeto. */
export default function WordPressPanel(props: {
  project: Project;
  extras: ExtraStatus[];
  progress?: InstallProgress;
  onChanged: () => void;
  onError: (m: string) => void;
}) {
  const { project: p, progress } = props;
  const wpcli = props.extras.find((x) => x.id === "wpcli");
  const [open, setOpen] = useState(false);
  const [title, setTitle] = useState(p.name);
  const [user, setUser] = useState("admin");
  const [password, setPassword] = useState(randomPassword);
  const [email, setEmail] = useState("admin@example.com");
  const [locale, setLocale] = useState("pt_BR");
  const [confirmWipe, setConfirmWipe] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [created, setCreated] = useState<Credentials | null>(null);
  const [terminalBusy, setTerminalBusy] = useState(false);
  const [admins, setAdmins] = useState<string[]>([]);
  const [loginBusy, setLoginBusy] = useState(false);
  const [adminBusy, setAdminBusy] = useState(false);
  const busy = submitting || isBusy(progress);
  // Se a pasta pública tem arquivos que não são do WordPress, eles serão apagados.
  const willWipe = p.docrootInUse;

  async function install(e: React.FormEvent) {
    e.preventDefault();
    setSubmitting(true);
    try {
      await api.installWordpress(p.id, { title, adminUser: user, adminPassword: password, adminEmail: email, locale, replaceExisting: willWipe && confirmWipe });
      setCreated({ user, password });
      setOpen(false);
    } catch (err) {
      props.onError(errMsg(err));
    } finally {
      setSubmitting(false);
      props.onChanged();
    }
  }

  useEffect(() => {
    setAdmins([]);
    if (!p.running || !p.hasWordpress) return;
    let alive = true;
    api
      .listWpAdmins(p.id)
      .then((list) => alive && setAdmins(list))
      .catch((e) => {
        if (alive) {
          setAdmins([]);
          props.onError(errMsg(e));
        }
      });
    return () => {
      alive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [p.id, p.running, p.hasWordpress]);

  // Parado: só o usuário salvo. Rodando: a lista de administradores (mais o salvo, se saiu dela).
  const options = p.running ? (p.autoLoginUser && !admins.includes(p.autoLoginUser) ? [p.autoLoginUser, ...admins] : admins) : p.autoLoginUser ? [p.autoLoginUser] : [];
  const canChoose = p.running ? admins.length > 0 : !!p.autoLoginUser;
  const selectedUser = p.autoLoginUser || options[0] || "";

  async function saveAutoLogin(enabled: boolean, who: string) {
    if (enabled && !who) return;
    setLoginBusy(true);
    try {
      await api.setAutoLogin(p.id, enabled, who);
    } catch (err) {
      props.onError(errMsg(err));
    } finally {
      setLoginBusy(false);
      props.onChanged();
    }
  }

  async function openWpAdmin() {
    setAdminBusy(true);
    try {
      const msg = await api.openWpAdmin(p.id);
      if (msg) props.onError(msg);
    } catch (err) {
      props.onError(errMsg(err));
    } finally {
      setAdminBusy(false);
    }
  }

  async function terminal() {
    setTerminalBusy(true);
    try {
      await api.openTerminal(p.id);
    } catch (err) {
      props.onError(errMsg(err));
    } finally {
      setTerminalBusy(false);
    }
  }

  return (
    <>
      {p.dbEngine !== "postgres" && (
      <section className={`panel ${open && !busy && !p.hasWordpress ? "wide" : ""}`}>
        <h3><Icon name="layout" /> WordPress</h3>
        {p.hasWordpress ? (
          <>
            <p className="small">
              <span className="tag managed"><Icon name="check" size={12} /> instalado</span> neste projeto.
            </p>
            {created && (
              <dl className="kv">
                <dt>Usuário</dt>
                <dd><Copyable value={created.user} label="usuário" /></dd>
                <dt>Senha</dt>
                <dd><Copyable value={created.password} label="senha" /></dd>
              </dl>
            )}
            <div className="row">
              <button
                className="btn small primary"
                disabled={!p.running || adminBusy}
                onClick={openWpAdmin}
              >
                <Icon name="external" size={13} /> {adminBusy ? "Abrindo…" : "wp-admin"}
              </button>
              <button
                className="btn small"
                disabled={!p.running}
                onClick={() => api.openSite(p.id).catch((e) => props.onError(errMsg(e)))}
              >
                <Icon name="external" size={13} /> Site
              </button>
            </div>
            <div className="row">
              <label className="check inline">
                <Switch
                  label="Login automático no wp-admin"
                  checked={p.autoLogin}
                  disabled={loginBusy || (!p.autoLogin && !canChoose)}
                  onChange={(next) => saveAutoLogin(next, selectedUser)}
                />
                Login automático no wp-admin
              </label>
              <label className="check inline">
                Usuário
                <select
                  value={selectedUser}
                  disabled={!p.running || !p.autoLogin || loginBusy || options.length === 0}
                  onChange={(e) => saveAutoLogin(true, e.target.value)}
                >
                  {options.map((u) => (
                    <option key={u} value={u}>
                      {u}
                    </option>
                  ))}
                </select>
              </label>
            </div>
            {!p.running && <p className="hint">Inicie o projeto para acessar o WordPress.</p>}
          </>
        ) : busy ? (
          <>
            <p className="small">Instalando o WordPress…</p>
            {progress && <ProgressView p={progress} />}
          </>
        ) : open ? (
          <form className="wp-form" onSubmit={install}>
            <label className="field">
              <span className="field-label">Título do site</span>
              <input value={title} onChange={(e) => setTitle(e.target.value)} />
            </label>
            <div className="field-row">
              <label className="field">
                <span className="field-label">Usuário administrador</span>
                <input value={user} onChange={(e) => setUser(e.target.value)} autoCapitalize="off" />
              </label>
              <label className="field">
                <span className="field-label">E-mail</span>
                <input type="email" value={email} onChange={(e) => setEmail(e.target.value)} />
              </label>
            </div>
            <label className="field">
              <span className="field-label">Senha</span>
              <div className="row nowrap">
                <input className="grow mono-input" value={password} onChange={(e) => setPassword(e.target.value)} />
                <button type="button" className="btn small" onClick={() => setPassword(randomPassword())}>
                  Gerar outra
                </button>
              </div>
            </label>
            <label className="field">
              <span className="field-label">Idioma</span>
              <select value={locale} onChange={(e) => setLocale(e.target.value)}>
                {LOCALES.map(([code, name]) => (
                  <option key={code} value={code}>
                    {name} ({code})
                  </option>
                ))}
              </select>
            </label>
            {willWipe && (
              <div className="banner warn" role="alert">
                <Icon name="alert" />
                <div>
                  <strong>Atenção: a pasta pública não está vazia.</strong>
                  <div>
                    Tudo que está em <code>app/{p.docroot}</code> será <strong>apagado</strong> para instalar o
                    WordPress. Isso não pode ser desfeito.
                  </div>
                  <label className="check">
                    <input type="checkbox" checked={confirmWipe} onChange={(e) => setConfirmWipe(e.target.checked)} />
                    Entendo e quero apagar os arquivos da pasta
                  </label>
                </div>
              </div>
            )}
            <p className="hint">
              O WordPress será baixado para a pasta pública (<code>app/{p.docroot}</code>) e ligado ao banco deste
              projeto.
              {!willWipe && " O index.php de exemplo será substituído."}
              {!p.running && " O projeto será iniciado automaticamente."}
              {!wpcli?.installed && " O WP-CLI será baixado junto."}
            </p>
            <div className="row">
              <button
                className={`btn ${willWipe ? "danger" : "primary"}`}
                disabled={!title.trim() || !user || password.length < 8 || (willWipe && !confirmWipe)}
              >
                <Icon name={willWipe ? "trash" : "download"} size={13} />{" "}
                {willWipe ? "Apagar a pasta e instalar" : "Instalar WordPress"}
              </button>
              <button type="button" className="btn ghost" onClick={() => setOpen(false)}>
                Cancelar
              </button>
            </div>
          </form>
        ) : (
          <>
            <p className="muted small">
              Baixa a versão mais recente, cria o <code>wp-config.php</code> com o banco do projeto e deixa o site
              pronto para usar.
              {p.docrootInUse && " A pasta pública atual será apagada (você confirma antes)."}
            </p>
            <button className="btn small primary" onClick={() => setOpen(true)}>
              <Icon name="download" size={13} /> Instalar WordPress
            </button>
          </>
        )}
        {progress?.stage === "error" && !busy && <div className="small err-text">{progress.message}</div>}
      </section>
      )}

      <section className="panel">
        <h3><Icon name="terminal" /> Linha de comando</h3>
        <p className="muted small">
          Abre um terminal na pasta do projeto onde <code>php</code>,{" "}
          {p.dbEngine === "postgres" ? (
            <>
              <code>psql</code>
            </>
          ) : (
            <>
              <code>wp</code> (WP-CLI) e <code>mysql</code>
            </>
          )}
          {p.redis && <>, <code>redis-cli</code></>}
          {" "}já usam o PHP e o banco deste projeto.
        </p>
        <button className="btn small" disabled={terminalBusy} onClick={terminal}>
          <Icon name="terminal" size={13} /> {terminalBusy ? "Abrindo…" : "Abrir terminal"}
        </button>
        {!wpcli?.installed && (
          <p className="muted small">
            O comando <code>wp</code> precisa do WP-CLI: instale em “PHP e ferramentas”.
          </p>
        )}
      </section>
    </>
  );
}
