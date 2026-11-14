//! Ferramentas para criar APIs: o **inspetor** (requisições que chegam no PHP do projeto) e o
//! **cliente** (envia requisições HTTP e mostra a resposta).
//!
//! O inspetor funciona com um script PHP (`auto_prepend_file`) que anota cada requisição
//! tratada pelo PHP em `logs/requests.jsonl`: o que chegou (cabeçalhos e corpo) e o que o PHP
//! respondeu (status, cabeçalhos e corpo). Arquivos estáticos não passam pelo PHP e não aparecem.

use crate::project;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const REQUESTS_LOG: &str = "requests.jsonl";
/// Quanto do registro é lido na primeira vez (as requisições mais recentes).
const TAIL_BYTES: u64 = 2 * 1024 * 1024;
/// Maior resposta guardada pelo cliente.
const MAX_RESPONSE: u64 = 5 * 1024 * 1024;

pub fn log_path(base: &Path, id: &str) -> PathBuf {
    project::project_dir(base, id).join("logs").join(REQUESTS_LOG)
}

/// Literal PHP entre aspas simples.
fn php_str(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Script PHP que registra as requisições. `tools_dir` (Adminer Neo etc.) é ignorado para não
/// poluir a lista com o tráfego das ferramentas.
pub fn capture_script(log: &Path, tools_dir: &Path) -> String {
    CAPTURE_PHP
        .replace("__LOG__", &php_str(&log.display().to_string()))
        .replace("__TOOLS__", &php_str(&tools_dir.display().to_string()))
}

const CAPTURE_PHP: &str = r#"<?php
// Gerado pelo LocalTusk. Registra as requisições do projeto para a aba "API". Não edite.
(function () {
    if (PHP_SAPI === 'cli' || PHP_SAPI === 'phpdbg') {
        return;
    }
    $skip = __TOOLS__;
    $script = isset($_SERVER['SCRIPT_FILENAME']) ? $_SERVER['SCRIPT_FILENAME'] : '';
    if ($skip !== '' && strpos($script, $skip) === 0) {
        return;
    }
    $log = __LOG__;
    $limit = 65536;
    $start = microtime(true);

    // Texto (UTF-8) limitado a $limit bytes; binário vira null.
    $clip = function ($raw) use ($limit) {
        $raw = (string) $raw;
        $cut = strlen($raw) > $limit ? substr($raw, 0, $limit) : $raw;
        for ($i = 0; $i < 4 && $cut !== '' && !preg_match('//u', $cut); $i++) {
            $cut = substr($cut, 0, -1);
        }
        if (strpos($cut, "\0") !== false || ($cut !== '' && !preg_match('//u', $cut))) {
            return [null, false];
        }
        return [$cut, strlen($raw) > $limit];
    };

    $reqHeaders = [];
    if (function_exists('getallheaders')) {
        foreach (getallheaders() as $k => $v) {
            $reqHeaders[] = [(string) $k, (string) $v];
        }
    }
    $body = '';
    $in = @fopen('php://input', 'rb');
    if ($in) {
        $body = (string) stream_get_contents($in, $limit + 1);
        fclose($in);
    }
    $reqSize = isset($_SERVER['CONTENT_LENGTH']) ? (int) $_SERVER['CONTENT_LENGTH'] : strlen($body);
    $type = isset($_SERVER['CONTENT_TYPE']) ? $_SERVER['CONTENT_TYPE'] : '';
    $form = null;
    $files = [];
    if (stripos($type, 'multipart/form-data') === 0) {
        $form = (object) $_POST;
        foreach ($_FILES as $field => $f) {
            $files[] = [
                'field' => (string) $field,
                'name' => is_array($f['name']) ? implode(', ', $f['name']) : (string) $f['name'],
                'size' => is_array($f['size']) ? array_sum($f['size']) : (int) $f['size'],
            ];
        }
    }

    $out = '';
    $bytes = 0;
    ob_start(function ($chunk, $phase) use (&$out, &$bytes, $limit, $log, $start, $reqHeaders, $body, $reqSize, $form, $files, $clip) {
        $bytes += strlen($chunk);
        if (strlen($out) < $limit + 4) {
            $out .= substr($chunk, 0, $limit + 4 - strlen($out));
        }
        if ($phase & PHP_OUTPUT_HANDLER_FINAL) {
            $resHeaders = [];
            foreach (headers_list() as $line) {
                $pos = strpos($line, ':');
                if ($pos !== false) {
                    $resHeaders[] = [substr($line, 0, $pos), ltrim(substr($line, $pos + 1))];
                }
            }
            list($reqText, $reqCut) = $clip($body);
            list($resText, $resCut) = $clip($out);
            if ($bytes > $limit) {
                $resCut = true;
            }
            $entry = [
                'time' => $start,
                'method' => isset($_SERVER['REQUEST_METHOD']) ? $_SERVER['REQUEST_METHOD'] : '',
                'uri' => isset($_SERVER['REQUEST_URI']) ? $_SERVER['REQUEST_URI'] : '',
                'host' => isset($_SERVER['HTTP_HOST']) ? $_SERVER['HTTP_HOST'] : '',
                'remote' => isset($_SERVER['REMOTE_ADDR']) ? $_SERVER['REMOTE_ADDR'] : '',
                'requestHeaders' => $reqHeaders,
                'requestBody' => $reqText,
                'requestBodyTruncated' => $reqCut,
                'requestSize' => $reqSize,
                'form' => $form,
                'files' => $files,
                'status' => http_response_code() ?: 200,
                'responseHeaders' => $resHeaders,
                'responseBody' => $resText,
                'responseBodyTruncated' => $resCut,
                'responseSize' => $bytes,
                'durationMs' => round((microtime(true) - $start) * 1000, 1),
            ];
            $json = json_encode($entry, JSON_INVALID_UTF8_SUBSTITUTE | JSON_PARTIAL_OUTPUT_ON_ERROR | JSON_UNESCAPED_UNICODE | JSON_UNESCAPED_SLASHES);
            if ($json !== false) {
                if (@filesize($log) > 5 * 1024 * 1024) {
                    @rename($log, $log . '.1');
                }
                @file_put_contents($log, $json . "\n", FILE_APPEND | LOCK_EX);
            }
        }
        return false; // a saída segue para o navegador sem alterações
    }, 16384);
})();
"#;

// ------------------------------------------------------------------ Inspetor

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestsChunk {
    /// Requisições novas (cada uma é o objeto JSON gravado pelo script PHP).
    pub entries: Vec<serde_json::Value>,
    /// Posição para pedir só o que vier depois.
    pub offset: u64,
}

/// Lê as requisições registradas a partir de `offset` (0 = as mais recentes).
pub fn read_requests(base: &Path, id: &str, offset: u64) -> Result<RequestsChunk, String> {
    let path = log_path(base, id);
    let Ok(meta) = fs::metadata(&path) else {
        return Ok(RequestsChunk { entries: vec![], offset: 0 });
    };
    let len = meta.len();
    // Offset maior que o arquivo: o registro foi limpo ou rotacionado.
    let mut start = if offset > len { 0 } else { offset };
    let mut skip_partial = false;
    if start == 0 && len > TAIL_BYTES {
        start = len - TAIL_BYTES;
        skip_partial = true; // começou no meio de uma linha
    }

    let mut file = File::open(&path).map_err(|e| e.to_string())?;
    file.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(16 * 1024 * 1024).read_to_end(&mut bytes).map_err(|e| e.to_string())?;

    // Só consome linhas completas; o resto é lido na próxima vez.
    let Some(end) = bytes.iter().rposition(|b| *b == b'\n') else {
        return Ok(RequestsChunk { entries: vec![], offset: start });
    };
    let entries = bytes[..end]
        .split(|b| *b == b'\n')
        .skip(usize::from(skip_partial))
        .filter_map(|line| serde_json::from_slice::<serde_json::Value>(line).ok())
        .collect();
    Ok(RequestsChunk { entries, offset: start + end as u64 + 1 })
}

/// Apaga as requisições registradas.
// ------------------------------------------------------------------ coleção

pub const COLLECTION_FILE: &str = "collection.json";
/// Maior arquivo de coleção aceito na importação.
const MAX_COLLECTION: u64 = 5 * 1024 * 1024;

pub fn collection_path(base: &Path, id: &str) -> PathBuf {
    project::project_dir(base, id).join(COLLECTION_FILE)
}

/// Confere se o texto é uma coleção válida: `{"requests": [...]}`.
fn validate_collection(text: &str) -> Result<(), String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("arquivo de coleção inválido: {e}"))?;
    match v.get("requests") {
        Some(serde_json::Value::Array(_)) => Ok(()),
        _ => Err("arquivo de coleção inválido: faltou a lista \"requests\"".into()),
    }
}

