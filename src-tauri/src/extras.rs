//! Ferramentas extras gerenciadas pelo app, instaladas em `~/LocalTusk/bin/tools/<id>/`:
//!
//! - **Caddy**: servidor web de cada projeto (essencial)
//! - **Adminer Neo**: administração do banco (arquivo PHP servido pelo Caddy)
//! - **Mailpit**: captura os e-mails enviados pelo PHP, para testes
//! - **WP-CLI**: o comando `wp` (WordPress), um `.phar` executado pelo PHP CLI do projeto

use crate::download::{self, InstallProgress};
use serde::Serialize;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const IDS: &[&str] = &["caddy", "adminneo", "mailpit", "wpcli"];

pub fn tools_root(base: &Path) -> PathBuf {
    base.join("bin").join("tools")
}

pub fn dir(base: &Path, id: &str) -> PathBuf {
    tools_root(base).join(id)
}

/// Arquivo cuja existência indica que a ferramenta está instalada.
fn marker(base: &Path, id: &str) -> PathBuf {
    match id {
        "mailpit" => crate::platform::bin(&dir(base, id), "mailpit"),
        "caddy" => crate::platform::bin(&dir(base, id), "caddy"),
        "wpcli" => dir(base, id).join("wp-cli.phar"),
        _ => dir(base, id).join("index.php"),
    }
}

pub fn is_installed(base: &Path, id: &str) -> bool {
    IDS.contains(&id) && marker(base, id).is_file()
}

/// Caddy instalado pelo app (se houver).
pub fn caddy_bin(base: &Path) -> Option<PathBuf> {
    let p = marker(base, "caddy");
    p.is_file().then_some(p)
}

/// WP-CLI instalado pelo app (se houver).
pub fn wpcli_phar(base: &Path) -> Option<PathBuf> {
    let p = marker(base, "wpcli");
    p.is_file().then_some(p)
}

pub fn mailpit_bin(base: &Path) -> Option<PathBuf> {
    let p = marker(base, "mailpit");
    p.is_file().then_some(p)
}

