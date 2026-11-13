//! Gerenciador de versões do PHP.
//!
//! Baixa binários estáticos do PHP-FPM (projeto static-php-cli, pacote `bulk`, que inclui
//! mysqli, intl, opcache, imagick, sodium, etc.) e os guarda em
//! `~/LocalTusk/bin/php/<versão>/php-fpm`. O usuário nunca precisa escolher onde instalar.

use crate::download::{self, InstallProgress};
use crate::tools;
use serde::Serialize;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;

const BASE_URL: &str = "https://dl.static-php.dev/static-php-cli/bulk";
/// Pacote "common" do static-php-cli: traz `pdo_pgsql` e `pgsql` (o "bulk" só tem `pgsql`),
/// mas não tem `mysqli`, `intl` nem `opcache`. Por isso projetos com PostgreSQL usam este PHP
/// (guardado ao lado do outro, como `php-fpm-pg` e `php-pg`) e os de MySQL continuam no "bulk".
const COMMON_URL: &str = "https://dl.static-php.dev/static-php-cli/common";

/// Sufixo de plataforma usado nos nomes dos arquivos do static-php-cli.
fn platform() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("macos-aarch64"),
        ("macos", "x86_64") => Some("macos-x86_64"),
        ("linux", "aarch64") => Some("linux-aarch64"),
        ("linux", "x86_64") => Some("linux-x86_64"),
        _ => None,
    }
}

pub fn php_root(base: &Path) -> PathBuf {
    base.join("bin").join("php")
}

#[allow(unreachable_code)]
pub fn install_dir(base: &Path, version: &str) -> PathBuf {
    php_root(base).join(version)
}

/// Aceita só `X.Y.Z` numérico, o que também impede path traversal.
pub fn valid_version(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit()))
}

fn version_key(v: &str) -> Vec<u32> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhpRelease {
    pub version: String,
    pub minor: String,
    /// Tamanho do download, como informado pelo servidor (ex.: `33.9M`).
    pub size: String,
    pub installed: bool,
    /// `true` se for o patch mais novo da sua série (8.3.x).
    pub latest: bool,
}


// ------------------------------------------------------------------ Windows
//
// O PHP oficial do Windows (windows.php.net) não tem PHP-FPM: quem atende o FastCGI é o
// `php-cgi.exe`. Guardamos uma cópia dele como `php-fpm.exe` para o resto do app usar o
// mesmo nome nos três sistemas. O pacote é único para MySQL e PostgreSQL (as extensões
// `php_pgsql.dll`/`php_pdo_pgsql.dll` vêm nele), então não há pacote "common" no Windows.

#[cfg(windows)]
const WIN_RELEASES: &str = "https://windows.php.net/downloads/releases";
#[cfg(windows)]
const WIN_ARCHIVES: &str = "https://windows.php.net/downloads/releases/archives";

/// Compilador do pacote oficial para a série: VS17 no 8.4/8.5, VS16 do 8.0 ao 8.3.
#[cfg(windows)]
fn win_compiler(version: &str) -> Option<&'static str> {
    match version.split('.').take(2).collect::<Vec<_>>().join(".").as_str() {
        "8.5" | "8.4" => Some("vs17"),
        "8.3" | "8.2" | "8.1" | "8.0" => Some("vs16"),
        _ => None,
    }
}

#[cfg(windows)]
fn win_file(version: &str) -> Option<String> {
    Some(format!("php-{version}-nts-Win32-{}-x64.zip", win_compiler(version)?))
}

#[cfg(windows)]
fn win_urls(version: &str) -> Vec<String> {
    let Some(file) = win_file(version) else { return vec![] };
    vec![format!("{WIN_RELEASES}/{file}"), format!("{WIN_ARCHIVES}/{file}")]
}

