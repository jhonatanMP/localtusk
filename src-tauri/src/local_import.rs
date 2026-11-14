//! Leitura dos sites do Local (localwp.com): lê `sites.json` e
//! `site-statuses.json` e diz o estado de cada site, sem mexer em nada.

use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MSG_RODANDO: &str = "Pare o site no Local para importar (se o Local estiver fechado, abra-o e pare o site).";
const MSG_SEM_DUMP: &str = "O Local ainda não salvou o banco deste site. Inicie e pare o site no Local e tente de novo.";
const MSG_MULTISITE: &str = "Sites multisite podem precisar de ajustes manuais.";
const MSG_APACHE: &str = "Regras do .htaccess não são usadas pelo LocalTusk.";
const MSG_DUMP_VELHO: &str = "O banco salvo pelo Local pode estar desatualizado. Inicie e pare o site no Local para garantir.";

/// Folga entre o dump e o banco vivo antes de considerar o dump desatualizado.
const FOLGA_DUMP: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSite {
    pub id: String,
    pub name: String,
    pub path: String,
    pub domain: String,
    /// Versão completa do Local, ex. "8.2.29" ("" se ausente).
    pub php_version: String,
    /// Ex. "8.4.0".
    pub mysql_version: String,
    /// "nginx" | "apache" | "".
    pub web_server: String,
    pub multisite: bool,
    pub running: bool,
    /// mtime do local.sql em segundos desde 1970; 0 se não existe.
    pub dump_saved_at: u64,
    pub dump_stale: bool,
    /// Mensagem que impede a importação.
    pub blocker: Option<String>,
    /// Avisos: multisite, apache, dump desatualizado.
    pub warnings: Vec<String>,
}

/// Pasta de dados do Local neste sistema (`None` no Windows, sem suporte).
pub fn local_data_dir(home: &Path) -> Option<PathBuf> {
    if cfg!(target_os = "linux") {
        Some(home.join(".config/Local"))
    } else if cfg!(target_os = "macos") {
        Some(home.join("Library/Application Support/Local"))
    } else {
        None
    }
}

pub fn public_dir(site: &LocalSite) -> PathBuf {
    Path::new(&site.path).join("app/public")
}

pub fn dump_file(site: &LocalSite) -> PathBuf {
    Path::new(&site.path).join("app/sql/local.sql")
}

/// "8.2.29" -> "8.2".
pub fn series(full: &str) -> String {
    full.split('.').take(2).collect::<Vec<_>>().join(".")
}

pub fn list_sites(data_dir: &Path, home: &Path) -> Result<Vec<LocalSite>, String> {
    let file = data_dir.join("sites.json");
    let raw = fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let sites: Value = serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", file.display()))?;
    let obj = sites.as_object().ok_or_else(|| format!("{}: formato inesperado", file.display()))?;
    let statuses: Value = fs::read_to_string(data_dir.join("site-statuses.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);

    let mut out: Vec<LocalSite> = obj
        .iter()
        .map(|(id, v)| build_site(data_dir, home, id, v, statuses.get(id).and_then(Value::as_str)))
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.id.cmp(&b.id)));
    Ok(out)
}

pub fn find_site(data_dir: &Path, home: &Path, id: &str) -> Result<LocalSite, String> {
    list_sites(data_dir, home)?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("Site '{id}' não encontrado no Local."))
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn mtime(p: &Path) -> Option<SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Maior mtime de qualquer arquivo dentro de `dir`, recursivamente.
fn newest_mtime(dir: &Path) -> Option<SystemTime> {
    let mut best: Option<SystemTime> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        // `file_type` não segue links: um link para pasta (ou em laço) conta só o próprio mtime.
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        let t = if is_dir { newest_mtime(&p) } else { mtime(&p) };
        if let Some(t) = t
            && best.is_none_or(|b| t > b)
        {
            best = Some(t);
        }
    }
    best
}

