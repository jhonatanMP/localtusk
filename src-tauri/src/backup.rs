//! Backup e restauração de projetos.
//!
//! Um backup é um `.tar.gz` em `~/LocalTusk/backups/` com:
//! - `manifest.json`: nome e configurações do projeto;
//! - `database.sql`: dump do banco (`mysqldump` ou `pg_dump`, conforme o banco do projeto);
//! - `app/`: os arquivos do site.

use crate::download::InstallProgress;
use crate::project::{self, DB_PASSWORD, DB_USER, Project};
use crate::{mysql_manager, postgres_manager, runtime, wp};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const MANIFEST: &str = "manifest.json";
pub const DUMP: &str = "database.sql";
const FORMAT: u32 = 1;
pub const EXT: &str = ".tar.gz";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub format: u32,
    pub id: String,
    pub name: String,
    pub docroot: String,
    #[serde(default)]
    pub php_version: String,
    /// `mysql` ou `postgres`. Backups antigos não têm o campo e são de MySQL.
    #[serde(default)]
    pub db_engine: String,
    #[serde(default)]
    pub db_version: String,
    pub db_name: String,
    pub http_port: u16,
    pub db_port: u16,
    pub site_url: String,
    pub has_wordpress: bool,
    /// Quando o backup foi feito (segundos desde 1970).
    pub created_at: u64,
    /// `"blueprint"` quando o arquivo é um modelo de projeto; vazio num backup comum.
    #[serde(default)]
    pub kind: String,
    /// Nome dado ao blueprint (só em blueprints; `name` continua sendo o do projeto de origem).
    #[serde(default)]
    pub blueprint_name: String,
    #[serde(default)]
    pub description: String,
}

/// Nome e descrição de um blueprint, gravados no manifesto.
pub struct BlueprintMeta<'a> {
    pub name: &'a str,
    pub description: &'a str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    pub file_name: String,
    pub path: String,
    pub size: u64,
    pub created_at: u64,
    pub project_id: String,
    pub project_name: String,
    pub has_wordpress: bool,
}

pub fn backups_dir(base: &Path) -> PathBuf {
    base.join("backups")
}

/// Atualiza a mensagem de progresso da operação `key`.
pub fn step(progress: &dyn Fn(InstallProgress), key: &str, message: &str) {
    progress(InstallProgress {
        id: key.into(),
        stage: "extract".into(),
        downloaded: 0,
        total: 0,
        message: Some(message.into()),
    });
}

/// Data e hora locais no formato `20261111-103000`.
#[cfg(unix)]
fn stamp(secs: u64) -> String {
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&t, &mut tm);
    }
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

#[cfg(not(unix))]
fn stamp(secs: u64) -> String {
    secs.to_string()
}

/// Cliente do MySQL (`mysql`, `mysqldump`) da mesma instalação do `mysqld` do projeto.
fn mysql_tool(base: &Path, p: &Project, name: &str) -> Result<PathBuf, String> {
    mysql_manager::pick(base, &p.db_version)
        .and_then(|i| i.mysqld.parent().map(|d| d.join(name)))
        .filter(|f| f.is_file())
        .ok_or_else(|| format!("O MySQL instalado não inclui o `{name}`. Instale um MySQL em \"PHP e ferramentas\"."))
}

/// Últimas linhas da saída de erro, para mensagens.
fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.contains("Using a password on the command line"))
        .collect();
    lines[lines.len().saturating_sub(6)..].join("\n")
}

/// Cliente do PostgreSQL (`psql`, `pg_dump`) da mesma instalação do servidor do projeto.
fn pg_tool(base: &Path, p: &Project, name: &str) -> Result<PathBuf, String> {
    postgres_manager::pick(base, &p.db_version)
        .and_then(|i| i.tool(name))
        .ok_or_else(|| format!("O PostgreSQL instalado não inclui o `{name}`. Instale um PostgreSQL em \"PHP e ferramentas\"."))
}

