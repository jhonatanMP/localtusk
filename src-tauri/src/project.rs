//! Modelo de projeto e persistência (`~/LocalTusk/projects.json`).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DB_USER: &str = "root";
pub const DB_PASSWORD: &str = "root";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    /// Slug único; também é o nome da pasta do projeto.
    pub id: String,
    pub name: String,
    /// Subpasta de `app/` servida pelo Caddy (ex.: `public`). Vazio = raiz.
    pub docroot: String,
    /// Versão major.minor do PHP (ex.: `8.3`). Vazio = a mais nova instalada.
    pub php_version: String,
    /// Banco do projeto: `mysql` (padrão, também quando vazio) ou `postgres`.
    #[serde(default)]
    pub db_engine: String,
    /// Versão do banco: MySQL (completa `8.4.9` ou série `8.4`) ou PostgreSQL (completa
    /// `16.16.0` ou major `16`). Vazio = a mais nova instalada.
    #[serde(default)]
    pub db_version: String,
    pub http_port: u16,
    pub db_port: u16,
    pub db_name: String,
    pub created_at: u64,
    /// Também serve o site por HTTPS (certificado da autoridade local do Caddy).
    #[serde(default)]
    pub https: bool,
    /// Porta HTTPS do projeto. 0 = ainda não reservada.
    #[serde(default)]
    pub https_port: u16,
    /// Aceita conexões de outros aparelhos da mesma rede (por padrão, só deste computador).
    #[serde(default)]
    pub lan: bool,
    /// Servidor Redis (Valkey) próprio do projeto.
    #[serde(default)]
    pub redis: bool,
    /// Porta do Redis. 0 = ainda não reservada.
    #[serde(default)]
    pub redis_port: u16,
    /// O botão do wp-admin entra direto, sem pedir senha (só WordPress).
    #[serde(default)]
    pub auto_login: bool,
    /// Administrador usado pelo login automático. Vazio = ainda não escolhido.
    #[serde(default)]
    pub auto_login_user: String,
}

pub const ENGINE_MYSQL: &str = "mysql";
pub const ENGINE_POSTGRES: &str = "postgres";

/// O projeto usa PostgreSQL (qualquer outro valor, inclusive vazio, é MySQL).
pub fn is_postgres(p: &Project) -> bool {
    p.db_engine == ENGINE_POSTGRES
}

/// Nome normalizado do banco do projeto: `mysql` ou `postgres`.
pub fn engine_of(p: &Project) -> &'static str {
    if is_postgres(p) { ENGINE_POSTGRES } else { ENGINE_MYSQL }
}

/// Aceita só os bancos que o app sabe gerenciar.
pub fn valid_engine(e: &str) -> bool {
    e == ENGINE_MYSQL || e == ENGINE_POSTGRES
}

/// Primeira porta a tentar para o banco de cada tipo (MySQL 3307, PostgreSQL 5433).
pub fn db_port_start(engine: &str) -> u16 {
    if engine == ENGINE_POSTGRES { 5433 } else { 3307 }
}

/// Primeira porta a tentar para o Redis.
pub const REDIS_PORT_START: u16 = 6380;

/// O projeto sobe o Redis de fato (ligado e com porta reservada).
pub fn redis_on(p: &Project) -> bool {
    p.redis && p.redis_port != 0
}

pub fn project_dir(base: &Path, id: &str) -> PathBuf {
    base.join("projects").join(id)
}

fn store_file(base: &Path) -> PathBuf {
    base.join("projects.json")
}

pub fn load(base: &Path) -> Vec<Project> {
    std::fs::read_to_string(store_file(base))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(base: &Path, projects: &[Project]) -> Result<(), String> {
    std::fs::create_dir_all(base).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(projects).map_err(|e| e.to_string())?;
    std::fs::write(store_file(base), json).map_err(|e| e.to_string())
}

pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for c in name.chars().flat_map(|c| c.to_lowercase()) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() { "projeto".into() } else { out }
}

/// Domínio do projeto: `<id>.localhost`. Navegadores e o macOS resolvem `*.localhost`
/// para 127.0.0.1 sem nenhuma configuração (e sem precisar de senha de administrador).
pub fn site_host(p: &Project) -> String {
    format!("{}.localhost", p.id)
}

/// Endereço HTTP do site: `http://<id>.localhost:<porta>`.
pub fn http_url(p: &Project) -> String {
    format!("http://{}:{}", site_host(p), p.http_port)
}

/// O projeto serve HTTPS de fato (ligado e com porta reservada).
pub fn https_on(p: &Project) -> bool {
    p.https && p.https_port != 0
}

/// Endereço principal do site: HTTPS quando está ligado, senão HTTP. É o que o WordPress
/// grava como endereço do site e o que o botão "Abrir site" abre.
pub fn site_url(p: &Project) -> String {
    if https_on(p) {
        format!("https://{}:{}", site_host(p), p.https_port)
    } else {
        http_url(p)
    }
}