fn version_of(base: &Path, id: &str) -> Option<String> {
    fs::read_to_string(dir(base, id).join(".version"))
        .ok()
        .map(|s| s.trim().to_string())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtraStatus {
    pub id: String,
    pub label: String,
    pub description: String,
    /// Necessária para os projetos funcionarem (aparece separada na interface).
    pub essential: bool,
    pub installed: bool,
    pub version: Option<String>,
}

pub fn status(base: &Path) -> Vec<ExtraStatus> {
    IDS.iter()
        .map(|id| {
            let (label, description) = match *id {
                "caddy" => (
                    "Caddy",
                    "Servidor web de cada projeto. Obrigatório.",
                ),
                "adminneo" => (
                    "Adminer Neo",
                    "Administração do MySQL e do PostgreSQL pelo navegador, leve e moderna, com login automático.",
                ),
                "wpcli" => (
                    "WP-CLI",
                    "A linha de comando do WordPress (comando wp). Necessária para o botão \"Instalar WordPress\".",
                ),
                _ => (
                    "Mailpit",
                    "Caixa de e-mails de teste: tudo que o PHP enviar com mail() ou SMTP cai aqui.",
                ),
            };
            ExtraStatus {
                id: id.to_string(),
                label: label.into(),
                description: description.into(),
                essential: *id == "caddy",
                installed: is_installed(base, id),
                version: if is_installed(base, id) { version_of(base, id) } else { None },
            }
        })
        .collect()
}

pub fn uninstall(base: &Path, id: &str) -> Result<(), String> {
    if !IDS.contains(&id) {
        return Err("ferramenta desconhecida".into());
    }
    let d = dir(base, id);
    if d.exists() {
        fs::remove_dir_all(&d).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Instala (ou reinstala) uma ferramenta. Bloqueante.
pub fn install(base: &Path, id: &str, progress: &dyn Fn(InstallProgress)) -> Result<(), String> {
    if !IDS.contains(&id) {
        return Err("ferramenta desconhecida".into());
    }
    let root = tools_root(base);
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let tmp = root.join(format!(".tmp-{id}"));
    let archive = root.join(format!(".download-{id}"));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;

    let emit = |stage: &str, downloaded: u64, total: u64| {
        progress(InstallProgress {
            id: id.to_string(),
            stage: stage.to_string(),
            downloaded,
            total,
            message: None,
        })
    };

    let result = (|| -> Result<String, String> {
        let version = match id {
            "caddy" => install_caddy(&tmp, &archive, &emit)?,
            "adminneo" => install_adminneo(&tmp, &archive, &emit)?,
            "wpcli" => install_wpcli(&tmp, &archive, &emit)?,
            _ => install_mailpit(&tmp, &archive, &emit)?,
        };
        fs::write(tmp.join(".version"), &version).map_err(|e| e.to_string())?;

        let final_dir = dir(base, id);
        let _ = fs::remove_dir_all(&final_dir);
        fs::rename(&tmp, &final_dir).map_err(|e| e.to_string())?;
        Ok(version)
    })();

    let _ = fs::remove_file(&archive);
    if result.is_err() {
        let _ = fs::remove_dir_all(&tmp);
    }
    result.map(|_| ())
}

type Emit<'a> = &'a dyn Fn(&str, u64, u64);

// ---------------------------------------------------------------- Adminer Neo

const ADMINNEO_CONFIG: &str = r#"<?php
// Gerado pelo LocalTusk. Os dados vêm do ambiente do PHP-FPM de cada projeto.
return [
    "colorVariant" => "purple",
    "navigationMode" => "dual",
    "preferSelection" => true,
    "versionVerification" => false,
    "defaultDriver" => getenv('DB_ENGINE') === 'postgres' ? "pgsql" : "mysql",
    "defaultServer" => (getenv('DB_HOST') ?: '127.0.0.1') . ':' . (getenv('DB_PORT') ?: (getenv('DB_ENGINE') === 'postgres' ? '5432' : '3306')),
    "defaultDatabase" => getenv('DB_NAME') ?: null,
];
"#;

/// Página de entrada com login automático: abre a sessão (cookie) e envia o formulário
/// de login do Adminer Neo com as credenciais do projeto.
const ADMINNEO_LOGIN: &str = r#"<?php
$h = fn($v) => htmlspecialchars((string) $v, ENT_QUOTES);
$pg = getenv('DB_ENGINE') === 'postgres';
$entry = $pg ? 'pg.php' : 'index.php';
$server = (getenv('DB_HOST') ?: '127.0.0.1') . ':' . (getenv('DB_PORT') ?: ($pg ? '5432' : '3306'));
?><!doctype html>
<html lang="pt-br"><head><meta charset="utf-8"><title>Abrindo o Adminer Neo…</title>
<style>body{font:15px system-ui;background:#232531;color:#eceef7;display:grid;place-items:center;height:100vh;margin:0}</style></head>
<body>
<form id="f" method="post" action="<?= $entry ?>">
  <input type="hidden" name="auth[driver]" value="<?= $pg ? 'pgsql' : 'mysql' ?>">
  <input type="hidden" name="auth[server]" value="<?= $h($server) ?>">
  <input type="hidden" name="auth[username]" value="<?= $h(getenv('DB_USER') ?: 'root') ?>">
  <input type="hidden" name="auth[password]" value="<?= $h(getenv('DB_PASSWORD')) ?>">
  <input type="hidden" name="auth[db]" value="<?= $h(getenv('DB_NAME')) ?>">
  <noscript><button>Entrar no Adminer Neo</button></noscript>
</form>
<p>Abrindo o Adminer Neo…</p>
<script>
  // O primeiro acesso cria o cookie de sessão; depois o login é enviado.
  fetch(<?= json_encode($entry) ?>, { credentials: 'same-origin' })
    .catch(() => {})
    .then(() => document.getElementById('f').submit());
</script>
</body></html>
"#;

/// O Adminer Neo é distribuído em um arquivo por driver (todos os idiomas, tema padrão com todas as
/// cores): `index.php` é o do MySQL e `pg.php` o do PostgreSQL.
fn adminneo_url(driver: &str) -> String {
    format!("https://www.adminneo.org/files/latest/{driver}__default/adminneo-latest.php")
}

fn check_php_file(path: &Path) -> Result<(), String> {
    let mut head = [0u8; 5];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .map_err(|e| e.to_string())?;
    if &head != b"<?php" {
        return Err("o arquivo baixado do Adminer Neo não parece ser PHP".into());
    }
    Ok(())
}

/// Reescreve a configuração e a página de login do Adminer Neo já instalado. Chamado a cada
/// início de projeto, assim instalações antigas passam a entender o PostgreSQL sem reinstalar.
pub fn refresh_adminneo_files(base: &Path) {
    let d = dir(base, "adminneo");
    if d.join("index.php").is_file() {
        let _ = fs::write(d.join("adminneo-config.php"), ADMINNEO_CONFIG);
        let _ = fs::write(d.join("login.php"), ADMINNEO_LOGIN);
    }
}

/// O Adminer Neo instalado tem o driver do PostgreSQL?
pub fn adminneo_has_pg(base: &Path) -> bool {
    dir(base, "adminneo").join("pg.php").is_file()
}

/// Baixa só o driver do PostgreSQL para um Adminer Neo instalado antes dele existir.
pub fn add_adminneo_pg(base: &Path) -> Result<(), String> {
    let d = dir(base, "adminneo");
    if !d.join("index.php").is_file() {
        return Err("o Adminer Neo não está instalado".into());
    }
    let tmp = d.join(".pg.php.download");
    let result = (|| -> Result<(), String> {
        download::download_to(&adminneo_url("pgsql"), &tmp, &mut |_, _| {}).map_err(|e| format!("Adminer Neo (PostgreSQL): {e}"))?;
        check_php_file(&tmp)?;
        fs::rename(&tmp, d.join("pg.php")).map_err(|e| e.to_string())
    })();
    let _ = fs::remove_file(&tmp);
    result
}

fn install_adminneo(tmp: &Path, archive: &Path, emit: Emit) -> Result<String, String> {
    download::download_to(&adminneo_url("mysql"), archive, &mut |d, t| emit("download", d, t))
        .map_err(|e| format!("Adminer Neo: {e}"))?;

    emit("verify", 0, 0);
    check_php_file(archive)?;

    emit("extract", 0, 0);
    fs::copy(archive, tmp.join("index.php")).map_err(|e| e.to_string())?;
    // Driver do PostgreSQL: se falhar, o do MySQL continua valendo (`add_adminneo_pg` tenta depois).
    let pg = tmp.join("pg.php");
    if download::download_to(&adminneo_url("pgsql"), &pg, &mut |_, _| {}).is_err() || check_php_file(&pg).is_err() {
        let _ = fs::remove_file(&pg);
    }
    fs::write(tmp.join("adminneo-config.php"), ADMINNEO_CONFIG).map_err(|e| e.to_string())?;
    fs::write(tmp.join("login.php"), ADMINNEO_LOGIN).map_err(|e| e.to_string())?;

    // Versão: o GitHub do projeto publica a mais recente.
    let version = download::get_string("https://api.github.com/repos/adminneo-org/adminneo/releases/latest")
        .ok()
        .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).ok())
        .and_then(|j| j.get("tag_name")?.as_str().map(|t| t.trim_start_matches('v').to_string()))
        .unwrap_or_else(|| "latest".into());
    Ok(version)
}

// ---------------------------------------------------------------- Mailpit

fn mailpit_asset() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("mailpit-darwin-arm64.tar.gz"),
        ("macos", "x86_64") => Some("mailpit-darwin-amd64.tar.gz"),
        ("linux", "aarch64") => Some("mailpit-linux-arm64.tar.gz"),
        ("linux", "x86_64") => Some("mailpit-linux-amd64.tar.gz"),
        ("windows", "x86_64") => Some("mailpit-windows-amd64.zip"),
        _ => None,
    }
}