/// Coleção salva do projeto (texto JSON); vazio se ainda não existe.
pub fn load_collection(base: &Path, id: &str) -> Result<String, String> {
    match fs::read_to_string(collection_path(base, id)) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.to_string()),
    }
}

/// Grava a coleção do projeto (de forma atômica, para não corromper se o app fechar no meio).
pub fn save_collection(base: &Path, id: &str, content: &str) -> Result<(), String> {
    validate_collection(content)?;
    let path = collection_path(base, id);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, content).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// Copia a coleção do projeto para um arquivo escolhido pelo usuário.
pub fn export_collection(base: &Path, id: &str, dest: &Path) -> Result<(), String> {
    let text = load_collection(base, id)?;
    let text = if text.is_empty() { "{\"format\":\"localtusk-collection\",\"version\":1,\"requests\":[]}".to_string() } else { text };
    fs::write(dest, text).map_err(|e| format!("{}: {e}", dest.display()))
}

/// Lê um arquivo de coleção para importar (valida o tamanho e o formato).
pub fn read_collection_file(path: &Path) -> Result<String, String> {
    let meta = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.len() > MAX_COLLECTION {
        return Err("arquivo grande demais para ser uma coleção (limite de 5 MB)".into());
    }
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    validate_collection(&text)?;
    Ok(text)
}