fn build_site(data_dir: &Path, home: &Path, id: &str, v: &Value, status: Option<&str>) -> LocalSite {
    let raw_path = text(v, "path");
    let path = match raw_path.strip_prefix('~') {
        Some(rest) => home.join(rest.trim_start_matches('/')).display().to_string(),
        None => raw_path,
    };
    let services = v.get("services");
    let service_version = |name: &str| {
        services
            .and_then(|s| s.get(name))
            .and_then(|s| s.get("version"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let has_service = |name: &str| services.and_then(|s| s.get(name)).is_some();
    let web_server = if has_service("apache") {
        "apache"
    } else if has_service("nginx") {
        "nginx"
    } else {
        ""
    };
    let multisite = v
        .get("multiSite")
        .and_then(Value::as_str)
        .is_some_and(|m| !m.is_empty() && m != "no");

    let mut site = LocalSite {
        id: id.to_string(),
        name: text(v, "name"),
        path,
        domain: text(v, "domain"),
        php_version: service_version("php"),
        // Sites antigos do Local usam MariaDB no lugar do MySQL.
        mysql_version: Some(service_version("mysql"))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| service_version("mariadb")),
        web_server: web_server.to_string(),
        multisite,
        running: status == Some("running"),
        dump_saved_at: 0,
        dump_stale: false,
        blocker: None,
        warnings: Vec::new(),
    };

    let dump_time = mtime(&dump_file(&site));
    if let Some(t) = dump_time {
        site.dump_saved_at = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let live = newest_mtime(&data_dir.join("run").join(id).join("mysql/data"));
        site.dump_stale = live.is_some_and(|l| l > t + FOLGA_DUMP);
    }

    site.blocker = if site.running {
        Some(MSG_RODANDO.to_string())
    } else if !public_dir(&site).is_dir() {
        Some(format!("A pasta do site não foi encontrada: {}.", public_dir(&site).display()))
    } else if dump_time.is_none() {
        Some(MSG_SEM_DUMP.to_string())
    } else {
        None
    };

    if site.multisite {
        site.warnings.push(MSG_MULTISITE.to_string());
    }
    if site.web_server == "apache" {
        site.warnings.push(MSG_APACHE.to_string());
    }
    if site.dump_stale {
        site.warnings.push(MSG_DUMP_VELHO.to_string());
    }
    site
}

/// Links simbólicos encontrados por [`copy_tree`] (caminhos relativos, com `/`).
#[derive(Debug, Default, PartialEq)]
pub struct CopiedLinks {
    /// Links para pastas, copiados como pastas de verdade.
    pub dirs: Vec<String>,
    /// Links para arquivos, copiados como arquivos.
    pub files: Vec<String>,
    /// Links quebrados, ilegíveis ou em laço, que ficaram de fora.
    pub skipped: Vec<String>,
}

/// Copia `src` para `dst` (criando `dst`). Links simbólicos viram cópia do
/// conteúdo apontado; laços e links quebrados são pulados. `on_file` recebe a
/// contagem acumulada de arquivos copiados. Devolve os links encontrados,
/// separados entre copiados e pulados. A origem só é lida, nunca alterada.
pub fn copy_tree(src: &Path, dst: &Path, on_file: &dyn Fn(u64)) -> Result<CopiedLinks, String> {
    let raiz = src
        .canonicalize()
        .map_err(|e| format!("Não foi possível ler {}: {e}", src.display()))?;
    let mut links = CopiedLinks::default();
    let mut count = 0u64;
    copy_dir(&raiz, dst, "", &mut vec![raiz.clone()], &mut count, on_file, &mut links)?;
    Ok(links)
}

fn copy_dir(
    src: &Path,
    dst: &Path,
    rel: &str,
    ancestrais: &mut Vec<PathBuf>,
    count: &mut u64,
    on_file: &dyn Fn(u64),
    links: &mut CopiedLinks,
) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("Não foi possível criar {}: {e}", dst.display()))?;
    let entries = fs::read_dir(src).map_err(|e| format!("Não foi possível ler {}: {e}", src.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("Não foi possível ler {}: {e}", src.display()))?;
        let path = entry.path();
        let name = entry.file_name();
        let rel_item = if rel.is_empty() {
            name.to_string_lossy().into_owned()
        } else {
            format!("{rel}/{}", name.to_string_lossy())
        };
        let alvo_dst = dst.join(&name);
        let meta = fs::symlink_metadata(&path).map_err(|e| format!("Não foi possível ler {}: {e}", path.display()))?;
        let eh_link = meta.file_type().is_symlink();
        // Resolve o link (se for) para o destino real; quebrado = pula.
        let real = if eh_link {
            match path.canonicalize() {
                Ok(r) => r,
                Err(_) => {
                    links.skipped.push(rel_item);
                    continue;
                }
            }
        } else {
            path.clone()
        };
        let meta_real = match fs::metadata(&real) {
            Ok(m) => m,
            Err(_) => {
                if eh_link {
                    links.skipped.push(rel_item);
                }
                continue;
            }
        };
        if meta_real.is_dir() {
            // Link para uma pasta ancestral = laço.
            if eh_link && ancestrais.contains(&real) {
                links.skipped.push(rel_item);
                continue;
            }
            if eh_link {
                links.dirs.push(rel_item.clone());
            }
            ancestrais.push(real.clone());
            let r = copy_dir(&real, &alvo_dst, &rel_item, ancestrais, count, on_file, links);
            ancestrais.pop();
            r?;
        } else if meta_real.is_file() {
            fs::copy(&real, &alvo_dst).map_err(|e| format!("Não foi possível copiar {}: {e}", path.display()))?;
            if eh_link {
                links.files.push(rel_item);
            }
            *count += 1;
            on_file(*count);
        } else if eh_link {
            links.skipped.push(rel_item);
        }
    }
    Ok(())
}