/// Comando de um cliente do PostgreSQL já ligado ao banco do projeto (TCP, senha padrão).
fn pg_command(bin: &Path, p: &Project) -> Command {
    let mut c = Command::new(bin);
    c.env("PGPASSWORD", DB_PASSWORD)
        .args(["-h", "127.0.0.1", "-p", &p.db_port.to_string(), "-U", DB_USER]);
    c
}

fn dump_postgres(base: &Path, p: &Project, out: &Path) -> Result<(), String> {
    let bin = pg_tool(base, p, "pg_dump")?;
    // `--clean --if-exists`: o dump limpa os objetos antes de recriá-los; sem dono nem
    // permissões, para restaurar em qualquer máquina.
    let output = pg_command(&bin, p)
        .args(["--no-owner", "--no-privileges", "--clean", "--if-exists", "--encoding=UTF8"])
        .arg(format!("--file={}", out.display()))
        .arg(&p.db_name)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("pg_dump: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("Falha ao exportar o banco ({})\n\n{}", output.status, tail(&output.stderr)))
    }
}

fn import_postgres(base: &Path, p: &Project, dump: &Path) -> Result<(), String> {
    let bin = pg_tool(base, p, "psql")?;
    let psql = |db: &str| {
        let mut c = pg_command(&bin, p);
        c.args(["-X", "-q", "-v", "ON_ERROR_STOP=1", "-d", db]);
        c
    };

    // Banco limpo: o conteúdo atual é substituído pelo do backup. `FORCE` derruba conexões
    // abertas (por exemplo, do PHP-FPM).
    let out = psql("postgres")
        .arg("-c")
        .arg(format!("DROP DATABASE IF EXISTS \"{db}\" WITH (FORCE)", db = p.db_name))
        .arg("-c")
        .arg(format!("CREATE DATABASE \"{db}\"", db = p.db_name))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("psql: {e}"))?;
    if !out.status.success() {
        return Err(format!("Falha ao preparar o banco ({})\n\n{}", out.status, tail(&out.stderr)));
    }

    let out = psql(&p.db_name)
        .arg("-f")
        .arg(dump)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("psql: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("Falha ao importar o banco ({})\n\n{}", out.status, tail(&out.stderr)))
    }
}

fn dump_database(base: &Path, p: &Project, out: &Path) -> Result<(), String> {
    if project::is_postgres(p) {
        return dump_postgres(base, p, out);
    }
    let bin = mysql_tool(base, p, "mysqldump")?;
    let output = Command::new(bin)
        .env("MYSQL_PWD", DB_PASSWORD)
        .arg("--host=127.0.0.1")
        .arg(format!("--port={}", p.db_port))
        .arg(format!("--user={DB_USER}"))
        .args(["--single-transaction", "--routines", "--triggers", "--events", "--default-character-set=utf8mb4"])
        .arg(format!("--result-file={}", out.display()))
        .arg(&p.db_name)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("mysqldump: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("Falha ao exportar o banco ({})\n\n{}", output.status, tail(&output.stderr)))
    }
}

/// Valores de gravação em disco do MySQL do projeto, guardados para voltar depois.
struct SavedDurability {
    flush: String,
    binlog: String,
}

fn mysql_query(base: &Path, p: &Project, q: &str) -> Option<String> {
    let bin = mysql_tool(base, p, "mysql").ok()?;
    let out = Command::new(&bin)
        .env("MYSQL_PWD", DB_PASSWORD)
        .arg("--host=127.0.0.1")
        .arg(format!("--port={}", p.db_port))
        .arg(format!("--user={DB_USER}"))
        .args(["-N", "-B", "-e", q])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Dumps com um INSERT por linha (como o do Local) e o `wp search-replace` fazem o MySQL
/// gravar no disco a cada linha, o que deixa a importação até ~15x mais lenta. Enquanto o
/// conteúdo é trazido, a gravação é adiada (seguro num ambiente local); `restore_durability`
/// volta ao que era. Melhor esforço: se algo falhar, segue no modo normal.
fn relax_durability(base: &Path, p: &Project) -> Option<SavedDurability> {
    if project::is_postgres(p) {
        return None;
    }
    let s = mysql_query(base, p, "SELECT @@GLOBAL.innodb_flush_log_at_trx_commit, @@GLOBAL.sync_binlog")?;
    let mut it = s.split_whitespace();
    let saved = SavedDurability { flush: it.next()?.to_string(), binlog: it.next()?.to_string() };
    let numeric = |v: &str| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit());
    if !numeric(&saved.flush) || !numeric(&saved.binlog) {
        return None;
    }
    mysql_query(base, p, "SET GLOBAL innodb_flush_log_at_trx_commit=2; SET GLOBAL sync_binlog=0;")?;
    Some(saved)
}

