//! Gerenciador de versões do PostgreSQL.
//!
//! Baixa os binários prontos do projeto theseus-rs/postgresql-binaries (macOS e Linux, com `psql`,
//! `pg_dump` e o servidor completo) e guarda só o necessário em
//! `~/LocalTusk/bin/postgres/<versão>/` (`bin/`, `lib/`, `share/`).
//!
//! Como no MySQL, o catálogo é uma lista **embutida** (`resources/postgres-versions.json`,
//! gerada por `scripts/update_postgres_versions.py`) com o tamanho e o SHA-256 de cada arquivo:
//! o app não consulta a API do GitHub (limite de 60 requisições por hora) e o catálogo funciona
//! offline. O download é conferido contra o SHA-256 antes de extrair.

use crate::download::{self, InstallProgress};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const MANIFEST: &str = include_str!("../resources/postgres-versions.json");
const RELEASES: &str = "https://github.com/theseus-rs/postgresql-binaries/releases/download";

/// Executáveis que o app usa. O resto do `bin/` (pg_upgrade, pgbench…) não vale o espaço.
const BINARIES: &[&str] = &[
    "postgres",
    "initdb",
    "psql",
    "pg_dump",
    "pg_dumpall",
    "pg_restore",
    "pg_isready",
    "createdb",
];

pub fn pg_root(base: &Path) -> PathBuf {
    base.join("bin").join("postgres")
}

pub fn install_dir(base: &Path, version: &str) -> PathBuf {
    pg_root(base).join(version)
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

/// Versão principal: `16.16.0` -> `16`.
pub fn major_of(version: &str) -> String {
    version.split('.').next().unwrap_or("").to_string()
}

/// `16.16.0` -> `16.16`, como o `postgres --version` mostra.
fn reported_version(version: &str) -> String {
    version.split('.').take(2).collect::<Vec<_>>().join(".")
}

fn arch() -> Option<(&'static str, &'static str)> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some(("arm64", "aarch64-apple-darwin")),
        ("macos", "x86_64") => Some(("x86_64", "x86_64-apple-darwin")),
        ("linux", "aarch64") => Some(("linux-aarch64", "aarch64-unknown-linux-gnu")),
        ("linux", "x86_64") => Some(("linux-x86_64", "x86_64-unknown-linux-gnu")),
        ("windows", "x86_64") => Some(("win-x86_64", "x86_64-pc-windows-msvc")),
        _ => None,
    }
}

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
    bytes: u64,
    sha256: String,
}

