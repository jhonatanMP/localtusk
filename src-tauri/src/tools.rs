//! Descoberta dos binários nativos (PHP-FPM, Caddy, MySQL).
//!
//! Ordem de busca: `~/LocalTusk/bin` (binários gerenciados pelo app), depois locais
//! comuns do Homebrew e, por fim, o `PATH`. As versões do PHP gerenciadas ficam em
//! `~/LocalTusk/bin/php/<versão>/` (ver `php_manager`).

/// Pastas do sistema onde executáveis do PHP, do Caddy e dos bancos podem estar.
#[cfg(unix)]
const SYSTEM_DIRS: &[&str] = &["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin", "/usr/local/sbin", "/usr/bin"];
#[cfg(windows)]
const SYSTEM_DIRS: &[&str] = &[r"C:\Program Files\MySQL", r"C:\Program Files\PostgreSQL", r"C:\tools", r"C:\xampp\php", r"C:\php"];

use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn search_dirs(base: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![base.join("bin")];
    for p in SYSTEM_DIRS {
        dirs.push(PathBuf::from(p));
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs
}

/// Procura um executável pelo nome.
pub fn find(name: &str, base: &Path) -> Option<PathBuf> {
    search_dirs(base)
        .into_iter()
        .map(|d| d.join(crate::platform::name(name)))
        .find(|p| p.is_file())
}

/// Procura um executável só nos locais do sistema (ignora as pastas do app).
pub fn find_in_system(name: &str) -> Option<PathBuf> {
    search_dirs(Path::new(""))
        .into_iter()
        .filter(|d| d.is_absolute())
        .map(|d| d.join(crate::platform::name(name)))
        .find(|p| p.is_file())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhpInstall {
    /// Versão completa, ex.: `8.5.9`.
    pub full_version: String,
    /// Versão major.minor, ex.: `8.5`.
    pub version: String,
    pub fpm: PathBuf,
    /// `true` se foi instalada pelo LocalTusk (em `~/LocalTusk/bin/php`);
    /// `false` se veio do sistema (ex.: Homebrew).
    pub managed: bool,
}

impl PhpInstall {
    /// PHP de linha de comando que acompanha esta instalação (`php` ao lado do `php-fpm`,
    /// ou `../bin/php` no Homebrew).
    pub fn cli(&self) -> Option<PathBuf> {
        let dir = self.fpm.parent()?;
        let mut candidates = vec![dir.join("php")];
        if !self.managed {
            candidates.push(dir.join("../bin/php"));
            candidates.push(crate::platform::bin(dir, "php"));
        }
        candidates.into_iter().find(|p| p.is_file())
    }
}

/// Versões instaladas pelo próprio app. A versão vem do nome da pasta (sem executar nada).
fn managed_installs(base: &Path) -> Vec<PhpInstall> {
    let Ok(rd) = std::fs::read_dir(crate::php_manager::php_root(base)) else {
        return vec![];
    };
    rd.flatten()
        .filter_map(|e| {
            let full = e.file_name().to_string_lossy().to_string();
            let fpm = crate::platform::bin(&e.path(), "php-fpm");
            if !crate::php_manager::valid_version(&full) || !fpm.is_file() {
                return None;
            }
            let version = full.split('.').take(2).collect::<Vec<_>>().join(".");
            Some(PhpInstall {
                full_version: full,
                version,
                fpm,
                managed: true,
            })
        })
        .collect()
}

pub fn homebrew_prefixes() -> Vec<PathBuf> {
    ["/opt/homebrew", "/usr/local"].into_iter().map(PathBuf::from).collect()
}

fn php_fpm_candidates() -> Vec<PathBuf> {
    let mut out = vec![];
    #[cfg(windows)]
    {
        // Sem PHP-FPM no Windows: o servidor é o php-cgi.exe do PHP oficial (ou do XAMPP).
        for d in [r"C:\php", r"C:\xampp\php", r"C:\tools\php"] {
            out.push(Path::new(d).join("php-cgi.exe"));
        }
        if let Some(p) = find_in_system("php-cgi") {
            out.push(p);
        }
        return out;
    }
    for prefix in homebrew_prefixes() {
        out.push(prefix.join("opt/php/sbin/php-fpm"));
        out.push(prefix.join("sbin/php-fpm"));
        if let Ok(rd) = std::fs::read_dir(prefix.join("opt")) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n.starts_with("php@") {
                    out.push(e.path().join("sbin/php-fpm"));
                }
            }
        }
    }
    if let Some(p) = find_in_system("php-fpm") {
        out.push(p);
    }
    out
}