fn install_mailpit(tmp: &Path, archive: &Path, emit: Emit) -> Result<String, String> {
    let asset = mailpit_asset().ok_or("Mailpit ainda não é suportado nesta plataforma.")?;
    let rel = download::get_string("https://api.github.com/repos/axllent/mailpit/releases/latest")?;
    let tag = serde_json::from_str::<serde_json::Value>(&rel)
        .ok()
        .and_then(|j| j.get("tag_name")?.as_str().map(String::from))
        .filter(|t| t.starts_with('v') && t[1..].chars().all(|c| c.is_ascii_digit() || c == '.'))
        .ok_or("não foi possível descobrir a versão atual do Mailpit")?;

    let url = format!("https://github.com/axllent/mailpit/releases/download/{tag}/{asset}");
    download::download_to(&url, archive, &mut |d, t| emit("download", d, t))
        .map_err(|e| format!("Mailpit: {e}"))?;

    emit("extract", 0, 0);
    unpack_tool(archive, tmp, asset)?;
    let bin = crate::platform::bin(tmp, "mailpit");
    if !bin.is_file() {
        return Err("o pacote do Mailpit não contém o executável".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }

    emit("verify", 0, 0);
    let out = Command::new(&bin)
        .arg("version")
        .output()
        .map_err(|e| format!("o Mailpit baixado não executa: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let version = tag.trim_start_matches('v').to_string();
    if !text.contains(&version) {
        return Err(format!("verificação falhou: {}", text.lines().next().unwrap_or("sem saída")));
    }
    Ok(version)
}

// ---------------------------------------------------------------- Caddy

fn caddy_platform() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("mac_arm64"),
        ("macos", "x86_64") => Some("mac_amd64"),
        ("linux", "aarch64") => Some("linux_arm64"),
        ("linux", "x86_64") => Some("linux_amd64"),
        ("windows", "x86_64") => Some("windows_amd64"),
        _ => None,
    }
}

/// Extrai o pacote de uma ferramenta: `.zip` no Windows, `.tar.gz` nos outros sistemas.
fn unpack_tool(archive: &Path, tmp: &Path, asset: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        let _ = asset;
        return crate::download::unpack_zip(archive, tmp);
    }
    #[cfg(not(windows))]
    {
        let _ = asset;
        let file = File::open(archive).map_err(|e| e.to_string())?;
        tar::Archive::new(flate2::read::GzDecoder::new(file))
            .unpack(tmp)
            .map_err(|e| format!("falha ao extrair: {e}"))
    }
}