/// (major, minor) de uma série como "8.2".
fn serie_num(s: &str) -> (u32, u32) {
    let mut it = s.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0))
}

/// Escolhe, entre as séries instaladas, a que vai no lugar de `wanted`: a mesma major mais próxima
/// (empate → a mais nova); sem a mesma major, a mais nova. O `bool` indica que a escolhida é mais antiga.
pub fn fallback_version(wanted: &str, installed: &[String]) -> Option<(String, bool)> {
    let w = serie_num(wanted);
    let melhor = installed
        .iter()
        .filter(|s| serie_num(s).0 == w.0)
        .min_by_key(|s| {
            let n = serie_num(s);
            (n.1.abs_diff(w.1), std::cmp::Reverse(n.1))
        })
        .or_else(|| installed.iter().max_by_key(|s| serie_num(s)))?;
    Some((melhor.clone(), serie_num(melhor) < w))
}

/// Série a usar quando o Local não diz a versão (`wanted` vazio): a mais nova instalada;
/// sem nenhuma instalada, a mais nova do catálogo (para oferecer a instalação).
pub fn wanted_or_newest(wanted: &str, installed: &[String], catalog: &[String]) -> String {
    if !wanted.is_empty() {
        return wanted.to_string();
    }
    let newest = |xs: &[String]| xs.iter().filter(|x| !x.is_empty()).max_by_key(|x| serie_num(x)).cloned();
    newest(installed).or_else(|| newest(catalog)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_base(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("localtusk-li-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(p: &Path, content: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    /// Monta um "Local" falso e devolve (data_dir, home).
    fn fake_local(dir: &Path) -> (PathBuf, PathBuf) {
        let home = dir.join("home");
        let data = home.join(".config/Local");
        fs::create_dir_all(&data).unwrap();
        let sites = json!({
            "snap1": {"name": "snap", "path": "~/Local Sites/snap", "domain": "snap.local",
                "services": {"php": {"version": "8.2.29"}, "nginx": {"version": "1.26.1"}, "mysql": {"version": "8.4.0"}}},
            "run1": {"name": "rodando", "path": "~/Local Sites/rodando", "domain": "rodando.local",
                "services": {"php": {"version": "8.1.0"}, "nginx": {}, "mysql": {"version": "8.0.0"}}},
            "nodump": {"name": "semdump", "path": "~/Local Sites/semdump", "services": {"nginx": {}}},
            "gone": {"name": "sumiu", "path": "~/Local Sites/sumiu", "services": {}},
            "bare": {"name": "nu", "path": "~/Local Sites/nu"},
            "ms": {"name": "multi", "path": "~/Local Sites/multi", "multiSite": "ms-subdir",
                "services": {"apache": {}, "php": {"version": "8.2.0"}}},
        });
        write(&data.join("sites.json"), &sites.to_string());
        write(
            &data.join("site-statuses.json"),
            &json!({"snap1": "halted", "run1": "running", "nodump": "halted", "ms": "halted"}).to_string(),
        );
        for (name, dump) in [("snap", true), ("rodando", true), ("semdump", false), ("nu", true), ("multi", true)] {
            let root = home.join("Local Sites").join(name);
            fs::create_dir_all(root.join("app/public")).unwrap();
            if dump {
                write(&root.join("app/sql/local.sql"), "-- dump");
            }
        }
        (data, home)
    }

    fn get(sites: &[LocalSite], id: &str) -> LocalSite {
        sites.iter().find(|x| x.id == id).unwrap().clone()
    }

    #[test]
    fn lista_site_parado_pronto() {
        let dir = temp_base("pronto");
        let (data, home) = fake_local(&dir);
        let s = list_sites(&data, &home).unwrap();
        let snap = s.iter().find(|x| x.id == "snap1").unwrap();
        assert_eq!(snap.path, home.join("Local Sites/snap").display().to_string());
        assert_eq!((snap.php_version.as_str(), snap.mysql_version.as_str(), snap.web_server.as_str()), ("8.2.29", "8.4.0", "nginx"));
        assert!(snap.blocker.is_none() && !snap.running && snap.dump_saved_at > 0);
        assert!(snap.warnings.is_empty());
        let names: Vec<_> = s.iter().map(|x| x.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "ordenado por nome");
    }

    #[test]
    fn site_rodando_fica_bloqueado() {
        let dir = temp_base("rodando");
        let (data, home) = fake_local(&dir);
        let s = get(&list_sites(&data, &home).unwrap(), "run1");
        assert!(s.running);
        assert_eq!(
            s.blocker.as_deref(),
            Some("Pare o site no Local para importar (se o Local estiver fechado, abra-o e pare o site).")
        );
    }

    #[test]
    fn site_sem_dump_fica_bloqueado() {
        let dir = temp_base("semdump");
        let (data, home) = fake_local(&dir);
        let s = get(&list_sites(&data, &home).unwrap(), "nodump");
        assert_eq!(
            s.blocker.as_deref(),
            Some("O Local ainda não salvou o banco deste site. Inicie e pare o site no Local e tente de novo.")
        );
        assert_eq!(s.dump_saved_at, 0);
    }

    #[test]
    fn site_sem_pasta_fica_bloqueado_e_lista_continua() {
        let dir = temp_base("sempasta");
        let (data, home) = fake_local(&dir);
        let all = list_sites(&data, &home).unwrap();
        let s = get(&all, "gone");
        let esperado = format!("A pasta do site não foi encontrada: {}.", public_dir(&s).display());
        assert!(public_dir(&s).ends_with("sumiu/app/public"));
        assert_eq!(s.blocker.as_deref(), Some(esperado.as_str()));
        assert!(get(&all, "snap1").blocker.is_none());
    }

    #[test]
    fn raiz_existe_mas_sem_app_public_fica_bloqueado() {
        let dir = temp_base("sempublic");
        let (data, home) = fake_local(&dir);
        fs::remove_dir_all(home.join("Local Sites/snap/app/public")).unwrap();
        let s = get(&list_sites(&data, &home).unwrap(), "snap1");
        let esperado = format!("A pasta do site não foi encontrada: {}.", public_dir(&s).display());
        assert!(public_dir(&s).ends_with("snap/app/public"));
        assert_eq!(s.blocker.as_deref(), Some(esperado.as_str()));
    }

    #[test]
    fn campos_faltando_nao_derrubam() {
        let dir = temp_base("faltando");
        let (data, home) = fake_local(&dir);
        let s = get(&list_sites(&data, &home).unwrap(), "bare");
        assert_eq!(s.php_version, "");
        assert_eq!(s.mysql_version, "");
        assert_eq!(s.web_server, "");
        assert_eq!(s.domain, "");
    }

    #[test]
    fn multisite_e_apache_avisam() {
        let dir = temp_base("multi");
        let (data, home) = fake_local(&dir);
        let s = get(&list_sites(&data, &home).unwrap(), "ms");
        assert!(s.multisite);
        assert_eq!(s.web_server, "apache");
        assert!(s.warnings.contains(&"Sites multisite podem precisar de ajustes manuais.".to_string()));
        assert!(s.warnings.contains(&"Regras do .htaccess não são usadas pelo LocalTusk.".to_string()));
        assert!(s.blocker.is_none());
    }

    #[test]
    fn dump_desatualizado_avisa() {
        let dir = temp_base("stale");
        let (data, home) = fake_local(&dir);
        let dump = home.join("Local Sites/snap/app/sql/local.sql");
        let dump_time = mtime(&dump).unwrap();
        let ibdata = data.join("run/snap1/mysql/data/ibdata1");
        write(&ibdata, "x");
        let set = |t: SystemTime| fs::File::options().write(true).open(&ibdata).unwrap().set_modified(t).unwrap();

        set(dump_time + Duration::from_secs(300));
        let s = get(&list_sites(&data, &home).unwrap(), "snap1");
        assert!(s.dump_stale);
        assert!(s.warnings.contains(
            &"O banco salvo pelo Local pode estar desatualizado. Inicie e pare o site no Local para garantir.".to_string()
        ));
        assert!(s.blocker.is_none());

        set(dump_time + Duration::from_secs(1));
        let s = get(&list_sites(&data, &home).unwrap(), "snap1");
        assert!(!s.dump_stale);
        assert!(s.warnings.is_empty());
    }

    #[test]
    fn sem_local_instalado_da_erro() {
        let dir = temp_base("semlocal");
        assert!(list_sites(&dir, &dir).is_err());
        assert!(find_site(&dir, &dir, "x").is_err());
    }

    #[test]
    fn serie_da_versao() {
        assert_eq!(series("8.2.29"), "8.2");
        assert_eq!(series("8.4"), "8.4");
        assert_eq!(series(""), "");
    }

    #[test]
    fn acha_site_por_id() {
        let dir = temp_base("find");
        let (data, home) = fake_local(&dir);
        assert_eq!(find_site(&data, &home, "snap1").unwrap().name, "snap");
        assert!(find_site(&data, &home, "nope").is_err());
        assert!(public_dir(&find_site(&data, &home, "snap1").unwrap()).ends_with("snap/app/public"));
        assert!(dump_file(&find_site(&data, &home, "snap1").unwrap()).ends_with("app/sql/local.sql"));
        assert!(local_data_dir(&home).is_some() || cfg!(windows));
    }

    #[test]
    fn copia_arquivos_e_subpastas() {
        let base = temp_base("copia");
        write(&base.join("src/a.txt"), "A");
        write(&base.join("src/sub/b.txt"), "B");
        let n = std::cell::Cell::new(0u64);
        let links = copy_tree(&base.join("src"), &base.join("dst"), &|c| n.set(c)).unwrap();
        assert_eq!(links, CopiedLinks::default());
        assert_eq!(fs::read_to_string(base.join("dst/a.txt")).unwrap(), "A");
        assert_eq!(fs::read_to_string(base.join("dst/sub/b.txt")).unwrap(), "B");
        assert_eq!(n.get(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn link_vira_copia_e_e_listado() {
        let base = temp_base("link");
        write(&base.join("fora/meu/x.php"), "X");
        let plugins = base.join("src/wp-content/plugins");
        fs::create_dir_all(&plugins).unwrap();
        std::os::unix::fs::symlink("../../../fora/meu", plugins.join("meu")).unwrap();
        let links = copy_tree(&base.join("src"), &base.join("dst"), &|_| {}).unwrap();
        assert_eq!(links.dirs, vec!["wp-content/plugins/meu".to_string()]);
        assert!(links.files.is_empty() && links.skipped.is_empty());
        let d = base.join("dst/wp-content/plugins/meu");
        assert!(!fs::symlink_metadata(&d).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(d.join("x.php")).unwrap(), "X");
    }

    #[cfg(unix)]
    #[test]
    fn laco_de_link_nao_trava() {
        let base = temp_base("laco");
        write(&base.join("src/sub/f.txt"), "F");
        std::os::unix::fs::symlink("..", base.join("src/sub/loop")).unwrap();
        let links = copy_tree(&base.join("src"), &base.join("dst"), &|_| {}).unwrap();
        assert_eq!(links.skipped, vec!["sub/loop".to_string()]);
        assert!(links.dirs.is_empty(), "laço não é listado como copiado");
        assert!(base.join("dst/sub/f.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn link_quebrado_e_pulado_e_listado() {
        let base = temp_base("quebrado");
        write(&base.join("src/a.txt"), "A");
        std::os::unix::fs::symlink("../nao-existe", base.join("src/morto")).unwrap();
        let links = copy_tree(&base.join("src"), &base.join("dst"), &|_| {}).unwrap();
        assert_eq!(links.skipped, vec!["morto".to_string()]);
        assert!(links.dirs.is_empty() && links.files.is_empty());
        assert!(fs::symlink_metadata(base.join("dst/morto")).is_err());
        assert!(base.join("dst/a.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn link_para_arquivo_vira_arquivo() {
        let base = temp_base("linkarq");
        write(&base.join("fora/x.txt"), "X");
        fs::create_dir_all(base.join("src")).unwrap();
        std::os::unix::fs::symlink("../fora/x.txt", base.join("src/x.txt")).unwrap();
        let links = copy_tree(&base.join("src"), &base.join("dst"), &|_| {}).unwrap();
        assert_eq!(links.files, vec!["x.txt".to_string()]);
        assert!(links.dirs.is_empty() && links.skipped.is_empty());
        let d = base.join("dst/x.txt");
        assert!(fs::symlink_metadata(&d).unwrap().file_type().is_file());
        assert_eq!(fs::read_to_string(d).unwrap(), "X");
    }

    #[cfg(unix)]
    #[test]
    fn mtime_mais_novo_nao_segue_links() {
        let base = temp_base("mtimelaco");
        write(&base.join("data/sub/f"), "x");
        std::os::unix::fs::symlink("..", base.join("data/sub/loop")).unwrap();
        std::os::unix::fs::symlink("../data", base.join("data/self")).unwrap();
        assert!(newest_mtime(&base.join("data")).is_some());
    }

    #[test]
    fn mariadb_conta_como_versao_do_banco() {
        let dir = temp_base("mariadb");
        let (data, home) = fake_local(&dir);
        let raw = fs::read_to_string(data.join("sites.json")).unwrap();
        let mut v: Value = serde_json::from_str(&raw).unwrap();
        v["maria"] = json!({"name": "maria", "path": "~/Local Sites/maria",
            "services": {"php": {"version": "7.4.30"}, "nginx": {}, "mariadb": {"version": "10.4.32"}}});
        write(&data.join("sites.json"), &v.to_string());
        let s = get(&list_sites(&data, &home).unwrap(), "maria");
        assert_eq!(s.mysql_version, "10.4.32");
        assert_eq!(get(&list_sites(&data, &home).unwrap(), "snap1").mysql_version, "8.4.0");
    }

    #[cfg(unix)]
    #[test]
    fn origem_fica_intacta() {
        let base = temp_base("intacta");
        write(&base.join("fora/meu/x.php"), "X");
        write(&base.join("src/a.txt"), "A");
        std::os::unix::fs::symlink("../fora/meu", base.join("src/lnk")).unwrap();
        copy_tree(&base.join("src"), &base.join("dst"), &|_| {}).unwrap();
        write(&base.join("dst/a.txt"), "mudou");
        write(&base.join("dst/lnk/x.php"), "mudou");
        assert_eq!(fs::read_to_string(base.join("src/a.txt")).unwrap(), "A");
        assert_eq!(fs::read_to_string(base.join("fora/meu/x.php")).unwrap(), "X");
        assert_eq!(fs::read_link(base.join("src/lnk")).unwrap(), PathBuf::from("../fora/meu"));
    }

    #[test]
    fn versao_vazia_usa_a_mais_nova() {
        let v = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(wanted_or_newest("8.2", &v(&["8.3"]), &v(&["8.4"])), "8.2");
        assert_eq!(wanted_or_newest("", &v(&["8.1", "8.10", "8.3"]), &v(&["9.0"])), "8.10");
        assert_eq!(wanted_or_newest("", &[], &v(&["8.0", "8.4"])), "8.4");
        assert_eq!(wanted_or_newest("", &[], &[]), "");
    }

    #[test]
    fn fallback_escolhe_mais_proxima() {
        let v = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(fallback_version("8.2", &v(&["8.3", "8.1"])), Some(("8.3".into(), false)));
        assert_eq!(fallback_version("8.2", &v(&["8.1"])), Some(("8.1".into(), true)));
        assert_eq!(fallback_version("7.4", &v(&["8.3", "8.4"])), Some(("8.4".into(), false)));
        assert_eq!(fallback_version("8.4", &v(&["8.0"])), Some(("8.0".into(), true)));
        assert_eq!(fallback_version("8.2", &[]), None);
    }
}