fn restore_durability(base: &Path, p: &Project, saved: Option<SavedDurability>) {
    if let Some(s) = saved {
        let _ = mysql_query(
            base,
            p,
            &format!(
                "SET GLOBAL innodb_flush_log_at_trx_commit={}; SET GLOBAL sync_binlog={};",
                s.flush, s.binlog
            ),
        );
    }
}

fn import_database(base: &Path, p: &Project, dump: &Path) -> Result<(), String> {
    if project::is_postgres(p) {
        return import_postgres(base, p, dump);
    }
    let bin = mysql_tool(base, p, "mysql")?;
    let client = || {
        let mut c = Command::new(&bin);
        c.env("MYSQL_PWD", DB_PASSWORD)
            .arg("--host=127.0.0.1")
            .arg(format!("--port={}", p.db_port))
            .arg(format!("--user={DB_USER}"))
            .arg("--default-character-set=utf8mb4");
        c
    };

    // Banco limpo: o conteúdo atual é substituído pelo do backup.
    let out = client()
        .arg("-e")
        .arg(format!("DROP DATABASE IF EXISTS `{db}`; CREATE DATABASE `{db}`;", db = p.db_name))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("mysql: {e}"))?;
    if !out.status.success() {
        return Err(format!("Falha ao preparar o banco ({})\n\n{}", out.status, tail(&out.stderr)));
    }

    let input = File::open(dump).map_err(|e| format!("{}: {e}", dump.display()))?;
    let out = client()
        .arg(&p.db_name)
        .stdin(Stdio::from(input))
        .output()
        .map_err(|e| format!("mysql: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("Falha ao importar o banco ({})\n\n{}", out.status, tail(&out.stderr)))
    }
}

/// Cria um backup do projeto (o banco precisa estar rodando). `kind` entra no nome do
/// arquivo (ex.: `antes-do-restore`); vazio para um backup comum.
pub fn create(
    base: &Path,
    p: &Project,
    kind: &str,
    key: &str,
    progress: &dyn Fn(InstallProgress),
) -> Result<PathBuf, String> {
    let dir = backups_dir(base);
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let now = project::now();
    let prefix = if kind.is_empty() { p.id.clone() } else { format!("{}-{kind}", p.id) };
    let mut target = dir.join(format!("{prefix}-{}{EXT}", stamp(now)));
    let mut n = 2;
    while target.exists() {
        target = dir.join(format!("{prefix}-{}-{n}{EXT}", stamp(now)));
        n += 1;
    }
    write_archive(base, p, &target, None, key, progress)
}

