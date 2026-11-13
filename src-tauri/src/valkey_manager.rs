//! Gerenciador do Valkey, o servidor compatível com o Redis que dá o "Redis" de cada projeto.
//!
//! O Valkey (fork livre do Redis, licença BSD, mesmo protocolo: `phpredis`, `predis`, o
//! `redis-cli`…) não publica binários para macOS, só o código-fonte. Então o app baixa o
//! código da versão fixada abaixo, confere o SHA-256, compila (cerca de 15 segundos, só
//! precisa do `make` e do compilador das Xcode Command Line Tools) e guarda os dois
//! executáveis em `~/LocalTusk/bin/valkey/<versão>/`.

use crate::download::{self, InstallProgress};
use serde::Serialize;
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

/// Versão estável compilada pelo app e o SHA-256 do arquivo de código-fonte dela no GitHub.
pub const VERSION: &str = "9.0.7";
const SOURCE_SHA256: &str = "ba3e4cd0fb90e994323212b4a49e7e454e25e02864b9b3f1bf04416699d3f5c2";
/// Tamanho aproximado do download, só para mostrar na interface.
const SOURCE_BYTES: u64 = 4_193_278;

fn source_url(version: &str) -> String {
    format!("https://github.com/valkey-io/valkey/archive/refs/tags/{version}.tar.gz")
}

pub fn valkey_root(base: &Path) -> PathBuf {
    base.join("bin").join("valkey")
}

#[allow(unreachable_code)]
pub fn install_dir(base: &Path, version: &str) -> PathBuf {
    valkey_root(base).join(version)
}

/// Aceita só `X.Y.Z` numérico (impede path traversal).
pub fn valid_version(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit()))
}

fn version_key(v: &str) -> Vec<u32> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValkeyRelease {
    pub version: String,
    /// Tamanho do download do código-fonte (ex.: `4 MB`).
    pub size: String,
    pub installed: bool,
}

/// A versão que o app instala nesta máquina.
pub fn catalog(base: &Path) -> Result<Vec<ValkeyRelease>, String> {
    // No Windows não há Valkey: usamos o Redis do projeto comunitário redis-windows.
    #[cfg(windows)]
    {
        return Ok(vec![ValkeyRelease {
            version: WIN_VERSION.into(),
            size: format!("{} MB", WIN_ZIP_BYTES.div_ceil(1024 * 1024)),
            installed: managed_versions(base).iter().any(|v| v == WIN_VERSION),
        }]);
    }
    if cfg!(not(unix)) {
        return Err("O Redis (Valkey) só está disponível para macOS e Linux por enquanto.".into());
    }
    Ok(vec![ValkeyRelease {
        version: VERSION.into(),
        size: format!("{} MB", SOURCE_BYTES.div_ceil(1024 * 1024)),
        installed: managed_versions(base).iter().any(|v| v == VERSION),
    }])
}

/// Versão e integridade do Redis para Windows (projeto comunitário redis-windows,
/// build MSYS2 do Redis oficial). Sem compilação: baixa, confere e extrai.
#[cfg(windows)]
const WIN_VERSION: &str = "8.10.3";
#[cfg(windows)]
const WIN_ZIP_SHA256: &str = "af16df340525b431e9ec8dabfd625d5e7a233f82805b5418819904ea909c447f";
#[cfg(windows)]
const WIN_ZIP_BYTES: u64 = 14_036_243;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValkeyInstall {
    pub full_version: String,
    pub server: PathBuf,
}

impl ValkeyInstall {
    /// `valkey-cli` (o cliente de linha de comando), ao lado do servidor.
    pub fn cli(&self) -> Option<PathBuf> {
        let dir = self.server.parent()?;
        [crate::platform::bin(dir, "valkey-cli"), crate::platform::bin(dir, "redis-cli")]
            .into_iter()
            .find(|p| p.is_file())
    }
}

pub fn managed_versions(base: &Path) -> Vec<String> {
    installs(base).into_iter().map(|i| i.full_version).collect()
}

/// Versões instaladas pelo app, da mais nova para a mais antiga.
pub fn installs(base: &Path) -> Vec<ValkeyInstall> {
    let Ok(rd) = fs::read_dir(valkey_root(base)) else {
        return vec![];
    };
    let mut all: Vec<ValkeyInstall> = rd
        .flatten()
        .filter_map(|e| {
            let version = e.file_name().to_string_lossy().to_string();
            let server = crate::platform::bin(&e.path(), "valkey-server");
            let server = if server.is_file() { server } else { crate::platform::bin(&e.path(), "redis-server") };
            (valid_version(&version) && server.is_file()).then_some(ValkeyInstall { full_version: version, server })
        })
        .collect();
    all.sort_by(|a, b| version_key(&b.full_version).cmp(&version_key(&a.full_version)));
    all
}