/// Lê a listagem de arquivos antigos do windows.php.net e monta o catálogo (x64, NTS).
#[cfg(windows)]
fn parse_win_catalog(html: &str, installed: &[String]) -> Vec<PhpRelease> {
    let re = regex::Regex::new(r"php-(\d+\.\d+\.\d+)-nts-Win32-vs\d+-x64\.zip").unwrap();
    let mut versions: Vec<String> = re
        .captures_iter(html)
        .filter_map(|c| {
            let v = c[1].to_string();
            (valid_version(&v) && win_compiler(&v).is_some()).then_some(v)
        })
        .collect();
    versions.sort_by(|a, b| version_key(b).cmp(&version_key(a)));
    versions.dedup();
    let mut seen = std::collections::HashSet::new();
    versions
        .into_iter()
        .map(|version| {
            let minor = version.split('.').take(2).collect::<Vec<_>>().join(".");
            PhpRelease {
                installed: installed.iter().any(|i| i == &version),
                latest: seen.insert(minor.clone()),
                minor,
                size: String::new(),
                version,
            }
        })
        .collect()
}

#[cfg(windows)]
fn catalog_win(base: &Path) -> Result<Vec<PhpRelease>, String> {
    let body = download::get_string(&format!("{WIN_ARCHIVES}/"))
        .map_err(|e| format!("Não foi possível consultar as versões do PHP (sem internet?): {e}"))?;
    let installed: Vec<String> = fs::read_dir(php_root(base))
        .map(|rd| {
            rd.flatten()
                .filter(|e| crate::platform::bin(&e.path(), "php-fpm").is_file())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    Ok(parse_win_catalog(&body, &installed))
}

#[cfg(windows)]
fn install_win(base: &Path, version: &str, progress: &dyn Fn(InstallProgress)) -> Result<(), String> {
    let final_dir = install_dir(base, version);
    if crate::platform::bin(&final_dir, "php-fpm").is_file() {
        return Ok(());
    }
    let urls = win_urls(version);
    if urls.is_empty() {
        return Err(format!("Não há PHP oficial para Windows na série de {version} (só x64, do 8.0 ao 8.5)."));
    }
    let root = php_root(base);
    let tmp_dir = root.join(format!(".tmp-{version}"));
    let archive = root.join(format!(".download-{version}.zip"));
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let emit = |stage: &str, downloaded: u64, total: u64| {
        progress(InstallProgress { id: version.to_string(), stage: stage.to_string(), downloaded, total, message: None });
    };
    let result = (|| -> Result<(), String> {
        let mut last_err = String::new();
        let mut ok = false;
        for url in &urls {
            match download::download_to(url, &archive, &mut |d, t| emit("download", d, t)) {
                Ok(()) => { ok = true; break; }
                Err(e) if e.contains("404") => last_err = e,
                Err(e) => return Err(format!("PHP {version}: {e}")),
            }
        }
        if !ok {
            return Err(format!("PHP {version}: {last_err}"));
        }
        emit("extract", 0, 0);
        fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
        download::unpack_zip(&archive, &tmp_dir)?;
        let cgi = crate::platform::bin(&tmp_dir, "php-cgi");
        if !cgi.is_file() || !crate::platform::bin(&tmp_dir, "php").is_file() {
            return Err("o pacote baixado não contém o php-cgi.exe".into());
        }
        // O resto do app procura `php-fpm(.exe)`: é o mesmo binário CGI com outro nome.
        fs::copy(&cgi, crate::platform::bin(&tmp_dir, "php-fpm")).map_err(|e| e.to_string())?;
        emit("verify", 0, 0);
        let out = Command::new(&cgi).arg("-v").output().map_err(|e| format!("o binário baixado não executa: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout);
        if !text.contains(&format!("PHP {version}")) {
            return Err(format!("verificação falhou: {}", text.lines().next().unwrap_or("sem saída")));
        }
        let _ = fs::remove_dir_all(&final_dir);
        fs::rename(&tmp_dir, &final_dir).map_err(|e| e.to_string())?;
        Ok(())
    })();
    let _ = fs::remove_file(&archive);
    if result.is_err() {
        let _ = fs::remove_dir_all(&tmp_dir);
    }
    result
}

/// Consulta o servidor e devolve todas as versões disponíveis para esta máquina.
#[allow(unreachable_code)]
pub fn catalog(base: &Path) -> Result<Vec<PhpRelease>, String> {
    #[cfg(windows)]
    {
        return catalog_win(base);
    }
    let platform = platform().ok_or("Esta plataforma ainda não é suportada para download do PHP.")?;
    let suffix = format!("-fpm-{platform}.tar.gz");

    let body = download::get_string(&format!("{BASE_URL}/?format=json"))
        .map_err(|e| format!("Não foi possível consultar as versões do PHP (sem internet?): {e}"))?;
    let listing: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;

    let installed: Vec<String> = fs::read_dir(php_root(base))
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().join("php-fpm").is_file())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();

    let mut releases: Vec<PhpRelease> = listing
        .as_array()
        .ok_or("resposta inesperada do servidor")?
        .iter()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?;
            let version = name.strip_prefix("php-")?.strip_suffix(suffix.as_str())?;
            if !valid_version(version) {
                return None;
            }
            let minor = version.split('.').take(2).collect::<Vec<_>>().join(".");
            Some(PhpRelease {
                version: version.to_string(),
                minor,
                size: item.get("size").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                installed: installed.iter().any(|i| i == version),
                latest: false,
            })
        })
        .collect();

    releases.sort_by(|a, b| version_key(&b.version).cmp(&version_key(&a.version)));
    let mut seen = std::collections::HashSet::new();
    for r in releases.iter_mut() {
        r.latest = seen.insert(r.minor.clone());
    }
    Ok(releases)
}

/// Caminho do PHP de linha de comando (`php`) de uma versão gerenciada, se instalado.
pub fn cli_bin(base: &Path, version: &str) -> Option<PathBuf> {
    let p = crate::platform::bin(&install_dir(base, version), "php");
    p.is_file().then_some(p)
}

/// Extrai um pacote do static-php-cli em `dest`, deixa o binário executável e confere
/// que ele roda e reporta a versão pedida.
fn unpack_binary(archive: &Path, dest: &Path, bin_name: &str, expect: &str) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let file = File::open(archive).map_err(|e| e.to_string())?;
    tar::Archive::new(flate2::read::GzDecoder::new(file))
        .unpack(dest)
        .map_err(|e| format!("falha ao extrair: {e}"))?;
    let bin = dest.join(bin_name);
    if !bin.is_file() {
        return Err(format!("o pacote baixado não contém o {bin_name}"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    let out = Command::new(&bin)
        .arg("-v")
        .output()
        .map_err(|e| format!("o binário baixado não executa: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    if !text.contains(expect) {
        return Err(format!("verificação falhou: {}", text.lines().next().unwrap_or("sem saída")));
    }
    Ok(())
}

/// Baixa, extrai e valida uma versão (PHP-FPM e PHP CLI). Bloqueante; `progress` é
/// chamado durante o processo. O CLI é opcional: se não existir para esta versão,
/// o PHP-FPM ainda é instalado.
pub fn install(
    base: &Path,
    version: &str,
    progress: &dyn Fn(InstallProgress),
) -> Result<(), String> {
    if !valid_version(version) {
        return Err("versão inválida".into());
    }
    #[cfg(windows)]
    {
        return install_win(base, version, progress);
    }
    let platform = platform().ok_or("Plataforma não suportada.")?;
    let final_dir = install_dir(base, version);
    if crate::platform::bin(&final_dir, "php-fpm").is_file() {
        return Ok(());
    }

    let root = php_root(base);
    let tmp_dir = root.join(format!(".tmp-{version}"));
    let fpm_archive = root.join(format!(".download-{version}-fpm.tar.gz"));
    let cli_archive = root.join(format!(".download-{version}-cli.tar.gz"));
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;

    let emit = |stage: &str, downloaded: u64, total: u64| {
        progress(InstallProgress {
            id: version.to_string(),
            stage: stage.to_string(),
            downloaded,
            total,
            message: None,
        })
    };

    let result = (|| -> Result<(), String> {
        // 1) Downloads. O progresso soma os dois arquivos (têm tamanho parecido).
        let url = |flavor: &str| format!("{BASE_URL}/php-{version}-{flavor}-{platform}.tar.gz");
        let mut fpm_total = 0u64;
        download::download_to(&url("fpm"), &fpm_archive, &mut |d, t| {
            fpm_total = t;
            emit("download", d, t * 2)
        })
        .map_err(|e| format!("PHP {version}: {e}"))?;
        let cli_ok = download::download_to(&url("cli"), &cli_archive, &mut |d, t| {
            emit("download", fpm_total + d, fpm_total + t)
        })
        .is_ok();

        // 2) Extração e verificação: os binários precisam rodar e reportar a versão pedida.
        emit("extract", 0, 0);
        unpack_binary(&fpm_archive, &tmp_dir, "php-fpm", &format!("PHP {version}"))?;
        emit("verify", 0, 0);
        if cli_ok {
            // Sem o CLI o PHP-FPM continua válido; `ensure_cli` tenta de novo quando precisar.
            if unpack_binary(&cli_archive, &tmp_dir, "php", &format!("PHP {version}")).is_err() {
                let _ = fs::remove_file(tmp_dir.join("php"));
            }
        }

        // 3) Publica de forma atômica.
        let _ = fs::remove_dir_all(&final_dir);
        fs::rename(&tmp_dir, &final_dir).map_err(|e| e.to_string())?;
        Ok(())
    })();

    let _ = fs::remove_file(&fpm_archive);
    let _ = fs::remove_file(&cli_archive);
    if result.is_err() {
        let _ = fs::remove_dir_all(&tmp_dir);
    }
    result
}

/// Garante o PHP CLI de uma versão instalada pelo app (versões instaladas antes de o
/// app baixar o CLI não o têm). Bloqueante.
#[allow(unreachable_code)]
pub fn ensure_cli(base: &Path, version: &str, progress: &dyn Fn(InstallProgress)) -> Result<PathBuf, String> {
    if !valid_version(version) {
        return Err("versão inválida".into());
    }
    if let Some(p) = cli_bin(base, version) {
        return Ok(p);
    }
    let dir = install_dir(base, version);
    if !crate::platform::bin(&dir, "php-fpm").is_file() {
        return Err(format!("o PHP {version} não está instalado"));
    }
    #[cfg(windows)]
    {
        return Err("o pacote do PHP para Windows não trouxe o php.exe".into());
    }
    let platform = platform().ok_or("Plataforma não suportada.")?;
    let archive = php_root(base).join(format!(".download-{version}-cli.tar.gz"));
    let tmp = php_root(base).join(format!(".tmp-{version}-cli"));
    let _ = fs::remove_dir_all(&tmp);

    let emit = |stage: &str, downloaded: u64, total: u64| {
        progress(InstallProgress {
            id: version.to_string(),
            stage: stage.to_string(),
            downloaded,
            total,
            message: Some("Baixando o PHP CLI…".into()),
        })
    };
    let result = (|| -> Result<(), String> {
        let url = format!("{BASE_URL}/php-{version}-cli-{platform}.tar.gz");
        download::download_to(&url, &archive, &mut |d, t| emit("download", d, t))
            .map_err(|e| format!("PHP CLI {version}: {e}"))?;
        emit("extract", 0, 0);
        unpack_binary(&archive, &tmp, "php", &format!("PHP {version}"))?;
        fs::rename(tmp.join("php"), dir.join("php")).map_err(|e| e.to_string())
    })();
    let _ = fs::remove_file(&archive);
    let _ = fs::remove_dir_all(&tmp);
    result?;
    cli_bin(base, version).ok_or_else(|| "o PHP CLI não foi instalado".into())
}

pub fn uninstall(base: &Path, version: &str) -> Result<(), String> {
    if !valid_version(version) {
        return Err("versão inválida".into());
    }
    let dir = install_dir(base, version);
    if !dir.exists() {
        return Ok(());
    }
    fs::remove_dir_all(&dir).map_err(|e| e.to_string())
}

/// Versões gerenciadas pelo app (instaladas em `bin/php`).
pub fn managed_versions(base: &Path) -> Vec<String> {
    tools::php_installs(base)
        .into_iter()
        .filter(|i| i.managed)
        .map(|i| i.full_version)
        .collect()
}

// ------------------------------------------------------------------ PHP com PostgreSQL

/// PHP-FPM do pacote "common" de uma versão gerenciada, se já foi baixado.
pub fn pg_fpm(base: &Path, version: &str) -> Option<PathBuf> {
    let p = install_dir(base, version).join("php-fpm-pg");
    p.is_file().then_some(p)
}

/// PHP CLI do pacote "common" de uma versão gerenciada, se já foi baixado.
pub fn pg_cli(base: &Path, version: &str) -> Option<PathBuf> {
    let p = install_dir(base, version).join("php-pg");
    p.is_file().then_some(p)
}

/// Patch do pacote "common" a usar para a série de `full_version` (`8.4.8` -> série `8.4`):
/// o mesmo patch se existir, senão o mais novo da série.
fn pick_common_version(listing: &serde_json::Value, full_version: &str, platform: &str) -> Option<String> {
    let minor = full_version.split('.').take(2).collect::<Vec<_>>().join(".");
    let suffix = format!("-fpm-{platform}.tar.gz");
    let mut found: Vec<String> = listing
        .as_array()?
        .iter()
        .filter_map(|item| {
            let name = item.get("name")?.as_str()?;
            let v = name.strip_prefix("php-")?.strip_suffix(suffix.as_str())?;
            (valid_version(v) && v.split('.').take(2).collect::<Vec<_>>().join(".") == minor).then(|| v.to_string())
        })
        .collect();
    if found.iter().any(|v| v == full_version) {
        return Some(full_version.to_string());
    }
    found.sort_by_key(|v| version_key(v));
    found.pop()
}

/// Garante o PHP com `pdo_pgsql` para uma versão instalada pelo app. Baixa o PHP-FPM e o CLI
/// do pacote "common" na primeira vez. Bloqueante.
#[allow(unreachable_code)]
pub fn ensure_pg_flavor(base: &Path, full_version: &str, progress: &dyn Fn(InstallProgress)) -> Result<(), String> {
    if !valid_version(full_version) {
        return Err("versão inválida".into());
    }
    // No Windows o pacote oficial já traz as extensões do PostgreSQL: nada a baixar.
    #[cfg(windows)]
    {
        let _ = (base, progress);
        return Ok(());
    }
    if pg_fpm(base, full_version).is_some() && pg_cli(base, full_version).is_some() {
        return Ok(());
    }
    let dir = install_dir(base, full_version);
    if !dir.join("php-fpm").is_file() {
        return Err(format!("o PHP {full_version} não está instalado"));
    }
    let platform = platform().ok_or("Plataforma não suportada.")?;
    let listing: serde_json::Value = serde_json::from_str(
        &download::get_string(&format!("{COMMON_URL}/?format=json"))
            .map_err(|e| format!("Não foi possível consultar o PHP com suporte a PostgreSQL (sem internet?): {e}"))?,
    )
    .map_err(|e| e.to_string())?;
    let common = pick_common_version(&listing, full_version, platform)
        .ok_or_else(|| format!("Não há PHP com suporte a PostgreSQL para a série {full_version}."))?;
    let minor = common.split('.').take(2).collect::<Vec<_>>().join(".");
    let expect = format!("PHP {minor}.");

    let root = php_root(base);
    let tmp = root.join(format!(".tmp-{full_version}-pg"));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;

    let emit = |stage: &str, downloaded: u64, total: u64| {
        progress(InstallProgress {
            id: format!("php-pg-{full_version}"),
            stage: stage.to_string(),
            downloaded,
            total,
            message: Some("Baixando o PHP com suporte a PostgreSQL…".into()),
        })
    };
    let result = (|| -> Result<(), String> {
        let mut total_fpm = 0u64;
        for (kind, bin, target) in [("fpm", "php-fpm", "php-fpm-pg"), ("cli", "php", "php-pg")] {
            let archive = tmp.join(format!("{kind}.tar.gz"));
            let url = format!("{COMMON_URL}/php-{common}-{kind}-{platform}.tar.gz");
            let offset = total_fpm;
            download::download_to(&url, &archive, &mut |d, t| {
                if kind == "fpm" {
                    total_fpm = t;
                }
                emit("download", offset + d, if kind == "fpm" { t * 2 } else { offset + t })
            })
            .map_err(|e| format!("PHP {common} (PostgreSQL): {e}"))?;
            emit("extract", 0, 0);
            let out = tmp.join(kind);
            unpack_binary(&archive, &out, bin, &expect)?;
            fs::rename(out.join(bin), tmp.join(target)).map_err(|e| e.to_string())?;
        }
        for target in ["php-fpm-pg", "php-pg"] {
            let _ = fs::remove_file(dir.join(target));
            fs::rename(tmp.join(target), dir.join(target)).map_err(|e| e.to_string())?;
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&tmp);
    result
}

/// PHP-FPM a usar num projeto: o do pacote "common" quando o banco é PostgreSQL (baixado
/// aqui se faltar), senão o normal. PHP do sistema (Homebrew) já vem com `pdo_pgsql`.
#[allow(unreachable_code)]
pub fn fpm_for(base: &Path, php: &tools::PhpInstall, postgres: bool) -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        let _ = (base, postgres);
        return Ok(php.fpm.clone());
    }
    if !postgres || !php.managed {
        return Ok(php.fpm.clone());
    }
    ensure_pg_flavor(base, &php.full_version, &|_| {})?;
    pg_fpm(base, &php.full_version).ok_or_else(|| "o PHP com suporte a PostgreSQL não foi instalado".into())
}

/// PHP CLI de um projeto, com a mesma regra de `fpm_for`, sem baixar nada: se o pacote
/// "common" ainda não existe usa o CLI normal.
pub fn cli_for(base: &Path, php: &tools::PhpInstall, postgres: bool) -> Option<PathBuf> {
    if postgres && php.managed {
        if let Some(p) = pg_cli(base, &php.full_version) {
            return Some(p);
        }
    }
    php.cli()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escolhe_o_patch_do_pacote_common() {
        let listing = serde_json::json!([
            {"name": "php-8.4.8-fpm-macos-aarch64.tar.gz"},
            {"name": "php-8.4.23-fpm-macos-aarch64.tar.gz"},
            {"name": "php-8.4.23-cli-macos-aarch64.tar.gz"},
            {"name": "php-8.3.2-fpm-macos-aarch64.tar.gz"},
            {"name": "php-8.3.30-fpm-macos-aarch64.tar.gz"},
            {"name": "php-8.4.99-fpm-linux-x86_64.tar.gz"},
        ]);
        let p = "macos-aarch64";
        assert_eq!(pick_common_version(&listing, "8.4.8", p).as_deref(), Some("8.4.8"), "mesmo patch");
        assert_eq!(pick_common_version(&listing, "8.4.5", p).as_deref(), Some("8.4.23"), "mais novo da série (23 > 8)");
        assert_eq!(pick_common_version(&listing, "8.3.32", p).as_deref(), Some("8.3.30"));
        assert_eq!(pick_common_version(&listing, "8.5.1", p), None);
    }

    #[cfg(windows)]
    #[test]
    fn catalogo_do_windows_le_a_listagem() {
        let html = r#"<a href="php-8.4.13-nts-Win32-vs17-x64.zip">x</a>
        <a href="php-8.4.12-nts-Win32-vs17-x64.zip">x</a>
        <a href="php-8.3.30-nts-Win32-vs16-x64.zip">x</a>
        <a href="php-8.4.13-ts-Win32-vs17-x64.zip">x</a>
        <a href="php-7.4.33-nts-Win32-vc15-x64.zip">x</a>
        <a href="php-8.4.13-nts-Win32-vs17-x86.zip">x</a>"#;
        let cat = parse_win_catalog(html, &["8.3.30".to_string()]);
        let versions: Vec<&str> = cat.iter().map(|r| r.version.as_str()).collect();
        assert_eq!(versions, ["8.4.13", "8.4.12", "8.3.30"]);
        assert!(cat[0].latest && !cat[1].latest && cat[2].installed);
    }

    #[test]
    fn php_do_sistema_nao_baixa_nada() {
        let base = std::env::temp_dir().join("localtusk-php-nodl");
        let sys = tools::PhpInstall {
            full_version: "8.4.1".into(),
            version: "8.4".into(),
            fpm: PathBuf::from("/opt/homebrew/sbin/php-fpm"),
            managed: false,
        };
        assert_eq!(fpm_for(&base, &sys, true).unwrap(), sys.fpm);
        assert_eq!(fpm_for(&base, &sys, false).unwrap(), sys.fpm);
    }
}