/// IP deste computador na rede local. Abre um socket UDP "para" um endereço de
/// documentação só para o sistema escolher a interface: nenhum pacote é enviado.
pub fn lan_ip() -> Option<Ipv4Addr> {
    let sock = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    sock.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    match sock.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

/// Nome deste computador na rede (`MeuMac.local`), resolvido por mDNS/Bonjour.
/// Só no macOS; nos outros sistemas devolve `None`.
pub fn lan_hostname() -> Option<String> {
    static NAME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        #[cfg(target_os = "macos")]
        {
            let out = std::process::Command::new("scutil").args(["--get", "LocalHostName"]).output().ok()?;
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            (out.status.success() && !name.is_empty()).then(|| format!("{name}.local"))
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    })
    .clone()
}

/// Nomes pelos quais outros aparelhos alcançam este computador (IP e nome `.local`).
pub fn lan_hosts() -> Vec<String> {
    lan_ip().map(|ip| ip.to_string()).into_iter().chain(lan_hostname()).collect()
}

/// Endereços do projeto na rede local (HTTP e, se ligado, HTTPS). Vazio se o acesso
/// pela rede estiver desligado ou se o computador não estiver em nenhuma rede.
pub fn lan_urls(p: &Project) -> Vec<String> {
    if !p.lan {
        return vec![];
    }
    lan_hosts()
        .iter()
        .flat_map(|h| {
            let mut v = vec![format!("http://{h}:{}", p.http_port)];
            if https_on(p) {
                v.push(format!("https://{h}:{}", p.https_port));
            }
            v
        })
        .collect()
}

pub fn unique_id(name: &str, existing: &[Project]) -> String {
    let base = slugify(name);
    let taken: HashSet<&str> = existing.iter().map(|p| p.id.as_str()).collect();
    if !taken.contains(base.as_str()) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !taken.contains(c.as_str()))
        .unwrap()
}

pub fn port_is_free(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
}

/// Primeira porta livre a partir de `start`, ignorando as já reservadas por projetos.
pub fn free_port(start: u16, reserved: &HashSet<u16>) -> Option<u16> {
    (start..start.saturating_add(2000)).find(|p| !reserved.contains(p) && port_is_free(*p))
}

/// `wanted` se estiver livre e não reservada; senão a primeira livre a partir de `start`.
/// `wanted = 0` significa "qualquer uma" (nunca devolve 0).
pub fn pick_port(wanted: u16, start: u16, reserved: &HashSet<u16>) -> Option<u16> {
    if wanted != 0 && !reserved.contains(&wanted) && port_is_free(wanted) {
        Some(wanted)
    } else {
        free_port(start.max(1), reserved)
    }
}

/// `n` portas livres e distintas (para serviços que não precisam de porta fixa).
pub fn ephemeral_ports(n: usize) -> Option<Vec<u16>> {
    // Mantém todos os sockets abertos até o fim para o SO não repetir a mesma porta.
    let listeners: Vec<TcpListener> = (0..n)
        .map(|_| TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok())
        .collect::<Option<_>>()?;
    listeners
        .iter()
        .map(|l| l.local_addr().ok().map(|a| a.port()))
        .collect()
}

