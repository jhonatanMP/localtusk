import { useCallback, useEffect, useState } from "react";
import { version as APP_VERSION } from "../package.json";
import { UpdateInfo, api } from "./api";
import { Icon, Spinner, Switch } from "./ui";

const CHECK_KEY = "localtusk.update.check";
const DISMISS_KEY = "localtusk.update.dismissed";
/** Com o app aberto, procura de novo a cada 6 horas. */
const EVERY_MS = 6 * 60 * 60 * 1000;

function errMsg(e: unknown) {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}

/**
 * Avisa quando há uma versão nova do LocalTusk. A consulta só lê o número da última versão
 * publicada no GitHub; desligada, o app não faz nenhuma requisição por conta própria.
 */
export function useUpdate() {
  const [enabled, setEnabledState] = useState(() => localStorage.getItem(CHECK_KEY) !== "off");
  const [dismissed, setDismissed] = useState(() => localStorage.getItem(DISMISS_KEY));
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  /** `manual`: o usuário pediu, então mostra erros e volta a avisar de uma versão dispensada. */
  const check = useCallback(async (manual: boolean) => {
    if (manual) {
      setChecking(true);
      setError(null);
    }
    try {
      const r = await api.checkUpdate();
      setInfo(r);
      if (manual && r.available) {
        localStorage.removeItem(DISMISS_KEY);
        setDismissed(null);
      }
    } catch (e) {
      if (manual) setError(errMsg(e));
    } finally {
      if (manual) setChecking(false);
    }
  }, []);

  useEffect(() => {
    if (!enabled) return;
    check(false);
    const t = setInterval(() => check(false), EVERY_MS);
    return () => clearInterval(t);
  }, [enabled, check]);

  const setEnabled = (on: boolean) => {
    if (on) localStorage.removeItem(CHECK_KEY);
    else localStorage.setItem(CHECK_KEY, "off");
    setEnabledState(on);
  };

  const dismiss = () => {
    if (!info) return;
    localStorage.setItem(DISMISS_KEY, info.latest);
    setDismissed(info.latest);
  };

  const visible = enabled && !!info?.available && dismissed !== info.latest;
  return { enabled, setEnabled, info, checking, error, check, visible, dismiss };
}

export type UpdateState = ReturnType<typeof useUpdate>;

function open(url: string, onError: (m: string) => void) {
  api.openUpdatePage(url).catch((e) => onError(errMsg(e)));
}

/** Faixa no topo do app: há uma versão nova. */
export function UpdateBanner({ update, onError }: { update: UpdateState; onError: (m: string) => void }) {
  const info = update.info;
  if (!update.visible || !info) return null;
  return (
    <div className="banner update" role="status">
      <Icon name="download" />
      <div className="grow">
        <strong>Nova versão do LocalTusk: {info.latest}</strong>{" "}
        <span className="muted">Você usa a {info.current}.</span>
      </div>
      <div className="update-actions">
        <button className="btn small primary" onClick={() => open(info.downloadUrl ?? info.pageUrl, onError)}>
          <Icon name="download" size={13} /> {info.downloadUrl ? "Baixar" : "Ver downloads"}
        </button>
        {info.downloadUrl && (
          <button className="btn small" onClick={() => open(info.pageUrl, onError)}>
            Novidades
          </button>
        )}
        <button className="btn small" onClick={update.dismiss}>
          Agora não
        </button>
      </div>
    </div>
  );
}

/** Seção da tela "PHP e ferramentas": verificar agora e ligar ou desligar o aviso. */
export function UpdatePanel({ update, onError }: { update: UpdateState; onError: (m: string) => void }) {
  const { info } = update;
  return (
    <section className="panel update-panel">
      <h3>
        <Icon name="download" /> Atualizações do LocalTusk
      </h3>
      <div className="row between">
        <div>
          <strong>Avisar quando houver versão nova</strong>
          <p className="muted small">
            O app consulta o GitHub para ler o número da última versão. Não envia nada sobre você e não instala nada sozinho.
          </p>
        </div>
        <Switch checked={update.enabled} onChange={update.setEnabled} label="Avisar quando houver versão nova" />
      </div>
      <div className="row update-check">
        <button className="btn small" onClick={() => update.check(true)} disabled={update.checking}>
          {update.checking ? <Spinner size={13} /> : <Icon name="restore" size={13} />} Verificar agora
        </button>
        <span className="small" aria-live="polite">
          {update.error ? (
            <span className="muted">{update.error}</span>
          ) : info?.available ? (
            <>
              Há uma versão nova: <strong>{info.latest}</strong> (você usa a {info.current}).{" "}
              <button className="link" onClick={() => open(info.downloadUrl ?? info.pageUrl, onError)}>
                {info.downloadUrl ? "Baixar" : "Ver downloads"}
              </button>
            </>
          ) : info ? (
            <span className="muted">Você está na versão mais recente ({info.current}).</span>
          ) : (
            <span className="muted">Versão instalada: {APP_VERSION}</span>
          )}
        </span>
      </div>
    </section>
  );
}
