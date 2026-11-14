//! Aviso de nova versão do LocalTusk.
//!
//! Consulta a última versão publicada no GitHub (`releases/latest`, que ignora rascunhos e
//! pré-lançamentos) e compara com a versão do app. Só lê: não envia nenhum dado seu e não
//! instala nada, apenas indica onde baixar.

use crate::download;
use serde::Serialize;
use serde_json::Value;

const LATEST_URL: &str = "https://api.github.com/repos/jhonatanMP/localtusk/releases/latest";
/// Só abrimos links que apontam para o repositório do app.
const ALLOWED_PREFIX: &str = "https://github.com/jhonatanMP/localtusk/";

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// Versão instalada (`0.10.0`).
    pub current: String,
    /// Última versão publicada, sem o `v`.
    pub latest: String,
    pub available: bool,
    /// Página do release, com as novidades.
    pub page_url: String,
    /// Link direto do instalador, quando dá para escolher sem perguntar (macOS).
    pub download_url: Option<String>,
}

/// `v0.10.0`, `0.10` ou `1.2.3-beta.1` viram `(maior, menor, correção)`.
fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let core = s.trim().trim_start_matches(['v', 'V']);
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |p| p.parse().ok())?;
    let patch = parts.next().map_or(Some(0), |p| p.parse().ok())?;
    Some((major, minor, patch))
}

/// `true` quando `latest` é uma versão maior que `current` (compara número a número:
/// 0.10.0 é maior que 0.9.0).
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// No macOS escolhemos o `.dmg` da arquitetura certa. No Linux há AppImage, .deb e .rpm e não
/// sabemos como o app foi instalado, então ficamos só com a página do release.
fn pick_download(assets: &[Value], os: &str, arch: &str) -> Option<String> {
    if os != "macos" {
        return None;
    }
    let suffix = match arch {
        "aarch64" => "_aarch64.dmg",
        "x86_64" => "_x64.dmg",
        _ => return None,
    };
    assets
        .iter()
        .find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(suffix)))
        .and_then(|a| a["browser_download_url"].as_str())
        .filter(|u| is_allowed_url(u))
        .map(str::to_string)
}

pub fn is_allowed_url(url: &str) -> bool {
    url.starts_with(ALLOWED_PREFIX)
}

fn parse_release(body: &str, current: &str, os: &str, arch: &str) -> Result<UpdateInfo, String> {
    let rel: Value = serde_json::from_str(body).map_err(|e| format!("resposta inesperada do GitHub: {e}"))?;
    let tag = rel["tag_name"].as_str().ok_or("o release não tem versão")?;
    let latest = tag.trim_start_matches(['v', 'V']).to_string();
    if parse_version(&latest).is_none() {
        return Err(format!("versão publicada não reconhecida: {tag}"));
    }
    let page_url = rel["html_url"]
        .as_str()
        .filter(|u| is_allowed_url(u))
        .map(str::to_string)
        .unwrap_or_else(|| format!("{ALLOWED_PREFIX}releases/latest"));
    let assets = rel["assets"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    Ok(UpdateInfo {
        current: current.to_string(),
        available: is_newer(&latest, current),
        latest,
        page_url,
        download_url: pick_download(assets, os, arch),
    })
}

/// Consulta o GitHub (bloqueante).
pub fn check() -> Result<UpdateInfo, String> {
    let body = download::get_string(LATEST_URL).map_err(|_| "não foi possível consultar as versões publicadas (sem internet?)".to_string())?;
    parse_release(&body, env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compara_numero_a_numero() {
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(is_newer("0.10.1", "0.10.0"));
        assert!(!is_newer("0.10.0", "0.10.0"));
        assert!(!is_newer("0.9.0", "0.10.0"));
        assert!(!is_newer("lixo", "0.10.0"));
        // Sufixo de pré-lançamento é ignorado na comparação.
        assert!(is_newer("0.11.0-beta.1", "0.10.0"));
        assert_eq!(parse_version("0.10"), Some((0, 10, 0)));
    }

    fn release(tag: &str) -> String {
        format!(
            r#"{{"tag_name":"{tag}","html_url":"https://github.com/jhonatanMP/localtusk/releases/tag/{tag}",
            "assets":[
              {{"name":"LocalTusk_0.11.0_x64.dmg","browser_download_url":"https://github.com/jhonatanMP/localtusk/releases/download/{tag}/LocalTusk_0.11.0_x64.dmg"}},
              {{"name":"LocalTusk_0.11.0_aarch64.dmg","browser_download_url":"https://github.com/jhonatanMP/localtusk/releases/download/{tag}/LocalTusk_0.11.0_aarch64.dmg"}},
              {{"name":"LocalTusk_0.11.0_amd64.deb","browser_download_url":"https://github.com/jhonatanMP/localtusk/releases/download/{tag}/LocalTusk_0.11.0_amd64.deb"}}
            ]}}"#
        )
    }

    #[test]
    fn nova_versao_no_macos_traz_o_dmg_da_arquitetura() {
        let i = parse_release(&release("v0.11.0"), "0.10.0", "macos", "aarch64").unwrap();
        assert!(i.available);
        assert_eq!(i.latest, "0.11.0");
        assert!(i.download_url.unwrap().ends_with("LocalTusk_0.11.0_aarch64.dmg"));
        let i = parse_release(&release("v0.11.0"), "0.10.0", "macos", "x86_64").unwrap();
        assert!(i.download_url.unwrap().ends_with("LocalTusk_0.11.0_x64.dmg"));
    }

    #[test]
    fn no_linux_so_a_pagina_do_release() {
        let i = parse_release(&release("v0.11.0"), "0.10.0", "linux", "x86_64").unwrap();
        assert!(i.available);
        assert_eq!(i.download_url, None);
        assert_eq!(i.page_url, "https://github.com/jhonatanMP/localtusk/releases/tag/v0.11.0");
    }

    #[test]
    fn versao_igual_ou_mais_antiga_nao_avisa() {
        assert!(!parse_release(&release("v0.10.0"), "0.10.0", "macos", "aarch64").unwrap().available);
        assert!(!parse_release(&release("v0.9.0"), "0.10.0", "macos", "aarch64").unwrap().available);
    }

    #[test]
    fn rejeita_links_de_fora_do_repositorio_e_respostas_ruins() {
        let hostil = r#"{"tag_name":"v9.0.0","html_url":"https://evil.example/x","assets":[{"name":"a_x64.dmg","browser_download_url":"https://evil.example/a_x64.dmg"}]}"#;
        let i = parse_release(hostil, "0.10.0", "macos", "x86_64").unwrap();
        assert_eq!(i.download_url, None);
        assert_eq!(i.page_url, "https://github.com/jhonatanMP/localtusk/releases/latest");
        assert!(parse_release("{}", "0.10.0", "macos", "x86_64").is_err());
        assert!(parse_release("não é json", "0.10.0", "macos", "x86_64").is_err());
        assert!(!is_allowed_url("https://github.com.evil.example/jhonatanMP/localtusk/"));
        assert!(is_allowed_url("https://github.com/jhonatanMP/localtusk/releases"));
    }

    /// Consulta o GitHub de verdade: `cargo test --lib update::tests::consulta_real -- --ignored`.
    #[test]
    #[ignore]
    fn consulta_real() {
        let i = check().unwrap();
        println!("{i:?}");
        assert!(parse_version(&i.latest).is_some());
        assert!(i.page_url.starts_with(ALLOWED_PREFIX));
    }
}