pub fn clear_requests(base: &Path, id: &str) -> Result<(), String> {
    let path = log_path(base, id);
    if path.exists() {
        File::create(&path).map_err(|e| e.to_string())?;
    }
    let _ = fs::remove_file(format!("{}.1", path.display()));
    Ok(())
}

// ------------------------------------------------------------------ Cliente

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiRequest {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub follow_redirects: bool,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiResponse {
    pub status: u16,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    /// Corpo como texto (vazio se for binário).
    pub body: String,
    pub binary: bool,
    /// O corpo passou do limite e foi cortado.
    pub truncated: bool,
    pub size: u64,
    pub duration_ms: u64,
}

fn friendly(e: ureq::Error) -> String {
    let text = e.to_string();
    // Pelo tipo do erro (a mensagem muda de sistema para sistema; no Windows é "actively refused").
    let refused = matches!(&e, ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused);
    if refused || text.contains("Connection refused") || text.contains("actively refused") {
        "Conexão recusada. O projeto está rodando?".into()
    } else if text.to_lowercase().contains("timeout") || text.contains("timed out") {
        "Tempo esgotado esperando a resposta.".into()
    } else {
        text
    }
}

/// Envia uma requisição HTTP e devolve a resposta (qualquer status, inclusive 4xx e 5xx).
pub fn send(req: &ApiRequest) -> Result<ApiResponse, String> {
    let method = req.method.trim().to_uppercase();
    if method.is_empty() || !method.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err("método HTTP inválido".into());
    }
    let url = req.url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("a URL precisa começar com http:// ou https://".into());
    }

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(if req.follow_redirects { 10 } else { 0 })
        .max_redirects_will_error(false)
        .timeout_global(Some(Duration::from_secs(req.timeout_secs.unwrap_or(30).clamp(1, 300))))
        .build()
        .into();

    let mut builder = ureq::http::Request::builder().method(method.as_str()).uri(url);
    for (k, v) in &req.headers {
        if !k.trim().is_empty() {
            builder = builder.header(k.trim(), v.as_str());
        }
    }

    let started = Instant::now();
    let result = match req.body.as_deref().filter(|b| !b.is_empty()) {
        Some(body) => agent.run(builder.body(body.as_bytes().to_vec()).map_err(|e| e.to_string())?),
        None => agent.run(builder.body(()).map_err(|e| e.to_string())?),
    };
    let mut resp = result.map_err(friendly)?;

    let status = resp.status();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("(binário)").to_string()))
        .collect();

    let mut bytes = Vec::new();
    resp.body_mut()
        .as_reader()
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("erro ao ler a resposta: {e}"))?;
    let duration_ms = started.elapsed().as_millis() as u64;

    let truncated = bytes.len() as u64 > MAX_RESPONSE;
    bytes.truncate(MAX_RESPONSE as usize);
    let size = bytes.len() as u64;
    let (body, binary) = match String::from_utf8(bytes) {
        Ok(text) => (text, false),
        Err(_) => (String::new(), true),
    };

    Ok(ApiResponse {
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or("").to_string(),
        headers,
        body,
        binary,
        truncated,
        size,
        duration_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    #[test]
    fn colecao_salva_exporta_e_importa() {
        let dir = std::env::temp_dir().join(format!("localtusk-colecao-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        assert_eq!(load_collection(&dir, "p1").unwrap(), "");
        assert!(save_collection(&dir, "p1", "não é json").is_err());
        assert!(save_collection(&dir, "p1", r#"{"requests": 1}"#).is_err());

        let doc = r#"{"format":"localtusk-collection","version":1,"requests":[{"id":"a","name":"Listar"}]}"#;
        save_collection(&dir, "p1", doc).unwrap();
        assert_eq!(load_collection(&dir, "p1").unwrap(), doc);
        assert!(!collection_path(&dir, "p1").with_extension("json.tmp").exists());

        let out = dir.join("saida.json");
        export_collection(&dir, "p1", &out).unwrap();
        assert_eq!(read_collection_file(&out).unwrap(), doc);

        // exportar sem nada salvo gera uma coleção vazia e válida
        export_collection(&dir, "vazio", &out).unwrap();
        assert!(read_collection_file(&out).unwrap().contains("\"requests\":[]"));

        fs::write(&out, "{}").unwrap();
        assert!(read_collection_file(&out).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn script_embute_caminhos_com_escape() {
        let s = capture_script(Path::new("/tmp/it's/logs/requests.jsonl"), Path::new("/tmp/tools"));
        assert!(s.contains(r"$log = '/tmp/it\'s/logs/requests.jsonl';"));
        assert!(s.contains("$skip = '/tmp/tools';"));
        assert!(!s.contains("__LOG__") && !s.contains("__TOOLS__"));
    }

    #[test]
    fn le_so_linhas_completas_e_acompanha_o_offset() {
        let base = std::env::temp_dir().join(format!("localtusk-test-reqs-{}", project::now()));
        let path = log_path(&base, "p");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Duas requisições completas e uma ainda sendo gravada (sem \n no fim).
        fs::write(&path, "{\"uri\":\"/a\"}\n{\"uri\":\"/b\"}\n{\"uri\":\"/c").unwrap();

        let first = read_requests(&base, "p", 0).unwrap();
        assert_eq!(first.entries.len(), 2);
        assert_eq!(first.entries[1]["uri"], "/b");

        // Sem novidades: nada novo, mesma posição.
        let again = read_requests(&base, "p", first.offset).unwrap();
        assert!(again.entries.is_empty() && again.offset == first.offset);

        // A linha termina de ser gravada.
        fs::write(&path, "{\"uri\":\"/a\"}\n{\"uri\":\"/b\"}\n{\"uri\":\"/c\"}\n").unwrap();
        let next = read_requests(&base, "p", first.offset).unwrap();
        assert_eq!(next.entries.len(), 1);
        assert_eq!(next.entries[0]["uri"], "/c");

        // Limpar zera; um offset antigo (maior que o arquivo) recomeça do início.
        clear_requests(&base, "p").unwrap();
        fs::write(&path, "{\"uri\":\"/x\"}\n").unwrap();
        let after = read_requests(&base, "p", next.offset).unwrap();
        assert_eq!(after.entries.len(), 1);
        assert_eq!(after.entries[0]["uri"], "/x");
        let _ = fs::remove_dir_all(&base);
    }

    /// Lê a requisição inteira: cabeçalhos e, se houver `Content-Length`, o corpo. O cliente pode
    /// mandar cabeçalhos e corpo em pacotes separados, então um único `read` não basta.
    fn read_request(s: &mut std::net::TcpStream) -> String {
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = s.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            data.extend_from_slice(&buf[..n]);
            let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
            let head = String::from_utf8_lossy(&data[..end]).to_lowercase();
            let want = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()))
                .unwrap_or(0);
            if data.len() >= end + 4 + want {
                break;
            }
        }
        String::from_utf8_lossy(&data).to_string()
    }

    /// Servidor HTTP mínimo: devolve a requisição recebida no corpo, com o status pedido na URL.
    fn echo_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(5) {
                let mut s = stream.unwrap();
                let req = read_request(&mut s);
                let (status, extra) = if req.starts_with("GET /missing") {
                    ("404 Not Found", "")
                } else if req.starts_with("GET /redirect") {
                    ("302 Found", "Location: /final\r\n")
                } else {
                    ("200 OK", "")
                };
                let _ = write!(
                    s,
                    "HTTP/1.1 {status}\r\n{extra}Content-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{req}",
                    req.len()
                );
            }
        });
        port
    }

    fn request(method: &str, url: String) -> ApiRequest {
        ApiRequest { method: method.into(), url, headers: vec![], body: None, follow_redirects: false, timeout_secs: Some(10) }
    }

    #[test]
    fn cliente_envia_metodo_cabecalhos_e_corpo() {
        let port = echo_server();
        let mut r = request("post", format!("http://127.0.0.1:{port}/api?x=1"));
        r.headers = vec![("X-Teste".into(), "ola".into()), ("Content-Type".into(), "application/json".into())];
        r.body = Some(r#"{"nome":"Ana"}"#.into());
        let resp = send(&r).unwrap();
        assert_eq!(resp.status, 200);
        assert!(resp.body.starts_with("POST /api?x=1 HTTP/1.1"), "{}", resp.body);
        assert!(resp.body.to_lowercase().contains("x-teste: ola"));
        assert!(resp.body.contains(r#"{"nome":"Ana"}"#));
        assert!(resp.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")));
    }

    #[test]
    fn cliente_devolve_4xx_e_nao_segue_redirecionamento() {
        let port = echo_server();
        let resp = send(&request("GET", format!("http://127.0.0.1:{port}/missing"))).unwrap();
        assert_eq!((resp.status, resp.status_text.as_str()), (404, "Not Found"));

        let resp = send(&request("GET", format!("http://127.0.0.1:{port}/redirect"))).unwrap();
        assert_eq!(resp.status, 302);
        assert!(resp.headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("location") && v == "/final"));
    }

    #[test]
    fn cliente_recusa_entradas_invalidas_e_explica_conexao_recusada() {
        assert!(send(&request("GE T", "http://x".into())).is_err());
        assert!(send(&request("GET", "ftp://x".into())).is_err());
        // Porta fechada. A porta recém-liberada pode ser reaproveitada por outro teste
        // em paralelo (já aconteceu no Windows); tenta algumas até uma recusar de fato.
        let mut err = String::new();
        for _ in 0..5 {
            let port = {
                let l = TcpListener::bind("127.0.0.1:0").unwrap();
                l.local_addr().unwrap().port()
            };
            err = send(&request("GET", format!("http://127.0.0.1:{port}/"))).unwrap_err();
            if err.contains("Conexão recusada") {
                break;
            }
        }
        assert!(err.contains("Conexão recusada"), "{err}");
    }
}