/// Exporta o banco e compacta os arquivos do projeto em `target` (backup ou blueprint,
/// conforme `blueprint`). O banco precisa estar rodando. Grava em `.part` e só então renomeia.
pub fn write_archive(
    base: &Path,
    p: &Project,
    target: &Path,
    blueprint: Option<BlueprintMeta<'_>>,
    key: &str,
    progress: &dyn Fn(InstallProgress),
) -> Result<PathBuf, String> {
    let now = project::now();
    let target = target.to_path_buf();
    let work = project::project_dir(base, &p.id).join("run").join("backup-tmp");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;

    let result = (|| -> Result<(), String> {
        step(progress, key, "Exportando o banco de dados…");
        let dump = work.join(DUMP);
        dump_database(base, p, &dump)?;

        step(progress, key, "Compactando os arquivos…");
        let manifest = Manifest {
            format: FORMAT,
            id: p.id.clone(),
            name: p.name.clone(),
            docroot: p.docroot.clone(),
            php_version: p.php_version.clone(),
            db_engine: project::engine_of(p).into(),
            db_version: p.db_version.clone(),
            db_name: p.db_name.clone(),
            http_port: p.http_port,
            db_port: p.db_port,
            site_url: project::site_url(p),
            has_wordpress: wp::is_wordpress(base, p),
            created_at: now,
            kind: if blueprint.is_some() { "blueprint".into() } else { String::new() },
            blueprint_name: blueprint.as_ref().map(|b| b.name.to_string()).unwrap_or_default(),
            description: blueprint.as_ref().map(|b| b.description.to_string()).unwrap_or_default(),
        };
        let json = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;

        let part = PathBuf::from(format!("{}.part", target.display()));
        let write = || -> std::io::Result<()> {
            let gz = GzEncoder::new(File::create(&part)?, Compression::new(3));
            let mut tar = tar::Builder::new(gz);
            tar.follow_symlinks(false);
            let mut header = tar::Header::new_gnu();
            header.set_size(json.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(now);
            tar.append_data(&mut header, MANIFEST, json.as_slice())?;
            tar.append_path_with_name(&dump, DUMP)?;
            tar.append_dir_all("app", runtime::app_dir(base, &p.id))?;
            tar.into_inner()?.finish()?;
            Ok(())
        };
        if let Err(e) = write() {
            let _ = fs::remove_file(&part);
            return Err(format!("Falha ao criar o arquivo de backup: {e}"));
        }
        fs::rename(&part, &target).map_err(|e| e.to_string())
    })();

    let _ = fs::remove_dir_all(&work);
    result.map(|()| target)
}

/// Lê o `manifest.json` de um backup (ele é o primeiro item do arquivo).
pub fn read_manifest(archive: &Path) -> Result<Manifest, String> {
    let invalid = || "Este arquivo não é um backup nem um blueprint do LocalTusk.".to_string();
    let file = File::open(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    let mut ar = tar::Archive::new(GzDecoder::new(file));
    let entries = ar.entries().map_err(|_| invalid())?;
    for entry in entries.take(3) {
        let entry = entry.map_err(|_| invalid())?;
        if entry.path().map(|p| p == Path::new(MANIFEST)).unwrap_or(false) {
            let mut text = String::new();
            entry.take(1 << 20).read_to_string(&mut text).map_err(|_| invalid())?;
            let m: Manifest = serde_json::from_str(&text).map_err(|_| invalid())?;
            if m.format > FORMAT {
                return Err("Este arquivo foi feito por uma versão mais nova do LocalTusk.".into());
            }
            return Ok(m);
        }
    }
    Err(invalid())
}

pub fn info(path: &Path) -> Result<BackupInfo, String> {
    let m = read_manifest(path)?;
    let meta = fs::metadata(path).map_err(|e| e.to_string())?;
    Ok(BackupInfo {
        file_name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        path: path.display().to_string(),
        size: meta.len(),
        created_at: m.created_at,
        project_id: m.id,
        project_name: m.name,
        has_wordpress: m.has_wordpress,
    })
}

/// Backups guardados em `~/LocalTusk/backups`, do mais novo para o mais antigo.
/// Com `project_id`, só os daquele projeto.
pub fn list(base: &Path, project_id: Option<&str>) -> Vec<BackupInfo> {
    let Ok(rd) = fs::read_dir(backups_dir(base)) else {
        return vec![];
    };
    let mut all: Vec<BackupInfo> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.to_string_lossy().ends_with(EXT))
        .filter_map(|p| info(&p).ok())
        .filter(|b| project_id.is_none_or(|id| b.project_id == id))
        .collect();
    all.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.file_name.cmp(&a.file_name)));
    all
}