fn manifest() -> Vec<ManifestVersion> {
    serde_json::from_str::<Manifest>(MANIFEST)
        .map(|m| m.versions)
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostgresRelease {
    pub version: String,
    /// Versão principal (`16`): é o que o projeto guarda para acompanhar os patches.
    pub major: String,
    /// Tamanho do download (ex.: `84 MB`).
    pub size: String,
    pub installed: bool,
    /// `true` se for o patch mais novo da versão principal.
    pub latest: bool,
}

/// Versões disponíveis para download nesta máquina, da mais nova para a mais antiga.
pub fn catalog(base: &Path) -> Result<Vec<PostgresRelease>, String> {
    let (arch, _) = arch().ok_or("O download do PostgreSQL só está disponível para macOS, Linux e Windows (x64) por enquanto.")?;
    let installed = managed_versions(base);
    let mut versions: Vec<(String, u64)> = manifest()
        .into_iter()
        .filter_map(|v| v.builds.get(arch).map(|b| (v.version, b.bytes)))
        .collect();
    versions.sort_by(|a, b| version_key(&b.0).cmp(&version_key(&a.0)));

    let mut seen = std::collections::HashSet::new();
    Ok(versions
        .into_iter()
        .map(|(version, bytes)| PostgresRelease {
            major: major_of(&version),
            size: format!("{} MB", bytes / 1024 / 1024),
            installed: installed.contains(&version),
            latest: seen.insert(major_of(&version)),
            version,
        })
        .collect())
}

/// Caminhos que valem a pena guardar. O pacote completo tem mais de 250 MB, quase tudo do
/// compilador JIT (LLVM), do PL/Python e de arquivos de desenvolvimento que o app não usa.
fn keep(rel: &Path) -> bool {
    let comps: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    match comps.first().map(String::as_str) {
        Some("bin") => comps.len() == 2 && BINARIES.contains(&comps[1].trim_end_matches(".exe")),
        Some("lib") => {
            comps.len() == 2 && {
                let n = comps[1].as_str();
                (n.ends_with(".dylib") || n.contains(".so") || n.ends_with(".dll")) && !n.starts_with("libLLVM") && !n.contains("llvmjit") && !n.contains("plpython")
            }
        }
        Some("share") => comps.get(1).map(String::as_str) != Some("doc"),
        _ => false,
    }
}

/// Baixa, confere o SHA-256, extrai (só o necessário) e valida uma versão. Bloqueante.
pub fn install(base: &Path, version: &str, progress: &dyn Fn(InstallProgress)) -> Result<(), String> {
    if !valid_version(version) {
        return Err("versão inválida".into());
    }
    let (arch, triple) = arch().ok_or("O download do PostgreSQL só está disponível para macOS, Linux e Windows (x64) por enquanto.")?;
    let final_dir = install_dir(base, version);
    if crate::platform::bin(&final_dir.join("bin"), "postgres").is_file() {
        return Ok(());
    }

    let id = format!("postgres-{version}");
    let emit = |stage: &str, downloaded: u64, total: u64| {
        progress(InstallProgress { id: id.clone(), stage: stage.to_string(), downloaded, total, message: None })
    };

    let build = manifest()
        .into_iter()
        .find(|v| v.version == version)
        .and_then(|v| v.builds.get(arch).cloned())
        .ok_or_else(|| format!("PostgreSQL {version} não está disponível para este sistema."))?;
    let url = format!("{RELEASES}/{version}/postgresql-{version}-{triple}.tar.gz");

    let root = pg_root(base);
    let tmp_dir = root.join(format!(".tmp-{version}"));
    let archive = root.join(format!(".download-{version}.tar.gz"));
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;

    let result = (|| -> Result<(), String> {
        download::download_to(&url, &archive, &mut |d, t| emit("download", d, t))
            .map_err(|e| format!("PostgreSQL {version}: {e}"))?;

        emit("verify", 0, 0);
        if download::sha256_hex(&archive)? != build.sha256 {
            return Err(format!("verificação de integridade (SHA-256) do PostgreSQL {version} falhou"));
        }

        emit("extract", 0, 0);
        fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
        let file = File::open(&archive).map_err(|e| e.to_string())?;
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        for entry in tar.entries().map_err(|e| e.to_string())? {
            let mut entry = entry.map_err(|e| format!("falha ao extrair: {e}"))?;
            let path = entry.path().map_err(|e| e.to_string())?.into_owned();
            // Remove a pasta raiz (`postgresql-16.16.0-aarch64-apple-darwin/`) e rejeita `..`.
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
            if entry.header().entry_type().is_hard_link() {
                // O destino do link vem com a pasta raiz do pacote; refaz o caminho dentro da pasta
                // temporária (o `unpack` o resolveria a partir da pasta atual do app).
                let target = entry.link_name().map_err(|e| e.to_string())?.ok_or("link sem destino")?.into_owned();
                let tcomps: Vec<Component> = target.components().skip(1).collect();
                if tcomps.is_empty() || !tcomps.iter().all(|c| matches!(c, Component::Normal(_))) {
                    continue;
                }
                let source: PathBuf = tmp_dir.join(tcomps.iter().collect::<PathBuf>());
                if fs::hard_link(&source, &out).is_err() {
                    fs::copy(&source, &out).map_err(|e| format!("falha ao extrair {}: {e}", rel.display()))?;
                }
                continue;
            }
            entry.unpack(&out).map_err(|e| format!("falha ao extrair {}: {e}", rel.display()))?;
        }

        // O servidor precisa rodar e reportar a versão pedida.
        let postgres = crate::platform::bin(&tmp_dir.join("bin"), "postgres");
        if !postgres.is_file() {
            return Err("o pacote do PostgreSQL não contém o postgres".into());
        }
        let out = Command::new(&postgres)
            .arg("--version")
            .output()
            .map_err(|e| format!("o postgres baixado não executa: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout);
        if !text.contains(&format!("(PostgreSQL) {}", reported_version(version))) {
            let err = String::from_utf8_lossy(&out.stderr);
            let detail = text.lines().chain(err.lines()).find(|l| !l.trim().is_empty()).unwrap_or("sem saída");
            return Err(format!("verificação falhou ({}): {detail}", out.status));
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
pub struct PostgresInstall {
    pub full_version: String,
    pub major: String,
    pub postgres: PathBuf,
}

impl PostgresInstall {
    /// Pasta `bin/` com `initdb`, `psql`, `pg_dump`…
    pub fn bin_dir(&self) -> PathBuf {
        self.postgres.parent().map(Path::to_path_buf).unwrap_or_default()
    }

    /// Caminho de um cliente (`psql`, `pg_dump`…), se existir.
    pub fn tool(&self, name: &str) -> Option<PathBuf> {
        let p = crate::platform::bin(&self.bin_dir(), name);
        p.is_file().then_some(p)
    }
}

pub fn managed_versions(base: &Path) -> Vec<String> {
    installs(base).into_iter().map(|i| i.full_version).collect()
}

/// Versões instaladas pelo app, da mais nova para a mais antiga.
pub fn installs(base: &Path) -> Vec<PostgresInstall> {
    let Ok(rd) = fs::read_dir(pg_root(base)) else {
        return vec![];
    };
    let mut all: Vec<PostgresInstall> = rd
        .flatten()
        .filter_map(|e| {
            let version = e.file_name().to_string_lossy().to_string();
            let postgres = crate::platform::bin(&e.path().join("bin"), "postgres");
            (valid_version(&version) && postgres.is_file()).then(|| PostgresInstall {
                major: major_of(&version),
                full_version: version,
                postgres,
            })
        })
        .collect();
    all.sort_by(|a, b| version_key(&b.full_version).cmp(&version_key(&a.full_version)));
    all
}

/// Escolhe o servidor para uma versão pedida (completa `16.16.0` ou major `16`).
/// Sem correspondência usa o mais novo instalado.
pub fn pick(base: &Path, wanted: &str) -> Option<PostgresInstall> {
    let all = installs(base);
    all.iter()
        .find(|i| i.full_version == wanted)
        .or_else(|| all.iter().find(|i| i.major == wanted))
        .cloned()
        .or_else(|| all.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogo_embutido_tem_as_versoes_principais_com_sha256() {
        let m = manifest();
        assert!(m.len() >= 4, "esperava ao menos 4 versões principais");
        for v in &m {
            assert!(valid_version(&v.version), "{}", v.version);
            for b in v.builds.values() {
                assert_eq!(b.sha256.len(), 64, "{} sem SHA-256", v.version);
                assert!(b.bytes > 10 * 1024 * 1024);
            }
        }
        // Sem repetir a versão principal.
        let majors: std::collections::HashSet<String> = m.iter().map(|v| major_of(&v.version)).collect();
        assert_eq!(majors.len(), m.len());
    }

    #[test]
    fn versao_so_aceita_tres_numeros() {
        assert!(valid_version("16.16.0"));
        assert!(!valid_version("16"));
        assert!(!valid_version("16.16"));
        assert!(!valid_version("../1.2.3"));
        assert!(!valid_version("16.x.0"));
        assert_eq!(major_of("16.16.0"), "16");
        assert_eq!(reported_version("16.16.0"), "16.16");
    }

    #[test]
    fn guarda_so_o_necessario_do_pacote() {
        let k = |s: &str| keep(Path::new(s));
        assert!(k("bin/postgres") && k("bin/initdb") && k("bin/psql") && k("bin/pg_dump") && k("bin/pg_restore"));
        assert!(!k("bin/pgbench") && !k("bin/pg_upgrade"));
        assert!(k("lib/libpq.5.dylib") && k("lib/libicudata.79.dylib") && k("lib/pg_trgm.dylib"));
        assert!(!k("lib/libLLVM.dylib") && !k("lib/llvmjit.dylib") && !k("lib/plpython3.dylib"));
        assert!(!k("lib/libpq.a") && !k("lib/bitcode/x.bc") && !k("lib/pgxs/Makefile"));
        assert!(k("bin/postgres.exe") && k("bin/pg_dump.exe") && !k("bin/pgbench.exe"));
        assert!(k("lib/libpq.dll") && !k("lib/libLLVM.dll"));
        assert!(k("share/postgres.bki") && k("share/timezone/UTC") && !k("share/doc/x.html"));
        assert!(!k("include/libpq-fe.h") && !k("COPYRIGHT"));
    }

    #[test]
    fn escolhe_por_versao_completa_ou_major() {
        let base = std::env::temp_dir().join(format!("localtusk-pg-pick-{}", crate::project::now()));
        let _ = fs::remove_dir_all(&base);
        for v in ["15.20.0", "16.15.0", "16.16.0"] {
            let d = install_dir(&base, v).join("bin");
            fs::create_dir_all(&d).unwrap();
            fs::write(crate::platform::bin(&d, "postgres"), "").unwrap();
        }
        // Pasta que não é versão é ignorada.
        fs::create_dir_all(pg_root(&base).join(".tmp-17.0.0")).unwrap();
        assert_eq!(pick(&base, "16").unwrap().full_version, "16.16.0");
        assert_eq!(pick(&base, "16.15.0").unwrap().full_version, "16.15.0");
        assert_eq!(pick(&base, "15").unwrap().full_version, "15.20.0");
        assert_eq!(pick(&base, "99").unwrap().full_version, "16.16.0", "sem correspondência usa a mais nova");
        assert_eq!(pick(&base, "").unwrap().full_version, "16.16.0");
        let _ = fs::remove_dir_all(&base);
        assert!(pick(&base, "16").is_none());
    }
}
