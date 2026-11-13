//! Gerenciador de versões do MySQL.
//!
//! Baixa os tarballs oficiais do MySQL Community Server (macOS), do CDN da Oracle, e guarda
//! só o necessário em `~/LocalTusk/bin/mysql/<versão>/` (`bin/mysqld`, `lib/`, `share/`).
//!
//! O catálogo de versões é uma lista **embutida** no app (`resources/mysql-versions.json`),
//! gerada por `scripts/update_mysql_versions.py`. O app não sonda servidores em tempo de
//! execução: o CDN da Oracle bloqueia rajadas de requisições, e assim o catálogo é instantâneo
//! e funciona offline.

use crate::download::{self, InstallProgress};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const MANIFEST: &str = include_str!("../resources/mysql-versions.json");
const CDN: &str = "https://cdn.mysql.com";

pub fn mysql_root(base: &Path) -> PathBuf {
    base.join("bin").join("mysql")
}

pub fn install_dir(base: &Path, version: &str) -> PathBuf {
    mysql_root(base).join(version)
}

/// Aceita só `X.Y.Z` numérico (impede path traversal).
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

fn series_of(version: &str) -> String {
    version.split('.').take(2).collect::<Vec<_>>().join(".")
}

/// Chave da plataforma no catálogo: `arm64`/`x86_64` no macOS, `linux-x86_64`/`linux-aarch64` no Linux.
fn arch() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("arm64"),
        ("macos", "x86_64") => Some("x86_64"),
        ("linux", "aarch64") => Some("linux-aarch64"),
        ("linux", "x86_64") => Some("linux-x86_64"),
        ("windows", "x86_64") => Some("win-x86_64"),
        _ => None,
    }
}

const UNSUPPORTED: &str = "O download do MySQL só está disponível para macOS, Linux e Windows (x64) por enquanto.";

#[derive(Deserialize)]
struct Manifest {
    versions: Vec<ManifestVersion>,
}

#[derive(Deserialize)]
struct ManifestVersion {
    version: String,
    builds: HashMap<String, Build>,
}

#[derive(Deserialize, Clone)]
struct Build {
    /// Pasta no CDN: `Downloads/MySQL-8.4` (versão atual) ou `archives/mysql-8.4` (anteriores).
    path: String,
    /// Rótulo do macOS usado no nome do arquivo (`macos15`, `macos14`…).
    #[serde(default)]
    label: String,
    /// Nome completo do arquivo, quando não segue o padrão do macOS (Linux: `…-linux-glibc2.28-x86_64-minimal.tar.xz`).
    #[serde(default)]
    file: Option<String>,
    bytes: u64,
}

fn manifest() -> Vec<ManifestVersion> {
    serde_json::from_str::<Manifest>(MANIFEST)
        .map(|m| m.versions)
        .unwrap_or_default()
}

