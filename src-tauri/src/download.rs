//! Utilitários de download compartilhados (PHP, MySQL, Caddy, Adminer Neo, Mailpit).

use serde::Serialize;
use sha2::{Digest, Sha256, Sha512};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Evento de progresso enviado à interface (`install-progress`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallProgress {
    /// Identificador do item: versão do PHP (`8.3.32`) ou nome da ferramenta (`mailpit`).
    pub id: String,
    /// `download` | `extract` | `verify` | `done` | `error`
    pub stage: String,
    pub downloaded: u64,
    pub total: u64,
    pub message: Option<String>,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent("LocalTusk/0.1")
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_resolve(Some(Duration::from_secs(20)))
        .build()
        .into()
}

/// GET que devolve o corpo como texto (até 10 MB).
pub fn get_string(url: &str) -> Result<String, String> {
    agent()
        .get(url)
        .call()
        .map_err(|e| format!("falha ao acessar {url}: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())
}

/// Extrai um `.zip` em `dest` (usado pelos pacotes do Windows). Rejeita caminhos
/// que escapem da pasta de destino.
#[cfg(windows)]
pub fn unpack_zip(archive: &Path, dest: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("pacote inválido: {e}"))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = entry.enclosed_name() else { continue };
        let out = dest.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut w = File::create(&out).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut w).map_err(|e| format!("falha ao extrair {}: {e}", rel.display()))?;
        }
    }
    Ok(())
}

/// Extrai um `.zip` descartando a pasta raiz do pacote e mantendo só o que `keep`
/// aceita (usado pelo MySQL no Windows, cujo pacote completo passa de 1 GB).
#[cfg(windows)]
pub fn unpack_zip_filtered(archive: &Path, dest: &Path, keep: &dyn Fn(&Path) -> bool) -> Result<(), String> {
    let file = File::open(archive).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("pacote inválido: {e}"))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(full) = entry.enclosed_name() else { continue };
        let comps: Vec<std::path::Component> = full.components().skip(1).collect();
        if comps.is_empty() || !comps.iter().all(|c| matches!(c, std::path::Component::Normal(_))) {
            continue;
        }
        let rel: PathBuf = comps.iter().collect();
        if !keep(&rel) {
            continue;
        }
        let out = dest.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut w = File::create(&out).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut w).map_err(|e| format!("falha ao extrair {}: {e}", rel.display()))?;
        }
    }
    Ok(())
}

/// Baixa `url` para `dest`, seguindo redirecionamentos. `on_progress(baixado, total)` é
/// chamado com no máximo ~6 eventos por segundo e uma vez ao final.
pub fn download_to(
    url: &str,
    dest: &Path,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<(), String> {
    let mut resp = agent().get(url).call().map_err(|e| match e {
        ureq::Error::StatusCode(404) => "arquivo não disponível para download (404)".to_string(),
        other => format!("falha no download: {other}"),
    })?;
    let total = resp.body().content_length().unwrap_or(0);
    let mut reader = resp.body_mut().as_reader();
    let mut out = File::create(dest).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut downloaded = 0u64;
    let mut last = Instant::now();
    on_progress(0, total);
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("falha no download: {e}"))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        downloaded += n as u64;
        if last.elapsed() > Duration::from_millis(150) {
            on_progress(downloaded, total);
            last = Instant::now();
        }
    }
    out.flush().map_err(|e| e.to_string())?;
    if total > 0 && downloaded != total {
        return Err("download incompleto, tente novamente".into());
    }
    on_progress(downloaded, downloaded.max(total));
    Ok(())
}

/// SHA-512 do arquivo, em hexadecimal minúsculo.
pub fn sha512_hex(path: &Path) -> Result<String, String> {
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha512::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// SHA-256 do arquivo, em hexadecimal minúsculo.
pub fn sha256_hex(path: &Path) -> Result<String, String> {
    let mut f = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