pub fn db_name_for(id: &str) -> String {
    id.replace('-', "_")
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn reserved_ports(projects: &[Project]) -> HashSet<u16> {
    projects
        .iter()
        .flat_map(|p| [p.http_port, p.db_port, p.https_port, p.redis_port])
        .filter(|port| *port != 0)
        .collect()
}

/// Pasta de dados atual e a do nome antigo do app (LocalPHP).
const DATA_DIR: &str = "LocalTusk";
const LEGACY_DATA_DIR: &str = "LocalPHP";

/// Pasta de dados do app dentro de `home`. Se só existir a pasta do nome antigo
/// (`~/LocalPHP`), ela é renomeada para `~/LocalTusk` — projetos, bancos, backups e
/// blueprints seguem intactos. Se o rename falhar, continua usando a pasta antiga.
pub fn resolve_base(home: &Path) -> PathBuf {
    let base = home.join(DATA_DIR);
    let legacy = home.join(LEGACY_DATA_DIR);
    if !base.exists() && legacy.is_dir() && std::fs::rename(&legacy, &base).is_err() {
        return legacy;
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_home(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("localtusk-test-home-{tag}-{}", now()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn migra_a_pasta_do_nome_antigo() {
        let home = tmp_home("migra");
        std::fs::create_dir_all(home.join("LocalPHP/projects/meu-site")).unwrap();
        std::fs::write(home.join("LocalPHP/projects.json"), "[]").unwrap();
        let base = resolve_base(&home);
        assert_eq!(base, home.join("LocalTusk"));
        assert!(base.join("projects/meu-site").is_dir());
        assert!(base.join("projects.json").is_file());
        assert!(!home.join("LocalPHP").exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn nao_mexe_se_a_pasta_nova_ja_existe() {
        let home = tmp_home("nova");
        std::fs::create_dir_all(home.join("LocalPHP")).unwrap();
        std::fs::create_dir_all(home.join("LocalTusk")).unwrap();
        let base = resolve_base(&home);
        assert_eq!(base, home.join("LocalTusk"));
        assert!(home.join("LocalPHP").is_dir());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn instalacao_nova_usa_a_pasta_nova() {
        let home = tmp_home("fresh");
        assert_eq!(resolve_base(&home), home.join("LocalTusk"));
        assert!(!home.join("LocalPHP").exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    fn sample() -> Project {
        Project { id: "loja".into(), http_port: 8000, db_port: 3307, ..Default::default() }
    }

    #[test]
    fn projetos_antigos_sem_campos_de_https_e_rede_continuam_carregando() {
        let old = r#"[{"id":"loja","name":"Loja","docroot":"","phpVersion":"","httpPort":8000,"dbPort":3307,"dbName":"loja","createdAt":1}]"#;
        let list: Vec<Project> = serde_json::from_str(old).unwrap();
        assert!(!list[0].https && !list[0].lan && list[0].https_port == 0);
        assert_eq!(site_url(&list[0]), "http://loja.localhost:8000");
    }

    #[test]
    fn projeto_antigo_vem_sem_login_automatico() {
        let old = r#"{"id":"loja","name":"Loja","docroot":"","phpVersion":"","httpPort":8000,"dbPort":3307,"dbName":"loja","createdAt":1}"#;
        let p: Project = serde_json::from_str(old).unwrap();
        assert!(!p.auto_login);
        assert_eq!(p.auto_login_user, "");
    }

    #[test]
    fn endereco_principal_vira_https_so_com_porta_reservada() {
        let mut p = sample();
        p.https = true;
        assert_eq!(site_url(&p), "http://loja.localhost:8000", "sem porta, continua HTTP");
        p.https_port = 8443;
        assert_eq!(site_url(&p), "https://loja.localhost:8443");
        assert_eq!(http_url(&p), "http://loja.localhost:8000");
        p.https = false;
        assert_eq!(site_url(&p), "http://loja.localhost:8000");
    }

    #[test]
    fn portas_https_entram_nas_reservadas_e_zero_e_ignorado() {
        let mut a = sample();
        a.https_port = 8443;
        let b = Project { id: "b".into(), http_port: 8001, db_port: 3308, ..Default::default() };
        let r = reserved_ports(&[a, b]);
        assert!(r.contains(&8443) && r.contains(&8000) && r.contains(&3308));
        assert!(!r.contains(&0));
    }

    #[test]
    fn projetos_antigos_sem_banco_nem_redis_sao_mysql_sem_redis() {
        let old = r#"[{"id":"loja","name":"Loja","docroot":"","phpVersion":"","httpPort":8000,"dbPort":3307,"dbName":"loja","createdAt":1}]"#;
        let list: Vec<Project> = serde_json::from_str(old).unwrap();
        assert_eq!(engine_of(&list[0]), "mysql");
        assert!(!is_postgres(&list[0]) && !redis_on(&list[0]));
    }

    #[test]
    fn banco_e_redis_so_valem_com_valores_conhecidos() {
        assert!(valid_engine("mysql") && valid_engine("postgres"));
        assert!(!valid_engine("") && !valid_engine("mariadb") && !valid_engine("../x"));
        let mut p = sample();
        p.db_engine = "postgres".into();
        assert_eq!(engine_of(&p), "postgres");
        assert_eq!(db_port_start(engine_of(&p)), 5433);
        assert_eq!(db_port_start("mysql"), 3307);
        p.redis = true;
        assert!(!redis_on(&p), "sem porta reservada o Redis não sobe");
        p.redis_port = 6380;
        assert!(redis_on(&p));
        assert!(reserved_ports(&[p]).contains(&6380));
    }

    #[test]
    fn enderecos_da_rede_so_aparecem_com_a_rede_ligada() {
        let mut p = sample();
        assert!(lan_urls(&p).is_empty());
        p.lan = true;
        p.https = true;
        p.https_port = 8443;
        for u in lan_urls(&p) {
            assert!(u.starts_with("http://") && u.ends_with(":8000") || u.starts_with("https://") && u.ends_with(":8443"), "{u}");
            assert!(!u.contains("localhost") && !u.contains("127.0.0.1"), "{u}");
        }
    }

    #[test]
    fn pick_port_com_zero_escolhe_uma_porta_livre() {
        let none = HashSet::new();
        let p = pick_port(0, 8000, &none).expect("porta");
        assert_ne!(p, 0);
        assert!(p >= 8000, "{p}");

        let w = free_port(18900, &none).expect("porta livre");
        assert_eq!(pick_port(w, 8000, &none), Some(w));

        let reserved: HashSet<u16> = [w].into_iter().collect();
        let other = pick_port(w, w, &reserved).expect("porta");
        assert_ne!(other, w);
    }
}