/// O Valkey mais novo instalado.
pub fn pick(base: &Path) -> Option<ValkeyInstall> {
    installs(base).into_iter().next()
}

/// As ferramentas de compilação (`make` e o compilador C) estão instaladas? No macOS são as Xcode
/// Command Line Tools; sem elas, rodar o `make` abriria um diálogo de instalação no meio do app.
fn build_tools_ready() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let ok = Command::new("xcode-select")
            .arg("-p")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            return Err(
                "Para instalar o Redis (Valkey) o macOS precisa das Xcode Command Line Tools. Abra o Terminal, rode `xcode-select --install`, aguarde terminar e tente de novo."
                    .into(),
            );
        }
    }
    #[cfg(target_os = "linux")]
    {
        let missing: Vec<&str> = [("make", &["make"][..]), ("um compilador C (gcc ou cc)", &["cc", "gcc"][..])]
            .into_iter()
            .filter(|(_, names)| !names.iter().any(|n| in_path(n)))
            .map(|(label, _)| label)
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "Para instalar o Redis (Valkey) o app precisa compilá-lo, e faltam: {}. No Ubuntu/Debian rode `sudo apt install build-essential`; no Fedora, `sudo dnf install make gcc`. Depois tente de novo.",
                missing.join(" e ")
            ));
        }
    }
    Ok(())
}

/// O programa existe em algum lugar do `PATH`?
#[cfg(target_os = "linux")]
fn in_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

/// Baixa o código-fonte, confere o SHA-256, compila e valida. Bloqueante.
pub fn install(base: &Path, version: &str, progress: &dyn Fn(InstallProgress)) -> Result<(), String> {
    #[cfg(windows)]
    {
        return install_win(base, version, progress);
    }
    if version != VERSION {
        return Err(format!("Só a versão {VERSION} do Valkey está disponível."));
    }
    let final_dir = install_dir(base, version);
    if final_dir.join("valkey-server").is_file() {
        return Ok(());
    }
    build_tools_ready()?;

    let id = format!("valkey-{version}");
    let emit = |stage: &str, downloaded: u64, total: u64, message: Option<&str>| {
        progress(InstallProgress {
            id: id.clone(),
            stage: stage.to_string(),
            downloaded,
            total,
            message: message.map(String::from),
        })
    };

    let root = valkey_root(base);
    let tmp_dir = root.join(format!(".tmp-{version}"));
    let archive = root.join(format!(".download-{version}.tar.gz"));
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;

    let result = (|| -> Result<(), String> {
        download::download_to(&source_url(version), &archive, &mut |d, t| emit("download", d, t, None))
            .map_err(|e| format!("Valkey {version}: {e}"))?;

        emit("verify", 0, 0, None);
        if download::sha256_hex(&archive)? != SOURCE_SHA256 {
            return Err(format!("verificação de integridade (SHA-256) do Valkey {version} falhou"));
        }

        emit("extract", 0, 0, Some("Extraindo o código-fonte…"));
        let src = tmp_dir.join("src");
        fs::create_dir_all(&src).map_err(|e| e.to_string())?;
        let file = File::open(&archive).map_err(|e| e.to_string())?;
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        for entry in tar.entries().map_err(|e| e.to_string())? {
            let mut entry = entry.map_err(|e| format!("falha ao extrair: {e}"))?;
            let path = entry.path().map_err(|e| e.to_string())?.into_owned();
            // Remove a pasta raiz (`valkey-9.0.7/`) e rejeita `..`/caminhos absolutos.
            let comps: Vec<Component> = path.components().skip(1).collect();
            if comps.is_empty() || !comps.iter().all(|c| matches!(c, Component::Normal(_))) {
                continue;
            }
            let rel: PathBuf = comps.iter().collect();
            let out = src.join(&rel);
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            entry.unpack(&out).map_err(|e| format!("falha ao extrair {}: {e}", rel.display()))?;
        }

        emit("extract", 0, 0, Some("Compilando o Valkey (cerca de 15 segundos)…"));
        let jobs = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).to_string();
        let log = tmp_dir.join("build.log");
        let log_file = File::create(&log).map_err(|e| e.to_string())?;
        let status = Command::new("make")
            .args(["-j", &jobs, "BUILD_TLS=no"])
            .current_dir(&src)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_file.try_clone().map_err(|e| e.to_string())?))
            .stderr(Stdio::from(log_file))
            .status()
            .map_err(|e| format!("não foi possível rodar o make: {e}"))?;
        if !status.success() {
            let text = fs::read_to_string(&log).unwrap_or_default();
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            return Err(format!("a compilação do Valkey falhou ({status})\n\n{}", lines[lines.len().saturating_sub(8)..].join("\n")));
        }

        let out_dir = tmp_dir.join("out");
        fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
        for name in ["valkey-server", "valkey-cli"] {
            fs::copy(src.join("src").join(name), out_dir.join(name))
                .map_err(|e| format!("a compilação não gerou o {name}: {e}"))?;
        }

        emit("verify", 0, 0, None);
        let out = Command::new(out_dir.join("valkey-server"))
            .arg("--version")
            .output()
            .map_err(|e| format!("o valkey-server compilado não executa: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout);
        if !text.contains(&format!("v={version}")) {
            return Err(format!("verificação falhou: {}", text.lines().next().unwrap_or("sem saída")));
        }

        let _ = fs::remove_dir_all(&final_dir);
        fs::rename(&out_dir, &final_dir).map_err(|e| e.to_string())?;
        Ok(())
    })();

    let _ = fs::remove_file(&archive);
    let _ = fs::remove_dir_all(&tmp_dir);
    result
}