/// PHP-FPM instalados no sistema (Homebrew etc.), fora do controle do app.
fn system_installs() -> Vec<PhpInstall> {
    let mut seen = HashSet::new();
    let mut installs = Vec::new();
    for cand in php_fpm_candidates() {
        let Ok(real) = std::fs::canonicalize(&cand) else {
            continue;
        };
        if !seen.insert(real) {
            continue;
        }
        let Some(full) = fpm_version(&cand) else {
            continue;
        };
        let version = full.split('.').take(2).collect::<Vec<_>>().join(".");
        installs.push(PhpInstall {
            full_version: full,
            version,
            fpm: cand,
            managed: false,
        });
    }
    installs
}

fn fpm_version(fpm: &Path) -> Option<String> {
    let out = Command::new(fpm).arg("-v").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // "PHP 8.5.9 (fpm-fcgi) (built: ...)"
    text.split_whitespace().nth(1).map(|s| s.to_string())
}

/// Todas as instalações de PHP-FPM disponíveis, da versão mais nova para a mais antiga.
/// Se a mesma versão existir como gerenciada e como do sistema, vale a gerenciada.
pub fn php_installs(base: &Path) -> Vec<PhpInstall> {
    let mut installs = managed_installs(base);
    for sys in system_installs() {
        if !installs.iter().any(|i| i.full_version == sys.full_version) {
            installs.push(sys);
        }
    }
    installs.sort_by(|a, b| version_key(&b.full_version).cmp(&version_key(&a.full_version)));
    installs
}

fn version_key(v: &str) -> Vec<u32> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Escolhe o PHP-FPM para uma versão pedida: completa (`8.3.32`) ou só a série (`8.3`,
/// que usa o patch mais novo instalado). Sem correspondência, usa o mais novo de todos.
pub fn pick_php(base: &Path, wanted: &str) -> Option<PhpInstall> {
    let installs = php_installs(base);
    installs
        .iter()
        .find(|i| i.full_version == wanted)
        .or_else(|| installs.iter().find(|i| i.version == wanted))
        .cloned()
        .or_else(|| installs.into_iter().next())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub name: String,
    pub found: bool,
    pub path: Option<String>,
    pub install_hint: String,
}

pub fn status(base: &Path) -> Vec<ToolStatus> {
    let php = php_installs(base);
    let mut out = vec![ToolStatus {
        name: "PHP-FPM".into(),
        found: !php.is_empty(),
        path: php.first().map(|p| p.fpm.display().to_string()),
        install_hint: "instale em Versões do PHP".into(),
    }];
    let caddy = crate::extras::caddy_bin(base).or_else(|| find("caddy", base));
    out.push(ToolStatus {
        name: "Caddy".into(),
        found: caddy.is_some(),
        path: caddy.map(|p| p.display().to_string()),
        install_hint: "instale em PHP e ferramentas".into(),
    });
    // Basta um banco instalado (MySQL ou PostgreSQL); cada projeto usa o que escolher.
    let mysql = crate::mysql_manager::installs(base);
    let pg = crate::postgres_manager::installs(base);
    out.push(ToolStatus {
        name: "MySQL ou PostgreSQL".into(),
        found: !mysql.is_empty() || !pg.is_empty(),
        path: mysql
            .first()
            .map(|m| m.mysqld.display().to_string())
            .or_else(|| pg.first().map(|i| i.postgres.display().to_string())),
        install_hint: "instale em PHP e ferramentas".into(),
    });
    out
}