/// Apaga um backup. Só aceita arquivos dentro da pasta de backups.
pub fn delete(base: &Path, path: &Path) -> Result<(), String> {
    let dir = backups_dir(base).canonicalize().map_err(|_| "pasta de backups não encontrada".to_string())?;
    let file = path.canonicalize().map_err(|_| "backup não encontrado".to_string())?;
    if !file.starts_with(&dir) || !file.to_string_lossy().ends_with(EXT) {
        return Err("este arquivo não está na pasta de backups".into());
    }
    fs::remove_file(file).map_err(|e| e.to_string())
}

/// Esvazia uma pasta sem apagá-la (links simbólicos são removidos sem seguir o destino).
fn clear_dir(dir: &Path) -> Result<(), String> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let is_dir = fs::symlink_metadata(&path).map(|m| m.file_type().is_dir()).unwrap_or(false);
        let r = if is_dir { fs::remove_dir_all(&path) } else { fs::remove_file(&path) };
        r.map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

/// Arquivos que costumam guardar o caminho absoluto do site (Wordfence, WP Super Cache).
const PATH_FILES: [&str; 5] = [
    "wp-config.php",
    ".user.ini",
    "wordfence-waf.php",
    "wp-content/advanced-cache.php",
    "wp-content/wp-cache-config.php",
];

/// Formas do caminho da pasta pública de origem como podem estar gravadas no site: a
/// informada e, se diferente, a real (sem links). Sem barra final.
fn old_paths(public: &Path) -> Vec<String> {
    let mut out = vec![public.display().to_string().trim_end_matches('/').to_string()];
    if let Ok(real) = public.canonicalize() {
        let real = real.display().to_string().trim_end_matches('/').to_string();
        if !out.contains(&real) {
            out.push(real);
        }
    }
    out.retain(|p| !p.is_empty() && p != "/");
    out
}

/// Troca os caminhos antigos (`olds`) por `new` nos [`PATH_FILES`] de `docroot` (a cópia
/// nova; a origem nunca é tocada). Arquivos ausentes são ignorados.
fn replace_local_paths_in_files(docroot: &Path, olds: &[String], new: &Path) -> Result<(), String> {
    let new = new.display().to_string();
    let mut erros = Vec::new();
    for rel in PATH_FILES {
        let file = docroot.join(rel);
        // Só arquivos comuns (a cópia não tem links, mas por garantia não segue nenhum).
        if !fs::symlink_metadata(&file).is_ok_and(|m| m.file_type().is_file()) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        let mut out = text.clone();
        for old in olds {
            out = out.replace(old.as_str(), &new);
        }
        if out != text
            && let Err(e) = fs::write(&file, out)
        {
            erros.push(format!("{rel}: {e}"));
        }
    }
    if erros.is_empty() { Ok(()) } else { Err(erros.join("; ")) }
}

/// De onde vêm os arquivos e o banco de uma restauração.
#[derive(Clone, Copy)]
pub enum RestoreSource<'a> {
    Archive(&'a Path),
    /// Pasta pública pronta + dump SQL (MySQL). Usado pelo importador do Local.
    Folder { public: &'a Path, dump: &'a Path },
}