/// Instalação no Windows: baixa o Redis (build comunitário redis-windows), confere o
/// SHA-256 e extrai. O servidor fica como `redis-server.exe` na pasta da versão.
#[cfg(windows)]
fn install_win(base: &Path, version: &str, progress: &dyn Fn(InstallProgress)) -> Result<(), String> {
    if version != WIN_VERSION {
        return Err(format!("Só a versão {WIN_VERSION} do Redis está disponível no Windows."));
    }
    let final_dir = install_dir(base, version);
    if crate::platform::bin(&final_dir, "redis-server").is_file() {
        return Ok(());
    }
    let id = format!("valkey-{version}");
    let emit = |stage: &str, downloaded: u64, total: u64, message: Option<&str>| {
        progress(InstallProgress { id: id.clone(), stage: stage.to_string(), downloaded, total, message: message.map(String::from) });
    };
    let root = valkey_root(base);
    let tmp_dir = root.join(format!(".tmp-{version}"));
    let archive = root.join(format!(".download-{version}.zip"));
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), String> {
        let url = format!("https://github.com/redis-windows/redis-windows/releases/download/{version}/Redis-{version}-Windows-x64-msys2.zip");
        download::download_to(&url, &archive, &mut |d, t| emit("download", d, t, None)).map_err(|e| format!("Redis {version}: {e}"))?;
        emit("verify", 0, 0, None);
        if download::sha256_hex(&archive)? != WIN_ZIP_SHA256 {
            return Err(format!("verificação de integridade (SHA-256) do Redis {version} falhou"));
        }
        emit("extract", 0, 0, None);
        fs::create_dir_all(&tmp_dir).map_err(|e| e.to_string())?;
        download::unpack_zip(&archive, &tmp_dir)?;
        let server = crate::platform::bin(&tmp_dir, "redis-server");
        if !server.is_file() {
            return Err("o pacote do Redis não contém o redis-server.exe".into());
        }
        let out = Command::new(&server).arg("--version").output().map_err(|e| format!("o redis-server baixado não executa: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout);
        if !text.contains(&format!("v={version}")) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versao_fixada_e_valida() {
        assert!(valid_version(VERSION));
        assert_eq!(SOURCE_SHA256.len(), 64);
        assert!(source_url(VERSION).ends_with("/9.0.7.tar.gz"));
        assert!(!valid_version("../9.0.7") && !valid_version("9.0"));
    }

    #[test]
    fn lista_e_escolhe_a_mais_nova_instalada() {
        let base = std::env::temp_dir().join(format!("localtusk-valkey-pick-{}", crate::project::now()));
        let _ = fs::remove_dir_all(&base);
        assert!(pick(&base).is_none());
        for v in ["8.1.0", "9.0.7"] {
            let d = install_dir(&base, v);
            fs::create_dir_all(&d).unwrap();
            fs::write(crate::platform::bin(&d, "valkey-server"), "").unwrap();
        }
        fs::create_dir_all(valkey_root(&base).join(".tmp-9.9.9")).unwrap();
        assert_eq!(pick(&base).unwrap().full_version, "9.0.7");
        assert_eq!(managed_versions(&base), vec!["9.0.7", "8.1.0"]);
        assert!(pick(&base).unwrap().cli().is_none());
        let _ = fs::remove_dir_all(&base);
    }
}