/// URLs candidatas para baixar uma versão. Quando uma versão deixa de ser a atual da série o
/// Oracle a move de `Downloads/` para `archives/`, então a segunda opção cobre esse caso.
fn urls_for(version: &str, build: &Build, arch: &str) -> Vec<String> {
    let series = series_of(version);
    let file = build
        .file
        .clone()
        .unwrap_or_else(|| format!("mysql-{version}-{}-{arch}.tar.gz", build.label));
    let mut paths = vec![build.path.clone()];
    for alt in [format!("Downloads/MySQL-{series}"), format!("archives/mysql-{series}")] {
        if !paths.contains(&alt) {
            paths.push(alt);
        }
    }
    paths.into_iter().map(|p| format!("{CDN}/{p}/{file}")).collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MysqlRelease {
    pub version: String,
    pub series: String,
    /// Tamanho do download (ex.: `160 MB`).
    pub size: String,
    pub installed: bool,
    /// `true` se for o patch mais novo da série.
    pub latest: bool,
}

/// Versões disponíveis para download nesta máquina, da mais nova para a mais antiga.
pub fn catalog(base: &Path) -> Result<Vec<MysqlRelease>, String> {
    let arch = arch().ok_or(UNSUPPORTED)?;
    let installed = managed_versions(base);
    let mut versions: Vec<(String, u64)> = manifest()
        .into_iter()
        .filter_map(|v| v.builds.get(arch).map(|b| (v.version, b.bytes)))
        .collect();
    versions.sort_by(|a, b| version_key(&b.0).cmp(&version_key(&a.0)));

    let mut seen = std::collections::HashSet::new();
    Ok(versions
        .into_iter()
        .map(|(version, bytes)| MysqlRelease {
            series: series_of(&version),
            size: format!("{} MB", bytes / 1024 / 1024),
            installed: installed.contains(&version),
            latest: seen.insert(series_of(&version)),
            version,
        })
        .collect())
}

/// Caminhos que valem a pena guardar (o tarball completo tem centenas de MB de testes e extras).
fn keep(rel: &Path) -> bool {
    let s = rel.to_string_lossy().replace('\\', "/");
    // `bin/*.dylib`: o mysqld carrega libprotobuf-lite via @loader_path (na própria pasta bin/).
    let bin_lib = s.starts_with("bin/") && !s[4..].contains('/') && (s.ends_with(".dylib") || s.contains(".so") || s.ends_with(".dll"));
    // No Linux o pacote completo traz bibliotecas estáticas (`.a`), a versão de depuração dos
    // plugins e dicionários do MeCab, que ocupam centenas de MB e o app não usa.
    let lib = s.starts_with("lib/")
        && !s.ends_with(".a")
        && !s.starts_with("lib/plugin/debug/")
        && !s.starts_with("lib/mecab/"); // dicionários do analisador de texto em japonês (~120 MB)
    bin_lib
        || lib
        || s.starts_with("share/")
        || matches!(
            s.as_str(),
            "bin/mysqld" | "bin/mysql" | "bin/mysqldump" | "bin/mysqladmin"
                | "bin/mysqld.exe" | "bin/mysql.exe" | "bin/mysqldump.exe" | "bin/mysqladmin.exe"
        )
}

/// Baixa, extrai (só o necessário) e valida uma versão. Bloqueante.
pub fn install(base: &Path, version: &str, progress: &dyn Fn(InstallProgress)) -> Result<(), String> {
    if !valid_version(version) {
        return Err("versão inválida".into());
    }
    let arch = arch().ok_or(UNSUPPORTED)?;
    let final_dir = install_dir(base, version);
    if crate::platform::bin(&final_dir.join("bin"), "mysqld").is_file() {
        return Ok(());
    }

    let id = format!("mysql-{version}");
    let emit = |stage: &str, downloaded: u64, total: u64| {
        progress(InstallProgress {
            id: id.clone(),
            stage: stage.to_string(),
            downloaded,
            total,
            message: None,
        })
    };

    let build = manifest()
        .into_iter()
        .find(|v| v.version == version)
        .and_then(|v| v.builds.get(arch).cloned())
        .ok_or_else(|| format!("MySQL {version} não está disponível para este sistema."))?;
    let urls = urls_for(version, &build, arch);

    let root = mysql_root(base);
    let tmp_dir = root.join(format!(".tmp-{version}"));
    let archive = root.join(format!(".download-{version}"));
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;

    let result = (|| -> Result<(), String> {
        let mut last_err = String::new();
        let mut ok = false;
        for url in &urls {
            match download::download_to(url, &archive, &mut |d, t| emit("download", d, t)) {
                Ok(()) => {
                    ok = true;
                    break;
                }
                // 404: a versão pode ter mudado de pasta no CDN; tenta a próxima.
                Err(e) if e.contains("404") => last_err = e,
                Err(e) => return Err(format!("MySQL {version}: {e}")),
            }
        }
        if !ok {
            return Err(format!("MySQL {version}: {last_err}"));
        }

        emit("extract", 0, 0);
        fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        {
            download::unpack_zip_filtered(&archive, &tmp_dir, &keep)?;
            return finish_install(&tmp_dir, &final_dir, version, &emit);
        }
        let file = File::open(&archive).map_err(|e| e.to_string())?;
        let mut tar = tar::Archive::new(open_tarball(file, urls.first().map(String::as_str).unwrap_or(""))?);
        for entry in tar.entries().map_err(|e| e.to_string())? {
            let mut entry = entry.map_err(|e| format!("falha ao extrair: {e}"))?;
            let path = entry.path().map_err(|e| e.to_string())?.into_owned();
            // Remove a pasta raiz (`mysql-8.4.9-macos15-arm64/`) e rejeita `..`/caminhos absolutos.
            let comps: Vec<Component> = path.components().skip(1).collect();
            if comps.is_empty() || !comps.iter().all(|c| matches!(c, Component::Normal(_))) {
                continue;
            }
            let rel: PathBuf = comps.iter().collect();
            if !keep(&rel) {
                continue;
            }
            let out = tmp_dir.join(&rel);
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            entry
                .unpack(&out)
                .map_err(|e| format!("falha ao extrair {}: {e}", rel.display()))?;
        }

        finish_install(&tmp_dir, &final_dir, version, &emit)
    })();

    let _ = fs::remove_file(&archive);
    if result.is_err() {
        let _ = fs::remove_dir_all(&tmp_dir);
    }
    result
}

/// Verifica o `mysqld` extraído e publica a pasta temporária como instalação.
fn finish_install(tmp_dir: &Path, final_dir: &Path, version: &str, emit: &dyn Fn(&str, u64, u64)) -> Result<(), String> {
        // Verificação: o binário precisa rodar e reportar a versão pedida.
        emit("verify", 0, 0);
        #[cfg(target_os = "linux")]
        link_libaio(tmp_dir)?;
        let mysqld = crate::platform::bin(&tmp_dir.join("bin"), "mysqld");
        if !mysqld.is_file() {
            return Err("o pacote do MySQL não contém o mysqld".into());
        }
        let out = Command::new(&mysqld)
            .arg("--version")
            .output()
            .map_err(|e| format!("o mysqld baixado não executa: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout);
        if !text.contains(&format!("Ver {version}")) {
            let err = String::from_utf8_lossy(&out.stderr);
            let detail = text.lines().chain(err.lines()).find(|l| !l.trim().is_empty()).unwrap_or("sem saída");
            let hint = missing_library_hint(detail).map(|h| format!(" {h}")).unwrap_or_default();
            return Err(format!("verificação falhou ({}): {detail}.{hint}", out.status));
        }

        let _ = fs::remove_dir_all(final_dir);
        fs::rename(tmp_dir, final_dir).map_err(|e| e.to_string())?;
        Ok(())
}

/// Explica como instalar a biblioteca do sistema que o `mysqld` não achou (só acontece no Linux).
fn missing_library_hint(detail: &str) -> Option<String> {
    if detail.contains("libaio") {
        Some("Instale a libaio: no Ubuntu/Debian, `sudo apt install libaio1t64` (ou `libaio1`); no Fedora, `sudo dnf install libaio`.".into())
    } else if detail.contains("libnuma") {
        Some("Instale a libnuma: no Ubuntu/Debian, `sudo apt install libnuma1`; no Fedora, `sudo dnf install numactl-libs`.".into())
    } else if detail.contains("libncurses") || detail.contains("libtinfo") {
        Some("Instale a libtinfo: no Ubuntu/Debian, `sudo apt install libtinfo6`; no Fedora, `sudo dnf install ncurses-libs`.".into())
    } else {
        None
    }
}

/// O `mysqld` do Linux pede `libaio.so.1`, mas o Ubuntu 24.04 e o Debian 13 renomearam o pacote
/// para `libaio1t64`, cuja biblioteca se chama `libaio.so.1t64`. Quando só existe o nome novo,
/// põe um atalho com o nome antigo na pasta privada do próprio MySQL (`lib/private`, que o
/// `mysqld` consulta antes do sistema). Sem a biblioteca, a verificação seguinte explica o que instalar.
#[cfg(target_os = "linux")]
fn link_libaio(install: &Path) -> Result<(), String> {
    let triplet = match std::env::consts::ARCH {
        "aarch64" => "aarch64-linux-gnu",
        _ => "x86_64-linux-gnu",
    };
    let dirs: Vec<PathBuf> = [format!("/usr/lib/{triplet}"), format!("/lib/{triplet}"), "/usr/lib64".into(), "/usr/lib".into(), "/lib64".into(), "/lib".into()]
        .into_iter()
        .map(PathBuf::from)
        .collect();
    if dirs.iter().any(|d| d.join("libaio.so.1").exists()) {
        return Ok(());
    }
    let Some(real) = dirs.iter().map(|d| d.join("libaio.so.1t64")).find(|p| p.exists()) else {
        return Ok(());
    };
    let private = install.join("lib").join("private");
    fs::create_dir_all(&private).map_err(|e| e.to_string())?;
    let link = private.join("libaio.so.1");
    let _ = fs::remove_file(&link);
    std::os::unix::fs::symlink(&real, &link).map_err(|e| format!("não foi possível ligar a libaio: {e}"))
}

/// Abre o pacote como `tar`: `.tar.xz` (Linux) ou `.tar.gz` (macOS), pela extensão da URL.
fn open_tarball(file: File, url: &str) -> Result<Box<dyn std::io::Read>, String> {
    if url.ends_with(".tar.xz") {
        #[cfg(target_os = "linux")]
        return Ok(Box::new(liblzma::read::XzDecoder::new(file)));
        #[cfg(not(target_os = "linux"))]
        return Err("pacotes .tar.xz só são lidos no Linux".into());
    }
    Ok(Box::new(flate2::read::GzDecoder::new(file)))
}

pub fn uninstall(base: &Path, version: &str) -> Result<(), String> {
    if !valid_version(version) {
        return Err("versão inválida".into());
    }
    let dir = install_dir(base, version);
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ------------------------------------------------------------------ instalações locais

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MysqlInstall {
    pub full_version: String,
    pub series: String,
    pub mysqld: PathBuf,
    /// Instalado pelo LocalTusk (true) ou vindo do sistema, ex.: Homebrew (false).
    pub managed: bool,
}

pub fn managed_versions(base: &Path) -> Vec<String> {
    managed_installs(base).into_iter().map(|i| i.full_version).collect()
}

fn managed_installs(base: &Path) -> Vec<MysqlInstall> {
    let Ok(rd) = fs::read_dir(mysql_root(base)) else {
        return vec![];
    };
    rd.flatten()
        .filter_map(|e| {
            let version = e.file_name().to_string_lossy().to_string();
            let mysqld = crate::platform::bin(&e.path().join("bin"), "mysqld");
            (valid_version(&version) && mysqld.is_file()).then(|| MysqlInstall {
                series: series_of(&version),
                full_version: version,
                mysqld,
                managed: true,
            })
        })
        .collect()
}

/// `mysqld  Ver 9.7.1 for macos26.6 on arm64 (Homebrew)` -> `9.7.1`
fn mysqld_version(bin: &Path) -> Option<String> {
    let out = Command::new(bin).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let after = text.split("Ver ").nth(1)?;
    let v: String = after.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    valid_version(&v).then_some(v)
}

fn system_installs() -> Vec<MysqlInstall> {
    let mut seen = std::collections::HashSet::new();
    let mut out = vec![];
    #[cfg(windows)]
    {
        let mut candidates: Vec<PathBuf> = vec![];
        if let Ok(rd) = fs::read_dir(r"C:\Program Files\MySQL") {
            for e in rd.flatten() {
                candidates.push(e.path().join("bin").join("mysqld.exe"));
            }
        }
        if let Some(path) = std::env::var_os("PATH") {
            candidates.extend(std::env::split_paths(&path).filter(|d| d.is_absolute()).map(|d| d.join("mysqld.exe")));
        }
        for c in candidates {
            let Ok(real) = fs::canonicalize(&c) else { continue };
            if !seen.insert(real) {
                continue;
            }
            if let Some(v) = mysqld_version(&c) {
                out.push(MysqlInstall { series: series_of(&v), full_version: v, mysqld: c, managed: false });
            }
        }
        return out;
    }
    let mut candidates: Vec<PathBuf> = ["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin", "/usr/local/sbin"]
        .iter()
        .map(|d| Path::new(d).join("mysqld"))
        .collect();
    for prefix in ["/opt/homebrew", "/usr/local"] {
        candidates.push(Path::new(prefix).join("opt/mysql/bin/mysqld"));
        if let Ok(rd) = fs::read_dir(Path::new(prefix).join("opt")) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with("mysql@") {
                    candidates.push(e.path().join("bin/mysqld"));
                }
            }
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).filter(|d| d.is_absolute()).map(|d| d.join("mysqld")));
    }
    for c in candidates {
        let Ok(real) = fs::canonicalize(&c) else { continue };
        if !seen.insert(real) {
            continue;
        }
        if let Some(v) = mysqld_version(&c) {
            out.push(MysqlInstall {
                series: series_of(&v),
                full_version: v,
                mysqld: c,
                managed: false,
            });
        }
    }
    out
}

/// Todas as instalações do MySQL, da mais nova para a mais antiga (a gerenciada vence a do sistema).
pub fn installs(base: &Path) -> Vec<MysqlInstall> {
    let mut all = managed_installs(base);
    for sys in system_installs() {
        if !all.iter().any(|i| i.full_version == sys.full_version) {
            all.push(sys);
        }
    }
    all.sort_by(|a, b| version_key(&b.full_version).cmp(&version_key(&a.full_version)));
    all
}

/// Escolhe o mysqld para uma versão pedida (completa `8.4.9` ou série `8.4`).
/// Sem correspondência usa o mais novo instalado.
pub fn pick(base: &Path, wanted: &str) -> Option<MysqlInstall> {
    let all = installs(base);
    all.iter()
        .find(|i| i.full_version == wanted)
        .or_else(|| all.iter().find(|i| i.series == wanted))
        .cloned()
        .or_else(|| all.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explica_biblioteca_que_falta() {
        let h = missing_library_hint("mysqld: error while loading shared libraries: libaio.so.1: cannot open shared object file").unwrap();
        assert!(h.contains("libaio1t64") && h.contains("dnf"));
        assert!(missing_library_hint("libnuma.so.1: cannot open").unwrap().contains("libnuma1"));
        assert!(missing_library_hint("sem saída").is_none());
    }

    #[test]
    fn catalogo_tem_o_linux_com_arquivo_proprio() {
        let versions = manifest();
        let v = versions.iter().find(|v| v.version.starts_with("8.4.")).expect("8.4");
        let x86 = v.builds.get("linux-x86_64").expect("linux x86_64");
        let urls = urls_for(&v.version, x86, "linux-x86_64");
        assert!(urls[0].ends_with(".tar.xz"), "{urls:?}");
        assert!(urls[0].contains("linux-glibc"), "{urls:?}");
        // O macOS continua com o nome antigo (.tar.gz com o rótulo do sistema).
        let mac = v.builds.get("arm64").expect("mac");
        assert!(urls_for(&v.version, mac, "arm64")[0].ends_with("-arm64.tar.gz"));
    }
}