/// Restaura o backup (ou a pasta) dentro do projeto `p`, **substituindo** os arquivos e o
/// banco dele. O projeto precisa estar rodando. Retorna um aviso se algo secundário falhou.
/// A origem só é lida, nunca alterada.
pub fn restore_into(
    base: &Path,
    p: &Project,
    source: RestoreSource,
    key: &str,
    progress: &dyn Fn(InstallProgress),
) -> Result<Option<String>, String> {
    let is_folder = matches!(source, RestoreSource::Folder { .. });
    // O dump de um tipo de banco não entra em outro (SQL do MySQL não roda no PostgreSQL).
    let from_engine = match source {
        RestoreSource::Archive(archive) => {
            let from = read_manifest(archive)?;
            if project::valid_engine(&from.db_engine) { from.db_engine } else { project::ENGINE_MYSQL.to_string() }
        }
        RestoreSource::Folder { dump, .. } => {
            if !dump.is_file() {
                return Err(format!("O banco de dados não foi encontrado: {}.", dump.display()));
            }
            project::ENGINE_MYSQL.to_string()
        }
    };
    if from_engine != project::engine_of(p) {
        let label = |e: &str| if e == project::ENGINE_POSTGRES { "PostgreSQL" } else { "MySQL" };
        return Err(format!(
            "Este backup é de um projeto com {} e o projeto de destino usa {}. Restaure como um projeto novo.",
            label(&from_engine),
            label(project::engine_of(p))
        ));
    }

    let pdir = project::project_dir(base, &p.id);
    let app = runtime::app_dir(base, &p.id);
    let work = pdir.join("run").join("restore-tmp");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;

    let result = (|| -> Result<Option<String>, String> {
        let (dump, links_note) = match source {
            RestoreSource::Archive(archive) => {
                step(progress, key, "Restaurando os arquivos…");
                clear_dir(&app)?;
                fs::create_dir_all(&app).map_err(|e| e.to_string())?;

                let file = File::open(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
                let mut ar = tar::Archive::new(GzDecoder::new(file));
                for entry in ar.entries().map_err(|e| e.to_string())? {
                    let mut entry = entry.map_err(|e| format!("backup corrompido: {e}"))?;
                    let path = entry.path().map_err(|e| e.to_string())?.into_owned();
                    // `unpack_in` recusa caminhos que saiam da pasta de destino.
                    if path == Path::new(DUMP) {
                        entry.unpack_in(&work).map_err(|e| e.to_string())?;
                    } else if path.starts_with("app") {
                        entry.unpack_in(&pdir).map_err(|e| format!("{}: {e}", path.display()))?;
                    }
                }

                let dump = work.join(DUMP);
                if !dump.is_file() {
                    return Err("O backup não contém o banco de dados.".into());
                }
                step(progress, key, "Restaurando o banco de dados…");
                (dump, None)
            }
            RestoreSource::Folder { public, dump } => {
                step(progress, key, "Copiando os arquivos…");
                clear_dir(&app)?;
                fs::create_dir_all(&app).map_err(|e| e.to_string())?;
                // Atualiza a contagem a cada 500 arquivos ou a cada segundo (arquivos grandes).
                let ultimo = std::cell::Cell::new(std::time::Instant::now());
                let on_file = |n: u64| {
                    if n.is_multiple_of(500) || ultimo.get().elapsed() >= std::time::Duration::from_secs(1) {
                        ultimo.set(std::time::Instant::now());
                        step(progress, key, &format!("Copiando os arquivos… ({n})"));
                    }
                };
                let docroot = runtime::docroot_dir(base, p);
                let links = crate::local_import::copy_tree(public, &docroot, &on_file)?;
                // Só as pastas entram no aviso (links para arquivos também foram copiados).
                let mut notes = Vec::new();
                if !links.dirs.is_empty() {
                    notes.push(format!("Pastas que eram links no Local foram copiadas: {}.", links.dirs.join(", ")));
                }
                if !links.skipped.is_empty() {
                    notes.push(format!("Links ignorados (quebrados ou em laço): {}.", links.skipped.join(", ")));
                }
                // Antes de qualquer PHP rodar: caminhos do Local em arquivos de configuração
                // (Wordfence, WP Super Cache) passam a apontar para a cópia nova.
                if let Err(e) = replace_local_paths_in_files(&docroot, &old_paths(public), &docroot) {
                    notes.push(format!("Não foi possível trocar o caminho do Local nos arquivos do site: {e}"));
                }
                let note = (!notes.is_empty()).then(|| notes.join("\n\n"));
                step(progress, key, "Importando o banco de dados…");
                (dump.to_path_buf(), note)
            }
        };
        let saved = relax_durability(base, p);
        let imported = import_database(base, p, &dump);
        if let Err(e) = imported {
            restore_durability(base, p, saved);
            return Err(e);
        }

        let mut wp_note = None;
        if wp::is_wordpress(base, p) {
            step(progress, key, "Ajustando o WordPress ao novo endereço…");
            if let Err(e) = wp::retarget(base, p, progress) {
                let what = if is_folder { "O site foi importado" } else { "O backup foi restaurado" };
                wp_note = Some(format!("{what}, mas não foi possível ajustar o WordPress automaticamente: {e}"));
            } else if let RestoreSource::Folder { public, .. } = source {
                // Caminho da pasta no Local gravado no banco (abspath, Wordfence, upload_path…).
                let new = runtime::docroot_dir(base, p).display().to_string();
                for old in old_paths(public) {
                    if let Err(e) = wp::search_replace_all(base, p, &old, &new, progress) {
                        wp_note = Some(format!(
                            "O site foi importado, mas não foi possível trocar o caminho do Local no banco: {e}"
                        ));
                        break;
                    }
                }
            }
        }
        restore_durability(base, p, saved);
        Ok(match (links_note, wp_note) {
            (Some(l), Some(w)) => Some(format!("{l}\n\n{w}")),
            (l, w) => l.or(w),
        })
    })();

    let _ = fs::remove_dir_all(&work);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_base(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("localtusk-bk-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn troca_caminho_do_local_nos_arquivos_da_copia() {
        let base = temp_base("caminhos");
        let origem = base.join("Local Sites/snap/app/public");
        fs::create_dir_all(origem.join("wp-content")).unwrap();
        let ini = format!("auto_prepend_file = '{}/wordfence-waf.php'\n", origem.display());
        fs::write(origem.join(".user.ini"), &ini).unwrap();

        let novo = base.join("proj/app/public");
        fs::create_dir_all(novo.join("wp-content")).unwrap();
        fs::write(novo.join(".user.ini"), &ini).unwrap();
        fs::write(novo.join("wp-config.php"), format!("define('WPCACHEHOME', '{}/wp-content/plugins/wp-super-cache/');\n", origem.display())).unwrap();
        fs::write(novo.join("wp-content/advanced-cache.php"), "<?php // nada\n").unwrap();
        fs::write(novo.join("outro.php"), origem.display().to_string()).unwrap();

        replace_local_paths_in_files(&novo, &old_paths(&origem), &novo).unwrap();
        let o = origem.display().to_string();
        let n = novo.display().to_string();
        let ini_novo = fs::read_to_string(novo.join(".user.ini")).unwrap();
        assert_eq!(ini_novo, format!("auto_prepend_file = '{n}/wordfence-waf.php'\n"));
        let cfg = fs::read_to_string(novo.join("wp-config.php")).unwrap();
        assert!(cfg.contains(&format!("'{n}/wp-content/plugins/wp-super-cache/'")) && !cfg.contains(&o));
        assert_eq!(fs::read_to_string(novo.join("wp-content/advanced-cache.php")).unwrap(), "<?php // nada\n");
        // Só os arquivos conhecidos são tocados; a origem fica igual.
        assert_eq!(fs::read_to_string(novo.join("outro.php")).unwrap(), o);
        assert_eq!(fs::read_to_string(origem.join(".user.ini")).unwrap(), ini);
    }

    #[cfg(unix)]
    #[test]
    fn caminho_antigo_inclui_o_real_sem_links() {
        // Caminho real da pasta temporária (no macOS, /var é um link para /private/var).
        let base = temp_base("real").canonicalize().unwrap();
        fs::create_dir_all(base.join("real/public")).unwrap();
        std::os::unix::fs::symlink(base.join("real"), base.join("link")).unwrap();
        let olds = old_paths(&base.join("link/public/"));
        assert_eq!(olds[0], base.join("link/public").display().to_string());
        assert!(olds.contains(&base.join("real/public").canonicalize().unwrap().display().to_string()));
        assert_eq!(old_paths(&base.join("real/public")).len(), 1);
    }
}