fn install_caddy(tmp: &Path, archive: &Path, emit: Emit) -> Result<String, String> {
    let platform = caddy_platform().ok_or("Caddy ainda não é suportado nesta plataforma.")?;
    let rel = download::get_string("https://api.github.com/repos/caddyserver/caddy/releases/latest")?;
    let tag = serde_json::from_str::<serde_json::Value>(&rel)
        .ok()
        .and_then(|j| j.get("tag_name")?.as_str().map(String::from))
        .filter(|t| t.starts_with('v') && t[1..].chars().all(|c| c.is_ascii_digit() || c == '.'))
        .ok_or("não foi possível descobrir a versão atual do Caddy")?;
    let version = tag.trim_start_matches('v').to_string();

    let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
    let file_name = format!("caddy_{version}_{platform}.{ext}");
    let base_url = format!("https://github.com/caddyserver/caddy/releases/download/{tag}");
    download::download_to(&format!("{base_url}/{file_name}"), archive, &mut |d, t| emit("download", d, t))
        .map_err(|e| format!("Caddy: {e}"))?;

    // O projeto publica o SHA-512 de cada arquivo.
    emit("verify", 0, 0);
    let sums = download::get_string(&format!("{base_url}/caddy_{version}_checksums.txt"))?;
    let expected = sums
        .lines()
        .find(|l| l.split_whitespace().nth(1) == Some(file_name.as_str()))
        .and_then(|l| l.split_whitespace().next())
        .unwrap_or("")
        .to_lowercase();
    if expected.len() != 128 || download::sha512_hex(archive)? != expected {
        return Err("verificação de integridade (SHA-512) do Caddy falhou".into());
    }

    emit("extract", 0, 0);
    unpack_tool(archive, tmp, &file_name)?;
    let bin = crate::platform::bin(tmp, "caddy");
    if !bin.is_file() {
        return Err("o pacote do Caddy não contém o executável".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    let out = Command::new(&bin)
        .arg("version")
        .output()
        .map_err(|e| format!("o Caddy baixado não executa: {e}"))?;
    if !String::from_utf8_lossy(&out.stdout).contains(&version) {
        return Err("verificação falhou: versão inesperada do Caddy".into());
    }
    Ok(version)
}

// ---------------------------------------------------------------- WP-CLI

fn install_wpcli(tmp: &Path, archive: &Path, emit: Emit) -> Result<String, String> {
    let rel = download::get_string("https://api.github.com/repos/wp-cli/wp-cli/releases/latest")?;
    let tag = serde_json::from_str::<serde_json::Value>(&rel)
        .ok()
        .and_then(|j| j.get("tag_name")?.as_str().map(String::from))
        .filter(|t| t.starts_with('v') && t[1..].chars().all(|c| c.is_ascii_digit() || c == '.'))
        .ok_or("não foi possível descobrir a versão atual do WP-CLI")?;
    let version = tag.trim_start_matches('v').to_string();

    let name = format!("wp-cli-{version}.phar");
    let base_url = format!("https://github.com/wp-cli/wp-cli/releases/download/{tag}");
    download::download_to(&format!("{base_url}/{name}"), archive, &mut |d, t| emit("download", d, t))
        .map_err(|e| format!("WP-CLI: {e}"))?;

    // O projeto publica o SHA-512 de cada arquivo.
    emit("verify", 0, 0);
    let expected = download::get_string(&format!("{base_url}/{name}.sha512"))?
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase();
    if expected.len() != 128 || download::sha512_hex(archive)? != expected {
        return Err("verificação de integridade (SHA-512) do WP-CLI falhou".into());
    }

    emit("extract", 0, 0);
    let phar = tmp.join("wp-cli.phar");
    fs::copy(archive, &phar).map_err(|e| e.to_string())?;
    let mut head = [0u8; 2];
    File::open(&phar)
        .and_then(|mut f| f.read_exact(&mut head))
        .map_err(|e| e.to_string())?;
    if &head != b"#!" {
        return Err("o arquivo baixado do WP-CLI não parece ser o wp-cli.phar".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&phar, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    Ok(version)
}
