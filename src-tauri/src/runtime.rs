//! Motor: gera as configs de cada projeto e controla os processos
//! (MySQL ou PostgreSQL, Redis/Valkey, PHP-FPM e Caddy) sem Docker.

use crate::extras;
use crate::mysql_manager;
use crate::php_manager;
use crate::postgres_manager;
use crate::valkey_manager;
use crate::project::{self, DB_PASSWORD, DB_USER, Project};
use crate::tools;
use std::fs::{self, File};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct Service {
    pub name: &'static str,
    /// Arquivo de log (em `logs/`) usado para explicar falhas.
    pub log: &'static str,
    pub child: Child,
    /// Para com SIGINT em vez de SIGTERM. O PostgreSQL trata SIGTERM como "desligamento
    /// inteligente" e espera todos os clientes saírem; SIGINT desliga rápido e com segurança.
    pub fast_stop: bool,
}

/// O que ficou disponível quando o projeto foi iniciado.
#[derive(Clone, Default)]
pub struct RunInfo {
    /// Porta do site de administração do banco (Adminer Neo).
    pub admin_port: Option<u16>,
    pub adminneo: bool,
    /// Porta da interface web do Mailpit.
    pub mail_ui_port: Option<u16>,
}

pub struct Running {
    pub services: Vec<Service>,
    pub info: RunInfo,
}

impl Running {
    /// `true` se todos os serviços continuam vivos.
    pub fn is_alive(&mut self) -> bool {
        self.services
            .iter_mut()
            .all(|s| matches!(s.child.try_wait(), Ok(None)))
    }
}

struct Layout {
    app: PathBuf,
    conf: PathBuf,
    logs: PathBuf,
    run: PathBuf,
    mysql_data: PathBuf,
    pg_data: PathBuf,
    redis_data: PathBuf,
}

impl Layout {
    fn new(base: &Path, p: &Project) -> Self {
        let root = project::project_dir(base, &p.id);
        Layout {
            app: root.join("app"),
            conf: root.join("conf"),
            logs: root.join("logs"),
            run: root.join("run"),
            mysql_data: root.join("data").join("mysql"),
            pg_data: root.join("data").join("postgres"),
            redis_data: root.join("data").join("redis"),
        }
    }

    fn docroot(&self, p: &Project) -> PathBuf {
        if p.docroot.is_empty() {
            self.app.clone()
        } else {
            self.app.join(&p.docroot)
        }
    }

    /// Sockets Unix têm limite de ~104 caracteres no macOS; se o caminho do
    /// projeto for longo, o socket do MySQL vai para `/tmp`.
    fn mysql_socket(&self, p: &Project) -> PathBuf {
        let preferred = self.run.join("mysql.sock");
        if preferred.as_os_str().len() < 90 {
            preferred
        } else {
            std::env::temp_dir().join(format!("localtusk-{}.sock", p.id))
        }
    }
}

/// Cria a estrutura de pastas e os arquivos de exemplo de um projeto novo.
pub fn scaffold(base: &Path, p: &Project) -> Result<(), String> {
    let l = Layout::new(base, p);
    for d in [&l.app, &l.conf, &l.logs, &l.run, &l.mysql_data.parent().unwrap().to_path_buf()] {
        fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let docroot = l.docroot(p);
    fs::create_dir_all(&docroot).map_err(|e| e.to_string())?;
    let index = docroot.join("index.php");
    if !index.exists() {
        fs::write(&index, SAMPLE_INDEX).map_err(|e| e.to_string())?;
    }
    Ok(())
}

const SAMPLE_INDEX: &str = r#"<?php
$db = ['host' => getenv('DB_HOST'), 'port' => getenv('DB_PORT'), 'name' => getenv('DB_NAME'),
       'user' => getenv('DB_USER'), 'pass' => getenv('DB_PASSWORD')];
$postgres = getenv('DB_ENGINE') === 'postgres';
try {
    if ($postgres) {
        $pdo = new PDO("pgsql:host={$db['host']};port={$db['port']};dbname={$db['name']}", $db['user'], $db['pass']);
        $dbStatus = 'conectado (PostgreSQL ' . $pdo->query('SHOW server_version')->fetchColumn() . ')';
    } else {
        $pdo = new PDO("mysql:host={$db['host']};port={$db['port']};dbname={$db['name']}", $db['user'], $db['pass']);
        $dbStatus = 'conectado (MySQL ' . $pdo->query('SELECT VERSION()')->fetchColumn() . ')';
    }
} catch (Throwable $e) {
    $dbStatus = 'erro: ' . $e->getMessage();
}
$redisStatus = null;
if (getenv('REDIS_HOST')) {
    try {
        $redis = new Redis();
        $redis->connect(getenv('REDIS_HOST'), (int) getenv('REDIS_PORT'));
        $info = $redis->info();
        $redisStatus = 'conectado (Valkey ' . ($info['valkey_version'] ?? $info['redis_version'] ?? '?') . ')';
    } catch (Throwable $e) {
        $redisStatus = 'erro: ' . $e->getMessage();
    }
}
?><!doctype html>
<html lang="pt-br"><head><meta charset="utf-8"><title>LocalTusk</title>
<style>body{font:16px system-ui;max-width:640px;margin:10vh auto;padding:0 1rem;color:#222}
code{background:#eee;padding:2px 6px;border-radius:4px}</style></head>
<body>
<h1>🚀 Seu projeto está rodando no LocalTusk</h1>
<p>PHP <strong><?= PHP_VERSION ?></strong> &middot; Banco: <strong><?= htmlspecialchars($dbStatus) ?></strong><?php if ($redisStatus !== null): ?> &middot; Redis: <strong><?= htmlspecialchars($redisStatus) ?></strong><?php endif ?></p>
<p>Edite <code>app/<?= htmlspecialchars(basename(__DIR__) === 'app' ? '' : basename(__DIR__) . '/') ?>index.php</code> para começar.</p>
<p>Variáveis disponíveis: <code>DB_HOST</code>, <code>DB_PORT</code>, <code>DB_NAME</code>, <code>DB_USER</code>, <code>DB_PASSWORD</code> e, com o Redis ligado, <code>REDIS_HOST</code> e <code>REDIS_PORT</code>.</p>
</body></html>
"#;

fn write(path: &Path, content: String) -> Result<(), String> {
    fs::write(path, content).map_err(|e| format!("{}: {e}", path.display()))
}

fn q(path: &Path) -> String {
    path.display().to_string()
}

struct MailCfg {
    bin: PathBuf,
    web: u16,
    smtp: u16,
}

/// Ferramentas extras ativas neste start.
struct Extras {
    tools_dir: PathBuf,
    admin_port: Option<u16>,
    mail: Option<MailCfg>,
}

fn write_configs(l: &Layout, p: &Project, fpm_port: u16, x: &Extras) -> Result<(), String> {
    let socket = l.mysql_socket(p);
    let postgres = project::is_postgres(p);

    // --- MySQL ---
    if !postgres {
    write(
        &l.conf.join("my.cnf"),
        format!(
            "[mysqld]\n\
             datadir={data}\n\
             socket={sock}\n\
             pid-file={run}/mysql.pid\n\
             log-error={logs}/mysql.log\n\
             port={port}\n\
             bind-address=127.0.0.1\n\
             mysqlx=OFF\n\
             skip-name-resolve\n\
             performance_schema=OFF\n\
             max_connections=50\n\
             innodb_buffer_pool_size=128M\n",
            data = q(&l.mysql_data),
            sock = q(&socket),
            run = q(&l.run),
            logs = q(&l.logs),
            port = p.db_port,
        ),
    )?;
    write(
        &l.conf.join("mysql-init.sql"),
        format!(
            "ALTER USER 'root'@'localhost' IDENTIFIED BY '{pw}';\n\
             CREATE DATABASE IF NOT EXISTS `{db}`;\n\
             CREATE USER IF NOT EXISTS '{u}'@'%' IDENTIFIED BY '{pw}';\n\
             GRANT ALL PRIVILEGES ON *.* TO '{u}'@'%' WITH GRANT OPTION;\n",
            pw = DB_PASSWORD,
            u = DB_USER,
            db = p.db_name
        ),
    )?;
    }

    // --- Redis (Valkey) ---
    if project::redis_on(p) {
        write(&l.conf.join("valkey.conf"), render_valkey_conf(p, &q(&l.redis_data)))?;
    }

    // --- PHP ---
    // PHP estático não conhece o repositório de certificados do sistema.
    let cacert = ["/etc/ssl/cert.pem", "/etc/ssl/certs/ca-certificates.crt", "/etc/pki/tls/certs/ca-bundle.crt"]
        .into_iter()
        .find(|p| Path::new(p).is_file())
        .map(|p| format!("curl.cainfo={p}\nopenssl.cafile={p}\n"))
        .unwrap_or_default();
    // Com o Mailpit instalado, mail() do PHP entrega na caixa de testes do projeto.
    let mail_ini = x
        .mail
        .as_ref()
        .map(|m| {
            format!(
                "sendmail_path=\"'{}' sendmail -S 127.0.0.1:{}\"\n",
                m.bin.display(),
                m.smtp
            )
        })
        .unwrap_or_default();
    // Registra as requisições PHP do projeto para a aba "API" (inspetor).
    let capture = l.conf.join("capture.php");
    write(
        &capture,
        crate::apitools::capture_script(&l.logs.join(crate::apitools::REQUESTS_LOG), &x.tools_dir),
    )?;
    write(
        &l.conf.join("php.ini"),
        format!(
            "{cacert}{mail_ini}\
             auto_prepend_file={capture}\n\
             memory_limit=512M\n\
             upload_max_filesize=64M\n\
             post_max_size=64M\n\
             max_execution_time=120\n\
             display_errors=On\n\
             error_reporting=E_ALL\n\
             date.timezone=UTC\n\
             error_log={logs}/php-error.log\n\
             pdo_mysql.default_socket={sock}\n\
             mysqli.default_socket={sock}\n\
             mysqli.default_port={port}\n",
            logs = q(&l.logs),
            sock = q(&socket),
            port = if postgres { 3306 } else { p.db_port },
            cacert = cacert,
            mail_ini = mail_ini,
            capture = q(&capture)
        ),
    )?;
    let mut env_lines = format!(
        "env[DB_HOST] = 127.0.0.1\n\
         env[DB_PORT] = {}\n\
         env[DB_NAME] = {}\n\
         env[DB_USER] = {}\n\
         env[DB_PASSWORD] = {}\n\
         env[DB_ENGINE] = {}\n\
         env[DB_CONNECTION] = {}\n",
        p.db_port,
        p.db_name,
        DB_USER,
        DB_PASSWORD,
        project::engine_of(p),
        if postgres { "pgsql" } else { "mysql" },
    );
    if project::redis_on(p) {
        env_lines.push_str(&format!(
            "env[REDIS_HOST] = 127.0.0.1\n\
             env[REDIS_PORT] = {port}\n\
             env[REDIS_URL] = redis://127.0.0.1:{port}\n",
            port = p.redis_port
        ));
    }
    if let Some(m) = &x.mail {
        env_lines.push_str(&format!(
            "env[MAIL_HOST] = 127.0.0.1\n\
             env[MAIL_PORT] = {smtp}\n\
             env[SMTP_HOST] = 127.0.0.1\n\
             env[SMTP_PORT] = {smtp}\n",
            smtp = m.smtp
        ));
    }
    write(
        &l.conf.join("php-fpm.conf"),
        format!(
            "[global]\n\
             pid = {run}/php-fpm.pid\n\
             error_log = {logs}/php-fpm.log\n\
             daemonize = no\n\n\
             [www]\n\
             listen = 127.0.0.1:{fpm}\n\
             pm = ondemand\n\
             pm.max_children = 8\n\
             pm.process_idle_timeout = 10s\n\
             catch_workers_output = yes\n\
             clear_env = no\n\
             {env_lines}",
            run = q(&l.run),
            logs = q(&l.logs),
            fpm = fpm_port,
        ),
    )?;

    // --- Caddy ---
    let caddyfile = render_caddyfile(
        p,
        &q(&l.docroot(p)),
        &q(&l.logs),
        fpm_port,
        x.admin_port.map(|port| (port, q(&x.tools_dir))),
        &project::lan_hosts(),
    );
    write(&l.conf.join("Caddyfile"), caddyfile)
}

/// Configuração do Valkey do projeto: só escuta em 127.0.0.1 (nunca na rede), grava um
/// instantâneo em disco de vez em quando e mantém a linha de comando original no `ps`
/// (assim o app reconhece e encerra um Valkey órfão pela pasta do projeto).
fn render_valkey_conf(p: &Project, data: &str) -> String {
    format!(
        "bind 127.0.0.1\n\
         port {port}\n\
         protected-mode yes\n\
         daemonize no\n\
         set-proc-title no\n\
         dir \"{data}\"\n\
         dbfilename dump.rdb\n\
         save 900 1\n\
         save 300 10\n\
         save 60 10000\n\
         appendonly no\n",
        port = p.redis_port
    )
}

/// Caddyfile do projeto.
///
/// - HTTP em `:<porta>` (aceita qualquer nome de host, então também serve o IP da rede).
/// - Com HTTPS ligado, uma segunda porta com `tls internal` (certificado da CA local) para
///   `<id>.localhost`, `127.0.0.1` e, com a rede ligada, o IP e o nome `.local` do computador.
/// - Só escuta em `127.0.0.1`, a menos que o acesso pela rede esteja ligado (`0.0.0.0`).
/// - O Adminer Neo (`admin`) fica sempre só em `127.0.0.1`.
fn render_caddyfile(
    p: &Project,
    docroot: &str,
    logs: &str,
    fpm: u16,
    admin: Option<(u16, String)>,
    lan_hosts: &[String],
) -> String {
    let https = project::https_on(p);
    let bind = if p.lan { "0.0.0.0" } else { "127.0.0.1" };

    let mut out = String::from("{\n\tadmin off\n");
    if https {
        // Mantém a emissão de certificados, mas sem redirecionar HTTP -> HTTPS (a porta 80
        // não é usada) e sem tentar instalar a CA sozinho: isso é um botão do app.
        out.push_str("\tauto_https disable_redirects\n\tskip_install_trust\n");
    } else {
        out.push_str("\tauto_https off\n");
    }
    out.push_str("}\n\n");

    out.push_str(&format!(
        "(app) {{\n\
         \troot * \"{docroot}\"\n\
         \tencode gzip\n\
         \tphp_fastcgi 127.0.0.1:{fpm}\n\
         \tfile_server\n\
         \tlog {{\n\
         \t\toutput file \"{logs}/access.log\"\n\
         \t}}\n\
         }}\n\n\
         :{port} {{\n\
         \tbind {bind}\n\
         \timport app\n\
         }}\n",
        port = p.http_port,
    ));

    if https {
        let mut hosts = vec![project::site_host(p), "127.0.0.1".to_string()];
        if p.lan {
            hosts.extend(lan_hosts.iter().cloned());
        }
        let addrs: Vec<String> = hosts.iter().map(|h| format!("https://{h}:{}", p.https_port)).collect();
        out.push_str(&format!(
            "\n{addrs} {{\n\
             \tbind {bind}\n\
             \ttls internal\n\
             \timport app\n\
             }}\n",
            addrs = addrs.join(", "),
        ));
    }

    // Site de administração do banco: a raiz é a pasta das ferramentas, então
    // /adminneo/ aponta direto para a pasta (sem reescrever URLs).
    if let Some((port, tools)) = admin {
        out.push_str(&format!(
            "\n:{port} {{\n\
             \tbind 127.0.0.1\n\
             \troot * \"{tools}\"\n\
             \t@allowed path /adminneo/*\n\
             \thandle @allowed {{\n\
             \t\tphp_fastcgi 127.0.0.1:{fpm} {{\n\
             \t\t\tenv PHP_VALUE \"display_errors=0\"\n\
             \t\t}}\n\
             \t\tfile_server\n\
             \t}}\n\
             \trespond \"Not found\" 404\n\
             }}\n",
        ));
    }
    out
}

fn log_file(l: &Layout, name: &str) -> Result<File, String> {
    File::options()
        .create(true)
        .append(true)
        .open(l.logs.join(name))
        .map_err(|e| e.to_string())
}

fn spawn(l: &Layout, mut cmd: Command, log: &str) -> Result<Child, String> {
    let out = log_file(l, log)?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    cmd.stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err))
        .spawn()
        .map_err(|e| format!("não foi possível iniciar {:?}: {e}", cmd.get_program()))
}

/// `true` se o banco do projeto já foi criado (a versão do MySQL fica "travada" a partir daí).
pub fn db_initialized(base: &Path, id: &str) -> bool {
    let data = project::project_dir(base, id).join("data");
    let mysql = data.join("mysql");
    mysql.join("mysql.ibd").exists() || mysql.join("mysql").exists() || data.join("postgres").join("PG_VERSION").exists()
}

fn mysql_initialized(l: &Layout) -> bool {
    l.mysql_data.join("mysql.ibd").exists() || l.mysql_data.join("mysql").exists()
}

fn init_mysql(l: &Layout, mysqld: &Path) -> Result<(), String> {
    fs::create_dir_all(&l.mysql_data).map_err(|e| e.to_string())?;
    let log = log_file(l, "mysql-init.log")?;
    let status = Command::new(mysqld)
        .arg(format!("--defaults-file={}", q(&l.conf.join("my.cnf"))))
        .arg("--initialize-insecure")
        .arg(format!("--init-file={}", q(&l.conf.join("mysql-init.sql"))))
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(log))
        .status()
        .map_err(|e| format!("mysqld: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        // Evita deixar um datadir pela metade, que impediria uma nova tentativa.
        let _ = fs::remove_dir_all(&l.mysql_data);
        let log = tail(&l.logs.join("mysql-init.log"), 6);
        Err(format!("falha ao inicializar o MySQL ({status})\n\n{log}"))
    }
}

fn pg_initialized(l: &Layout) -> bool {
    l.pg_data.join("PG_VERSION").exists()
}

/// Cria o diretório de dados do PostgreSQL: superusuário `root` com a senha padrão do app
/// e autenticação por senha (SCRAM) nas conexões TCP.
fn init_postgres(l: &Layout, initdb: &Path) -> Result<(), String> {
    fs::create_dir_all(&l.pg_data).map_err(|e| e.to_string())?;
    let pwfile = l.conf.join("pg-pass");
    write(&pwfile, format!("{DB_PASSWORD}\n"))?;
    let log = log_file(l, "postgres-init.log")?;

    // Prefere a coleção de textos en_US.UTF-8; se o sistema não tiver, cai para "C".
    let mut last = None;
    for locale in ["en_US.UTF-8", "C"] {
        let _ = fs::remove_dir_all(&l.pg_data);
        fs::create_dir_all(&l.pg_data).map_err(|e| e.to_string())?;
        let status = Command::new(initdb)
            .arg("-D")
            .arg(&l.pg_data)
            .args(["-U", DB_USER, "-A", "scram-sha-256", "-E", "UTF8"])
            .arg(format!("--locale={locale}"))
            .arg(format!("--pwfile={}", q(&pwfile)))
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
            .stderr(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
            .status()
            .map_err(|e| format!("initdb: {e}"))?;
        if status.success() {
            let _ = fs::remove_file(&pwfile);
            return Ok(());
        }
        last = Some(status);
    }
    // Evita deixar um diretório pela metade, que impediria uma nova tentativa.
    let _ = fs::remove_dir_all(&l.pg_data);
    let _ = fs::remove_file(&pwfile);
    let log = tail(&l.logs.join("postgres-init.log"), 6);
    Err(format!("falha ao inicializar o PostgreSQL ({})\n\n{log}", last.map(|s| s.to_string()).unwrap_or_default()))
}

/// Roda um comando SQL no PostgreSQL do projeto (via TCP, com a senha padrão).
fn psql(psql: &Path, port: u16, db: &str, sql: &str) -> Result<String, String> {
    let out = Command::new(psql)
        .env("PGPASSWORD", DB_PASSWORD)
        .args(["-h", "127.0.0.1", "-p", &port.to_string(), "-U", DB_USER, "-d", db, "-X", "-tA", "-v", "ON_ERROR_STOP=1", "-c", sql])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("psql: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Cria o banco do projeto se ainda não existir. Logo depois de abrir a porta o PostgreSQL
/// pode estar terminando de iniciar, então tenta de novo por alguns segundos.
fn ensure_pg_database(running: &mut Running, l: &Layout, psql_bin: &Path, p: &Project) -> Result<(), String> {
    let start = Instant::now();
    loop {
        let attempt = psql(
            psql_bin,
            p.db_port,
            "postgres",
            &format!("SELECT 1 FROM pg_database WHERE datname = '{}'", p.db_name),
        )
        .and_then(|found| {
            if found == "1" {
                Ok(())
            } else {
                psql(psql_bin, p.db_port, "postgres", &format!("CREATE DATABASE \"{}\"", p.db_name)).map(|_| ())
            }
        });
        match attempt {
            Ok(()) => return Ok(()),
            Err(e) => {
                for s in running.services.iter_mut() {
                    if let Ok(Some(status)) = s.child.try_wait() {
                        let log = tail(&l.logs.join(s.log), 6);
                        return Err(format!("{} encerrou inesperadamente ({status})\n\n{log}", s.name));
                    }
                }
                if start.elapsed() > Duration::from_secs(30) {
                    return Err(format!("não foi possível criar o banco no PostgreSQL: {e}"));
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    }
}

/// Últimas linhas de um log, para incluir nas mensagens de erro.
fn tail(path: &Path, n: usize) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn wait_port(port: u16, timeout: Duration, running: &mut Running, logs: &Path, service: &str) -> Result<(), String> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let start = Instant::now();
    loop {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return Ok(());
        }
        for s in running.services.iter_mut() {
            if let Ok(Some(status)) = s.child.try_wait() {
                let log = tail(&logs.join(s.log), 6);
                return Err(format!(
                    "{} encerrou inesperadamente ({status}){}",
                    s.name,
                    if log.is_empty() { String::new() } else { format!("\n\n{log}") }
                ));
            }
        }
        if start.elapsed() > timeout {
            let log = running
                .services
                .iter()
                .find(|s| s.name == service)
                .map(|s| tail(&logs.join(s.log), 6))
                .unwrap_or_default();
            return Err(format!(
                "tempo esgotado aguardando {service} na porta {port}{}",
                if log.is_empty() { String::new() } else { format!("\n\n{log}") }
            ));
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// Sobe MySQL, PHP-FPM e Caddy do projeto. Bloqueante.
pub fn start(base: &Path, p: &Project) -> Result<Running, String> {
    // Serviços de uma sessão anterior (app encerrado à força, crash...) segurariam o
    // banco e as portas do projeto.
    reap_orphans(base, &p.id);
    let l = Layout::new(base, p);
    scaffold(base, p)?;
    fs::create_dir_all(crate::ssl::data_home(base)).map_err(|e| e.to_string())?;

    let postgres = project::is_postgres(p);
    let php = tools::pick_php(base, &p.php_version)
        .ok_or("PHP não encontrado. Instale uma versão em \"PHP e ferramentas\".")?;
    // Com PostgreSQL o PHP precisa do `pdo_pgsql`: usa (e baixa, se faltar) o pacote "common".
    let php_fpm = php_manager::fpm_for(base, &php, postgres)?;
    let caddy = extras::caddy_bin(base)
        .or_else(|| tools::find("caddy", base))
        .ok_or("Caddy não encontrado. Instale em \"PHP e ferramentas\".")?;
    let mysqld = if postgres {
        PathBuf::new()
    } else {
        mysql_manager::pick(base, &p.db_version)
            .ok_or("MySQL não encontrado. Instale uma versão em \"PHP e ferramentas\".")?
            .mysqld
    };
    let pg = if postgres {
        Some(
            postgres_manager::pick(base, &p.db_version)
                .ok_or("PostgreSQL não encontrado. Instale uma versão em \"PHP e ferramentas\".")?,
        )
    } else {
        None
    };
    let valkey = if project::redis_on(p) {
        Some(
            valkey_manager::pick(base)
                .ok_or("Redis (Valkey) não encontrado. Instale-o em \"PHP e ferramentas\" ou desligue o Redis deste projeto.")?,
        )
    } else {
        None
    };

    let mut has_neo = extras::is_installed(base, "adminneo");
    if has_neo {
        extras::refresh_adminneo_files(base);
        // Instalações antigas só tinham o driver do MySQL: baixa o do PostgreSQL na primeira vez.
        if postgres && !extras::adminneo_has_pg(base) && extras::add_adminneo_pg(base).is_err() {
            has_neo = false;
        }
    }
    let mailpit = extras::mailpit_bin(base);
    let ports = project::ephemeral_ports(4).ok_or("sem portas livres")?;
    let fpm_port = ports[0];
    let x = Extras {
        tools_dir: extras::tools_root(base),
        admin_port: has_neo.then_some(ports[1]),
        mail: mailpit.map(|bin| MailCfg {
            bin,
            web: ports[2],
            smtp: ports[3],
        }),
    };

    write_configs(&l, p, fpm_port, &x)?;
    if let Some(pg) = &pg {
        if !pg_initialized(&l) {
            let initdb = pg.tool("initdb").ok_or("o PostgreSQL instalado não inclui o initdb")?;
            init_postgres(&l, &initdb)?;
        }
    } else if !mysql_initialized(&l) {
        init_mysql(&l, &mysqld)?;
    }
    if valkey.is_some() {
        fs::create_dir_all(&l.redis_data).map_err(|e| e.to_string())?;
    }

    let mut running = Running {
        services: vec![],
        info: RunInfo {
            admin_port: x.admin_port,
            adminneo: has_neo,
            mail_ui_port: x.mail.as_ref().map(|m| m.web),
        },
    };

    let result = (|| -> Result<(), String> {
        if let Some(m) = &x.mail {
            let mut c = Command::new(&m.bin);
            c.arg("--listen")
                .arg(format!("127.0.0.1:{}", m.web))
                .arg("--smtp")
                .arg(format!("127.0.0.1:{}", m.smtp))
                .arg("--database")
                .arg(l.mysql_data.parent().unwrap().join("mailpit.db"))
                .arg("--disable-version-check");
            running.services.push(Service {
                name: "Mailpit",
                log: "mailpit.log",
                child: spawn(&l, c, "mailpit.log")?,
                fast_stop: false,
            });
        }

        if let Some(pg) = &pg {
            // Só escuta em 127.0.0.1 e sem socket Unix (o limite de caminho do macOS é curto):
            // tudo conecta por TCP.
            let mut c = Command::new(&pg.postgres);
            c.arg("-D")
                .arg(&l.pg_data)
                .args(["-p", &p.db_port.to_string()])
                .args(["-c", "listen_addresses=127.0.0.1"])
                .args(["-c", "unix_socket_directories="])
                .args(["-c", "max_connections=50"])
                .args(["-c", "shared_buffers=64MB"])
                .args(["-c", "jit=off"])
                .args(["-c", "timezone=UTC"]);
            running.services.push(Service {
                name: "PostgreSQL",
                log: "postgres.log",
                child: spawn(&l, c, "postgres.log")?,
                fast_stop: true,
            });
        } else {
            let mut c = Command::new(&mysqld);
            c.arg(format!("--defaults-file={}", q(&l.conf.join("my.cnf"))));
            running.services.push(Service {
                name: "MySQL",
                log: "mysql.log",
                child: spawn(&l, c, "mysql-stdout.log")?,
                fast_stop: false,
            });
        }

        if let Some(v) = &valkey {
            let mut c = Command::new(&v.server);
            c.arg(l.conf.join("valkey.conf"));
            running.services.push(Service {
                name: "Redis",
                log: "valkey.log",
                child: spawn(&l, c, "valkey.log")?,
                fast_stop: false,
            });
        }

        let mut c = Command::new(&php_fpm);
        c.arg("-F")
            .arg("-y")
            .arg(l.conf.join("php-fpm.conf"))
            .arg("-c")
            .arg(l.conf.join("php.ini"));
        running.services.push(Service {
            name: "PHP-FPM",
            log: "php-fpm.log",
            child: spawn(&l, c, "php-fpm-stdout.log")?,
            fast_stop: false,
        });

        let mut c = Command::new(&caddy);
        c.arg("run")
            .arg("--config")
            .arg(l.conf.join("Caddyfile"))
            .arg("--adapter")
            .arg("caddyfile")
            // Dados do Caddy (a CA do HTTPS) compartilhados entre os projetos.
            .env("XDG_DATA_HOME", crate::ssl::data_home(base))
            .env("XDG_CONFIG_HOME", &l.run);
        running.services.push(Service {
            name: "Caddy",
            log: "caddy.log",
            child: spawn(&l, c, "caddy.log")?,
            fast_stop: false,
        });

        let db_name = if postgres { "PostgreSQL" } else { "MySQL" };
        wait_port(p.db_port, Duration::from_secs(90), &mut running, &l.logs, db_name)?;
        if let Some(pg) = &pg {
            let psql_bin = pg.tool("psql").ok_or("o PostgreSQL instalado não inclui o psql")?;
            ensure_pg_database(&mut running, &l, &psql_bin, p)?;
        }
        if valkey.is_some() {
            wait_port(p.redis_port, Duration::from_secs(20), &mut running, &l.logs, "Redis")?;
        }
        wait_port(fpm_port, Duration::from_secs(20), &mut running, &l.logs, "PHP-FPM")?;
        wait_port(p.http_port, Duration::from_secs(20), &mut running, &l.logs, "Caddy")?;
        if project::https_on(p) {
            wait_port(p.https_port, Duration::from_secs(20), &mut running, &l.logs, "Caddy")?;
        }
        if let Some(port) = x.admin_port {
            wait_port(port, Duration::from_secs(20), &mut running, &l.logs, "Caddy")?;
        }
        if let Some(m) = &x.mail {
            wait_port(m.web, Duration::from_secs(20), &mut running, &l.logs, "Mailpit")?;
        }
        Ok(())
    })();

    match result {
        Ok(()) => Ok(running),
        Err(e) => {
            stop(&mut running);
            Err(e)
        }
    }
}

/// Linhas `(pid, comando)` dos processos do projeto que o LocalTusk sobe
/// (mysqld, php-fpm, caddy, mailpit). Identifica pelo caminho da pasta do projeto
/// na linha de comando.
#[cfg(unix)]
pub fn project_processes(base: &Path, id: &str) -> Vec<(i32, String)> {
    let needle = format!("{}/", project::project_dir(base, id).display());
    let Ok(out) = Command::new("ps").args(["-axo", "pid=,command="]).output() else {
        return vec![];
    };
    parse_ps(&String::from_utf8_lossy(&out.stdout), &needle, std::process::id() as i32)
}

/// Filtra a listagem do `ps` (`pid comando` por linha): só mysqld, postgres, valkey-server, caddy, mailpit e php-fpm
/// cuja linha de comando cita a pasta do projeto (`needle`). Ignora o próprio processo (`me`).
fn parse_ps(listing: &str, needle: &str, me: i32) -> Vec<(i32, String)> {
    listing
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let (pid, cmd) = line.split_once(' ')?;
            let pid: i32 = pid.parse().ok()?;
            let cmd = cmd.trim_start();
            let exe = cmd.split_whitespace().next().unwrap_or("");
            let name = exe.rsplit('/').next().unwrap_or("");
            let ours = matches!(name, "mysqld" | "postgres" | "valkey-server" | "caddy" | "mailpit") || cmd.starts_with("php-fpm:");
            (pid != me && ours && cmd.contains(needle)).then(|| (pid, cmd.to_string()))
        })
        .collect()
}

/// Encerra serviços do projeto que ficaram rodando fora do controle do app.
#[cfg(unix)]
pub fn reap_orphans(base: &Path, id: &str) -> usize {
    kill_processes(&project_processes(base, id))
}

/// Sinal para encerrar o processo de uma linha de comando do `ps`: SIGINT (desligamento
/// rápido) para o PostgreSQL, SIGTERM para os demais.
#[cfg(unix)]
fn stop_signal(cmd: &str) -> i32 {
    let exe = cmd.split_whitespace().next().unwrap_or("");
    if exe.rsplit('/').next() == Some("postgres") { libc::SIGINT } else { libc::SIGTERM }
}

/// Encerra os processos informados (SIGTERM, depois SIGKILL após 20s). Retorna quantos eram.
#[cfg(unix)]
pub fn kill_processes(found: &[(i32, String)]) -> usize {
    for (pid, cmd) in found {
        unsafe {
            libc::kill(*pid, stop_signal(cmd));
        }
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    for (pid, _) in found {
        while unsafe { libc::kill(*pid, 0) } == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        if unsafe { libc::kill(*pid, 0) } == 0 {
            unsafe {
                libc::kill(*pid, libc::SIGKILL);
            }
        }
    }
    found.len()
}

#[cfg(not(unix))]
pub fn reap_orphans(_base: &Path, _id: &str) -> usize {
    0
}

#[cfg(not(unix))]
pub fn project_processes(_base: &Path, _id: &str) -> Vec<(i32, String)> {
    vec![]
}

#[cfg(not(unix))]
pub fn kill_processes(_found: &[(i32, String)]) -> usize {
    0
}

#[cfg(unix)]
fn terminate(child: &Child, fast: bool) {
    unsafe {
        libc::kill(child.id() as i32, if fast { libc::SIGINT } else { libc::SIGTERM });
    }
}

#[cfg(not(unix))]
fn terminate(_child: &Child, _fast: bool) {}

/// Para os serviços em ordem inversa (Caddy, PHP-FPM, Redis, banco), de forma graciosa.
pub fn stop(running: &mut Running) {
    for s in running.services.iter_mut().rev() {
        terminate(&s.child, s.fast_stop);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match s.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100))
                }
                _ => {
                    let _ = s.child.kill();
                    let _ = s.child.wait();
                    break;
                }
            }
        }
    }
    running.services.clear();
}

pub const LOG_FILES: &[&str] = &[
    "access.log",
    "caddy.log",
    "php-error.log",
    "php-fpm.log",
    "mysql.log",
    "postgres.log",
    "valkey.log",
    "mailpit.log",
    "wp-cli.log",
];

/// Últimas `max_lines` linhas de um log do projeto.
pub fn read_log(base: &Path, id: &str, file: &str, max_lines: usize) -> Result<String, String> {
    if !LOG_FILES.contains(&file) {
        return Err("log desconhecido".into());
    }
    let path = project::project_dir(base, id).join("logs").join(file);
    let text = fs::read_to_string(&path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    Ok(lines[start..].join("\n"))
}

/// Pasta pública do projeto (onde fica o `index.php`, ou o WordPress).
pub fn docroot_dir(base: &Path, p: &Project) -> PathBuf {
    Layout::new(base, p).docroot(p)
}

pub fn app_dir(base: &Path, id: &str) -> PathBuf {
    project::project_dir(base, id).join("app")
}

#[cfg(test)]
mod tests {
    /// Só serviços nossos, do projeto certo, são tratados como órfãos.
    #[test]
    fn reconhece_servicos_orfaos_do_projeto() {
        let needle = "/Users/ana/LocalTusk/projects/meu-site/";
        let listing = "\
  101 /opt/homebrew/bin/mysqld --defaults-file=/Users/ana/LocalTusk/projects/meu-site/conf/my.cnf
  102 php-fpm: master process (/Users/ana/LocalTusk/projects/meu-site/conf/php-fpm.conf)
  103 /Users/ana/LocalTusk/bin/tools/caddy/caddy run --config /Users/ana/LocalTusk/projects/meu-site/conf/Caddyfile --adapter caddyfile
  104 /Users/ana/LocalTusk/bin/tools/mailpit/mailpit --database /Users/ana/LocalTusk/projects/meu-site/data/mailpit.db
  105 php-fpm: pool www
  106 /opt/homebrew/bin/mysqld --defaults-file=/Users/ana/LocalTusk/projects/outro-site/conf/my.cnf
  107 /usr/bin/vim /Users/ana/LocalTusk/projects/meu-site/conf/my.cnf
  108 php wp-cli.phar --path=/Users/ana/LocalTusk/projects/meu-site/app/public core version
  109 /Users/ana/LocalTusk/projects/meu-site/conf/shims/mysql --version
  110 /opt/homebrew/bin/mysqld --defaults-file=/Users/ana/LocalTusk/projects/meu-site-2/conf/my.cnf
  999 /opt/homebrew/bin/mysqld --defaults-file=/Users/ana/LocalTusk/projects/meu-site/conf/my.cnf
";
        let pids = |me| parse_ps(listing, needle, me).into_iter().map(|(p, _)| p).collect::<Vec<_>>();
        // mysqld, php-fpm (master), caddy e mailpit do projeto; nada de editor, WP-CLI ou outros projetos.
        assert_eq!(pids(0), vec![101, 102, 103, 104, 999]);
        // O próprio processo do app nunca entra.
        assert_eq!(pids(999), vec![101, 102, 103, 104]);
        assert!(parse_ps(listing, "/Users/ana/LocalTusk/projects/nenhum/", 0).is_empty());
    }

    use super::*;
    use crate::download::InstallProgress;
    use std::io::{Read, Write};

    fn http_get(port: u16) -> String {
        http_get_path(port, "/")
    }

    fn http_get_path(port: u16, path: &str) -> String {
        http_request(port, "GET", path, "", "")
    }

    fn http_request(port: u16, method: &str, path: &str, extra_headers: &str, body: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
        // Sem um `Host:` próprio nos cabeçalhos extras, usa localhost:porta.
        let host = if extra_headers.starts_with("Host:") { String::new() } else { format!("Host: localhost:{port}\r\n") };
        write!(
            s,
            "{method} {path} HTTP/1.0\r\n{host}{extra_headers}Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    /// Pasta com as ferramentas dos testes ponta a ponta (Caddy, MySQL 8.4 e PHP 8.3), baixadas pelo
    /// próprio app uma única vez e reaproveitadas pelos testes. Assim eles não dependem do que a
    /// máquina tem instalado (Homebrew, pacotes da distribuição).
    fn tools_cache() -> PathBuf {
        let cache = std::env::temp_dir().join("localtusk-test-tools");
        let quiet = |_: InstallProgress| {};
        let has = |sub: &str| {
            fs::read_dir(cache.join("bin").join(sub)).map(|mut d| d.next().is_some()).unwrap_or(false)
        };
        if !has("tools/caddy") {
            extras::install(&cache, "caddy", &quiet).expect("instalar o Caddy");
        }
        if !has("mysql") {
            let v = mysql_manager::catalog(&cache).unwrap().into_iter().find(|r| r.series == "8.4" && r.latest).expect("MySQL 8.4");
            mysql_manager::install(&cache, &v.version, &quiet).expect("instalar o MySQL");
        }
        if !has("php") {
            let v = crate::php_manager::catalog(&cache).unwrap().into_iter().find(|r| r.minor == "8.3" && r.latest).expect("PHP 8.3");
            crate::php_manager::install(&cache, &v.version, &quiet).expect("instalar o PHP");
        }
        cache
    }

    /// Pasta nova de teste que usa as ferramentas do cache (`parts`: `tools/caddy`, `mysql`, `php`)
    /// por atalho, sem baixar de novo. O que o teste instalar fora dessas pastas fica só nela.
    fn seeded_base(name: &str, parts: &[&str]) -> PathBuf {
        let cache = tools_cache();
        let base = std::env::temp_dir().join(format!("localtusk-test-{name}-{}", project::now()));
        for part in parts {
            let from = cache.join("bin").join(part);
            let to = base.join("bin").join(part);
            fs::create_dir_all(to.parent().unwrap()).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(&from, &to).unwrap();
            #[cfg(windows)]
            std::os::windows::fs::symlink_dir(&from, &to).unwrap();
        }
        base
    }

    /// Teste ponta a ponta: baixa (uma vez) Caddy, MySQL e PHP.
    /// Rode com: `cargo test -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn start_serve_stop() {
        let base = seeded_base("serve", &["tools/caddy", "mysql", "php"]);
        let http = project::free_port(18000, &Default::default()).unwrap();
        let db = project::free_port(13307, &Default::default()).unwrap();
        let p = Project {
            id: "teste".into(),
            name: "Teste".into(),
            docroot: "".into(),
            php_version: "".into(),
            db_version: "".into(),
            http_port: http,
            db_port: db,
            db_name: "teste".into(),
            created_at: 0,
            ..Default::default()
        };
        let mut running = start(&base, &p).expect("start falhou");
        assert!(running.is_alive());
        let body = http_get(http);
        println!("{body}");
        assert!(body.contains("200 OK"), "resposta inesperada");
        assert!(body.contains("conectado"), "PHP não conectou ao MySQL");
        stop(&mut running);
        assert!(project::port_is_free(http), "Caddy ainda no ar");
        assert!(project::port_is_free(db), "MySQL ainda no ar");
        let _ = fs::remove_dir_all(&base);
    }

    /// O inspetor registra o que o PHP recebeu e respondeu, sem alterar a resposta ao cliente.
    /// Usa o Caddy, o MySQL e o PHP baixados pelo app para o teste. Rode com:
    /// `cargo test --lib api_inspector -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn api_inspector_end_to_end() {
        let base = seeded_base("api", &["tools/caddy", "mysql", "php"]);
        let http = project::free_port(18200, &Default::default()).unwrap();
        let db = project::free_port(13500, &Default::default()).unwrap();
        let p = Project {
            id: "api".into(),
            name: "API".into(),
            docroot: "public".into(),
            php_version: "".into(),
            db_version: "".into(),
            http_port: http,
            db_port: db,
            db_name: "api".into(),
            created_at: 0,
            ..Default::default()
        };
        scaffold(&base, &p).unwrap();
        fs::create_dir_all(docroot_dir(&base, &p)).unwrap();
        fs::write(
            docroot_dir(&base, &p).join("index.php"),
            "<?php\nheader('Content-Type: application/json');\nheader('X-Resposta: ok');\nhttp_response_code(201);\n\
             echo json_encode(['metodo' => $_SERVER['REQUEST_METHOD'], 'corpo' => file_get_contents('php://input'), 'ç' => 'ã']);\n",
        )
        .unwrap();
        let mut running = start(&base, &p).expect("start falhou");

        // 1) POST com JSON e um cabeçalho próprio.
        let json = r#"{"nome":"Ana","idade":30}"#;
        let r1 = http_request(
            http,
            "POST",
            "/index.php?x=1",
            "X-Teste: ola\r\nContent-Type: application/json\r\n",
            json,
        );
        assert!(r1.contains("201 Created") && r1.contains("X-Resposta: ok"), "resposta alterada: {r1}");
        assert!(r1.contains(r#""metodo":"POST""#) && r1.contains("Ana"), "o PHP não recebeu o corpo: {r1}");
        // 2) GET simples e 3) formulário urlencoded.
        http_request(http, "GET", "/index.php?q=busca", "", "");
        http_request(http, "POST", "/index.php", "Content-Type: application/x-www-form-urlencoded\r\n", "a=1&b=2");

        // O registro é gravado no fim da requisição; espera as 3 entradas.
        let mut entries = vec![];
        for _ in 0..40 {
            entries = crate::apitools::read_requests(&base, "api", 0).unwrap().entries;
            if entries.len() >= 3 {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(entries.len(), 3, "esperava 3 requisições registradas: {entries:?}");

        let first = &entries[0];
        assert_eq!(first["method"], "POST");
        assert_eq!(first["uri"], "/index.php?x=1");
        assert_eq!(first["status"], 201);
        assert_eq!(first["requestBody"], json);
        assert_eq!(first["requestBodyTruncated"], false);
        let has = |list: &serde_json::Value, name: &str, value: &str| {
            list.as_array().unwrap().iter().any(|h| {
                h[0].as_str().unwrap().eq_ignore_ascii_case(name) && h[1].as_str().unwrap() == value
            })
        };
        assert!(has(&first["requestHeaders"], "X-Teste", "ola"), "{}", first["requestHeaders"]);
        assert!(has(&first["responseHeaders"], "X-Resposta", "ok"), "{}", first["responseHeaders"]);
        assert!(first["responseBody"].as_str().unwrap().contains("\"metodo\":\"POST\""));
        assert!(first["durationMs"].as_f64().unwrap() >= 0.0);
        assert_eq!(entries[1]["method"], "GET");
        assert_eq!(entries[1]["uri"], "/index.php?q=busca");
        assert_eq!(entries[2]["requestBody"], "a=1&b=2");

        // O cliente de API conversa com o mesmo projeto.
        let resp = crate::apitools::send(&crate::apitools::ApiRequest {
            method: "PUT".into(),
            url: format!("http://127.0.0.1:{http}/index.php"),
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: Some(r#"{"a":1}"#.into()),
            follow_redirects: false,
            timeout_secs: Some(10),
        })
        .expect("cliente");
        assert_eq!(resp.status, 201);
        assert!(resp.body.contains(r#""metodo":"PUT""#), "{}", resp.body);

        // Limpar zera o registro.
        crate::apitools::clear_requests(&base, "api").unwrap();
        assert!(crate::apitools::read_requests(&base, "api", 0).unwrap().entries.is_empty());

        stop(&mut running);
        let _ = fs::remove_dir_all(&base);
    }

    /// Baixa um PHP 8.1 de verdade (~36 MB), sobe um projeto com ele e confere a versão.
    /// Rode com: `cargo test --lib install_and_serve -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn install_and_serve_old_php() {
        let base = seeded_base("dl", &["tools/caddy", "mysql"]);

        let cat = crate::php_manager::catalog(&base).expect("catálogo");
        println!("{} versões no catálogo", cat.len());
        let target = cat.iter().find(|r| r.minor == "8.1" && r.latest).expect("8.1 no catálogo");
        println!("instalando PHP {}", target.version);

        crate::php_manager::install(&base, &target.version, &|p| {
            if p.stage != "download" || p.downloaded == p.total {
                println!("  [{}] {}/{}", p.stage, p.downloaded, p.total);
            }
        })
        .expect("install falhou");

        let installs = tools::php_installs(&base);
        assert!(installs.iter().any(|i| i.managed && i.full_version == target.version));

        let http = project::free_port(18100, &Default::default()).unwrap();
        let db = project::free_port(13400, &Default::default()).unwrap();
        let p = Project {
            id: "velho".into(),
            name: "Velho".into(),
            docroot: "".into(),
            php_version: "8.1".into(),
            db_version: "".into(),
            http_port: http,
            db_port: db,
            db_name: "velho".into(),
            created_at: 0,
            ..Default::default()
        };
        let mut running = start(&base, &p).expect("start falhou");
        let body = http_get(http);
        println!("{}", body.lines().take(12).collect::<Vec<_>>().join("\n"));
        assert!(body.contains(&format!("X-Powered-By: PHP/{}", target.version)), "não está usando o PHP baixado");
        assert!(body.contains("conectado"), "PHP baixado não conectou ao MySQL");
        stop(&mut running);

        crate::php_manager::uninstall(&base, &target.version).unwrap();
        assert!(!tools::php_installs(&base).iter().any(|i| i.managed));
        let _ = fs::remove_dir_all(&base);
    }

    /// Instala Adminer Neo e Mailpit de verdade, sobe um projeto e testa cada um.
    /// Rode com: `cargo test --lib extras_end_to_end -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn extras_end_to_end() {
        let base = seeded_base("extras", &["mysql", "php"]);
        for id in extras::IDS {
            extras::install(&base, id, &|p| {
                if p.stage != "download" {
                    println!("  [{}] {}", p.id, p.stage);
                }
            })
            .unwrap_or_else(|e| panic!("instalar {id}: {e}"));
            println!("{id} instalado: {:?}", extras::status(&base).iter().find(|s| s.id == *id).unwrap().version);
        }

        let http = project::free_port(18200, &Default::default()).unwrap();
        let db = project::free_port(13500, &Default::default()).unwrap();
        let p = Project {
            id: "extras".into(),
            name: "Extras".into(),
            docroot: "".into(),
            php_version: "".into(),
            db_version: "".into(),
            http_port: http,
            db_port: db,
            db_name: "extras".into(),
            created_at: 0,
            ..Default::default()
        };
        let mut running = start(&base, &p).expect("start falhou");
        let info = running.info.clone();
        let admin = info.admin_port.expect("sem porta admin");
        let mail_ui = info.mail_ui_port.expect("sem mailpit");

        // Adminer Neo
        let neo = http_get_path(admin, "/adminneo/");
        println!("--- Adminer Neo: {}", neo.lines().next().unwrap_or(""));
        let forms: Vec<&str> = neo.split("name=\"").skip(1).map(|x| x.split('"').next().unwrap()).collect();
        println!("campos do formulário: {forms:?}");
        println!("server prefill presente: {}", neo.contains(&format!("127.0.0.1:{db}")));
        assert!(neo.starts_with("HTTP/1.0 200"));

        // Login automático do Adminer Neo, como o navegador faria: GET (cookie) + POST (login)
        let login_page = http_get_path(admin, "/adminneo/login.php");
        assert!(login_page.contains("auth[username]"), "login.php não gerou o formulário");
        let cookie = neo
            .lines()
            .find(|l| l.to_lowercase().starts_with("set-cookie:"))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|c| c.split(';').next())
            .unwrap_or("")
            .trim()
            .to_string();
        println!("cookie de sessão: {cookie}");
        let form = format!(
            "auth%5Bdriver%5D=mysql&auth%5Bserver%5D=127.0.0.1%3A{db}&auth%5Busername%5D=root&auth%5Bpassword%5D=root&auth%5Bdb%5D=extras"
        );
        let post = http_request(
            admin,
            "POST",
            "/adminneo/index.php",
            &format!("Cookie: {cookie}\r\nContent-Type: application/x-www-form-urlencoded\r\n"),
            &form,
        );
        println!("--- login POST: {}", post.lines().next().unwrap_or(""));
        let location = post.lines().find(|l| l.to_lowercase().starts_with("location:")).unwrap_or("(sem Location)");
        println!("{location}");
        // Segue o redirecionamento com o mesmo cookie
        let target = location.splitn(2, ':').nth(1).unwrap_or("/adminneo/").trim().to_string();
        let target = if target.starts_with('/') { target } else { format!("/adminneo/{target}") };
        // O login regenera a sessão: o navegador passa a usar o cookie novo.
        let new_cookie = post
            .lines()
            .find(|l| l.to_lowercase().starts_with("set-cookie:"))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|c| c.split(';').next())
            .unwrap_or(&cookie)
            .trim()
            .to_string();
        let after = http_request(
            admin,
            "GET",
            &target,
            &format!("Cookie: {new_cookie}\r\nAccept-Language: pt-BR\r\n"),
            "",
        );
        println!("--- headers do POST:\n{}", post.split("\r\n\r\n").next().unwrap_or(""));
        let text: String = after.split("\r\n\r\n").nth(1).unwrap_or("").split('<').filter_map(|c| c.split('>').nth(1)).map(|t| t.trim()).filter(|t| !t.is_empty()).take(25).collect::<Vec<_>>().join(" | ");
        println!("--- texto da página após login: {text}");
        println!("--- depois do login: {} | pede senha: {} | tem logout: {}", after.lines().next().unwrap_or(""), after.contains("auth[password]"), after.to_lowercase().contains("logout"));
        assert!(!after.contains("auth[password]"), "Adminer Neo continuou pedindo login");

        // Caminhos fora de /adminneo não podem ser servidos
        let outside = http_get_path(admin, "/mailpit/mailpit");
        assert!(outside.starts_with("HTTP/1.0 404"), "o site admin expôs arquivos indevidos: {}", outside.lines().next().unwrap());

        // mail() do PHP -> Mailpit
        let docroot = base.join("projects/extras/app");
        fs::write(
            docroot.join("mail.php"),
            "<?php var_dump(mail('dest@example.test', 'Assunto de teste', 'Corpo da mensagem', 'From: app@localtusk.test'));",
        )
        .unwrap();
        let out = http_get_path(http, "/mail.php");
        println!("--- mail.php: {}", out.lines().last().unwrap_or(""));
        std::thread::sleep(Duration::from_millis(800));
        let api = http_get_path(mail_ui, "/api/v1/messages");
        let body = api.split("\r\n\r\n").nth(1).unwrap_or("");
        println!("--- Mailpit API: {}", &body[..body.len().min(300)]);
        assert!(body.contains("Assunto de teste"), "o e-mail não chegou ao Mailpit");

        stop(&mut running);
        assert!(project::port_is_free(admin) && project::port_is_free(mail_ui));
        let _ = fs::remove_dir_all(&base);
    }

    fn dir_size(p: &Path) -> u64 {
        fs::read_dir(p)
            .map(|rd| {
                rd.flatten()
                    .map(|e| match e.metadata() {
                        Ok(m) if m.is_dir() => dir_size(&e.path()),
                        Ok(m) => m.len(),
                        Err(_) => 0,
                    })
                    .sum()
            })
            .unwrap_or(0)
    }

    /// Ambiente 100% gerenciado: instala Caddy, MySQL 8.4 e PHP 8.3 numa pasta limpa e sobe um
    /// projeto com eles (versões diferentes das do Homebrew, para provar que não são as do sistema).
    /// Baixa ~230 MB. Rode com: `cargo test --lib managed_stack -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn managed_stack() {
        let base = std::env::temp_dir().join(format!("localtusk-test-stack-{}", project::now()));
        let t = Instant::now();

        let cat = mysql_manager::catalog(&base).expect("catálogo do MySQL");
        println!("catálogo do MySQL em {:?}: {} versões", t.elapsed(), cat.len());
        assert!(t.elapsed() < Duration::from_millis(200), "o catálogo deveria ser instantâneo (embutido)");
        for series in ["8.0", "8.4", "9.7"] {
            let v: Vec<&str> = cat.iter().filter(|r| r.series == series).map(|r| r.version.as_str()).collect();
            println!("  {series}: {} versões, mais nova {:?}", v.len(), v.first());
        }
        let my = cat.iter().find(|r| r.series == "8.4" && r.latest).expect("8.4").version.clone();
        let pcat = crate::php_manager::catalog(&base).expect("catálogo do PHP");
        let php = pcat.iter().find(|r| r.minor == "8.3" && r.latest).expect("php 8.3").version.clone();

        let show = |p: InstallProgress| {
            if p.stage != "download" {
                println!("  [{}] {}", p.id, p.stage);
            }
        };
        extras::install(&base, "caddy", &show).expect("caddy");
        println!("Caddy {:?}", extras::status(&base).iter().find(|s| s.id == "caddy").unwrap().version);
        mysql_manager::install(&base, &my, &show).expect("mysql");
        println!("MySQL {my} instalado: {} MB em disco", dir_size(&mysql_manager::install_dir(&base, &my)) / 1024 / 1024);
        crate::php_manager::install(&base, &php, &show).expect("php");

        let http = project::free_port(18300, &Default::default()).unwrap();
        let db = project::free_port(13600, &Default::default()).unwrap();
        let p = Project {
            id: "stack".into(),
            name: "Stack".into(),
            docroot: "".into(),
            php_version: "8.3".into(),
            db_version: "8.4".into(),
            http_port: http,
            db_port: db,
            db_name: "stack".into(),
            created_at: 0,
            ..Default::default()
        };
        let mut running = start(&base, &p).expect("start falhou");
        let body = http_get(http);
        println!("{}", body.lines().filter(|l| l.contains("X-Powered") || l.contains("Banco")).collect::<Vec<_>>().join("\n"));
        assert!(body.contains(&format!("X-Powered-By: PHP/{php}")), "não usou o PHP gerenciado");
        assert!(body.contains(&format!("MySQL {my}")), "não usou o MySQL gerenciado");
        stop(&mut running);
        assert!(project::port_is_free(http) && project::port_is_free(db));

        // Segunda subida: reaproveita o banco já criado
        let mut running = start(&base, &p).expect("2º start falhou");
        assert!(http_get(http).contains(&format!("MySQL {my}")));
        stop(&mut running);

        assert!(db_initialized(&base, "stack"));
        let _ = fs::remove_dir_all(&base);
    }

    /// PostgreSQL + Redis (Valkey) de ponta a ponta, tudo gerenciado pelo app: instala Caddy, PHP,
    /// PostgreSQL e Valkey (compilado) numa pasta de cache, sobe um projeto e conversa com os dois
    /// pelo PHP. Confere também o PHP "common" (pdo_pgsql) baixado sozinho, backup e restauração com
    /// pg_dump/psql, persistência entre reinícios, os atalhos do terminal e o encerramento de
    /// serviços órfãos. Baixa ~250 MB na primeira vez (cache em /tmp/localtusk-test-pgredis).
    /// Rode com: `cargo test --lib postgres_redis_end_to_end -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn postgres_redis_end_to_end() {
        let base = std::env::temp_dir().join("localtusk-test-pgredis");
        let _ = fs::remove_dir_all(base.join("projects"));
        let _ = fs::remove_dir_all(base.join("backups"));
        let show = |p: InstallProgress| {
            if p.stage != "download" {
                println!("  [{}] {} {}", p.id, p.stage, p.message.unwrap_or_default());
            }
        };

        // --- Ferramentas ---
        if !extras::is_installed(&base, "caddy") {
            extras::install(&base, "caddy", &show).expect("caddy");
        }
        let t = Instant::now();
        let pg_cat = postgres_manager::catalog(&base).expect("catálogo do PostgreSQL");
        assert!(t.elapsed() < Duration::from_millis(200), "o catálogo deveria ser instantâneo (embutido)");
        let pg_ver = pg_cat.iter().find(|r| r.major == "16").expect("PostgreSQL 16").version.clone();
        if postgres_manager::pick(&base, "16").is_none_or(|i| i.full_version != pg_ver) {
            postgres_manager::install(&base, &pg_ver, &show).expect("postgresql");
        }
        let pg = postgres_manager::pick(&base, "16").unwrap();
        println!(
            "PostgreSQL {} instalado: {} MB em disco",
            pg.full_version,
            dir_size(&postgres_manager::install_dir(&base, &pg.full_version)) / 1024 / 1024
        );
        for tool in ["postgres", "initdb", "psql", "pg_dump", "pg_restore"] {
            assert!(pg.tool(tool).is_some(), "faltou {tool}");
        }
        if valkey_manager::pick(&base).is_none() {
            let t = Instant::now();
            valkey_manager::install(&base, valkey_manager::VERSION, &show).expect("valkey");
            println!("Valkey compilado em {:?}", t.elapsed());
        }
        assert!(valkey_manager::pick(&base).unwrap().cli().is_some());
        if tools::pick_php(&base, "8.3").is_none_or(|i| !i.managed) {
            let pcat = crate::php_manager::catalog(&base).expect("catálogo do PHP");
            let php = pcat.iter().find(|r| r.minor == "8.3" && r.latest).expect("php 8.3").version.clone();
            crate::php_manager::install(&base, &php, &show).expect("php");
        }
        let php = tools::pick_php(&base, "8.3").unwrap();
        // Começa sem o PHP "common", para provar o download automático no primeiro start.
        let _ = fs::remove_file(install_dir_of(&base, &php.full_version, "php-fpm-pg"));
        let _ = fs::remove_file(install_dir_of(&base, &php.full_version, "php-pg"));

        // --- Projeto ---
        let http = project::free_port(18500, &Default::default()).unwrap();
        let db = project::free_port(15433, &Default::default()).unwrap();
        let redis = project::free_port(16380, &Default::default()).unwrap();
        let p = Project {
            id: "pgredis".into(),
            name: "PG Redis".into(),
            php_version: "8.3".into(),
            db_engine: "postgres".into(),
            db_version: "16".into(),
            http_port: http,
            db_port: db,
            db_name: "pgredis".into(),
            redis: true,
            redis_port: redis,
            ..Default::default()
        };
        scaffold(&base, &p).unwrap();
        fs::write(
            docroot_dir(&base, &p).join("index.php"),
            r#"<?php
$pdo = new PDO('pgsql:host=' . getenv('DB_HOST') . ';port=' . getenv('DB_PORT') . ';dbname=' . getenv('DB_NAME'), getenv('DB_USER'), getenv('DB_PASSWORD'));
$pdo->exec('CREATE TABLE IF NOT EXISTS notas (id serial PRIMARY KEY, texto text)');
if (isset($_GET['add'])) { $pdo->prepare('INSERT INTO notas (texto) VALUES (?)')->execute([$_GET['add']]); }
$r = new Redis();
$r->connect(getenv('REDIS_HOST'), (int) getenv('REDIS_PORT'));
if (isset($_GET['set'])) { $r->set('chave', $_GET['set']); }
echo 'engine=' . getenv('DB_ENGINE') . ' conn=' . getenv('DB_CONNECTION') . "\n";
echo 'pg=' . $pdo->query('SHOW server_version')->fetchColumn() . "\n";
echo 'notas=' . implode(',', $pdo->query('SELECT texto FROM notas ORDER BY id')->fetchAll(PDO::FETCH_COLUMN)) . "\n";
echo 'redis=' . ($r->get('chave') ?: '-') . "\n";
echo 'ext=' . (extension_loaded('pdo_pgsql') ? 'pdo_pgsql' : 'sem') . "\n";
"#,
        )
        .unwrap();

        let t = Instant::now();
        let mut running = start(&base, &p).expect("start falhou");
        println!("start (com download do PHP common) em {:?}", t.elapsed());
        assert!(crate::php_manager::pg_fpm(&base, &php.full_version).is_some(), "o PHP common deveria ter sido baixado");
        assert!(crate::php_manager::pg_cli(&base, &php.full_version).is_some());

        let body = http_get(http);
        println!("{}", body.lines().rev().take(6).collect::<Vec<_>>().join(" | "));
        assert!(body.contains("engine=postgres") && body.contains("conn=pgsql"), "{body}");
        assert!(body.contains(&format!("pg={}", &pg.full_version[..pg.full_version.rfind('.').unwrap()])), "{body}");
        assert!(body.contains("ext=pdo_pgsql"), "{body}");
        assert!(body.contains("notas=\n") && body.contains("redis=-"), "{body}");
        assert!(!project::port_is_free(db) && !project::port_is_free(redis));
        // O banco e o Redis só escutam em 127.0.0.1.
        if let Some(ip) = project::lan_ip() {
            assert!(TcpStream::connect_timeout(&SocketAddr::from((ip, db)), Duration::from_millis(500)).is_err(), "PostgreSQL exposto na rede");
            assert!(TcpStream::connect_timeout(&SocketAddr::from((ip, redis)), Duration::from_millis(500)).is_err(), "Redis exposto na rede");
        }

        // Grava dados nos dois e confere pelo PHP.
        let body = http_request(http, "GET", "/?add=primeira&set=valor1", "", "");
        assert!(body.contains("notas=primeira") && body.contains("redis=valor1"), "{body}");

        // A página de exemplo padrão também entende PostgreSQL e Redis.
        let sample = docroot_dir(&base, &p).join("sample.php");
        fs::write(&sample, SAMPLE_INDEX).unwrap();
        let body = http_get_path(http, "/sample.php");
        assert!(body.contains("PostgreSQL 16") && body.contains("Valkey"), "{body}");
        let _ = fs::remove_file(&sample);

        // --- Backup e restauração (pg_dump / psql) ---
        let archive = crate::backup::create(&base, &p, "", "backup-teste", &|_| {}).expect("backup");
        let manifest = crate::backup::read_manifest(&archive).unwrap();
        assert_eq!(manifest.db_engine, "postgres");
        psql(&pg.tool("psql").unwrap(), db, "pgredis", "DROP TABLE notas").expect("drop");
        crate::backup::restore_into(&base, &p, crate::backup::RestoreSource::Archive(&archive), "restore-teste", &|_| {}).expect("restore");
        // restore_into apaga a pasta app/ e traz a do backup de volta (com o index.php do teste).
        let body = http_get(http);
        assert!(body.contains("notas=primeira"), "dados não voltaram do backup: {body}");
        // Um backup de PostgreSQL não entra num projeto MySQL.
        let mysql_twin = Project { db_engine: String::new(), ..p.clone() };
        let err = crate::backup::restore_into(&base, &mysql_twin, crate::backup::RestoreSource::Archive(&archive), "x", &|_| {}).unwrap_err();
        assert!(err.contains("PostgreSQL") && err.contains("MySQL"), "{err}");

        // --- Terminal: atalhos já ligados ao banco e ao Redis ---
        let script = crate::wp::prepare_terminal(&base, &p).expect("terminal");
        let script_text = fs::read_to_string(&script).unwrap();
        assert!(script_text.contains("export PGPORT=") && script_text.contains("export REDIS_PORT="), "{script_text}");
        let shims = project::project_dir(&base, &p.id).join("conf").join("shims");
        let psql_out = Command::new(shims.join("psql"))
            .env("PGHOST", "127.0.0.1")
            .env("PGPORT", db.to_string())
            .env("PGUSER", "root")
            .env("PGPASSWORD", "root")
            .env("PGDATABASE", "pgredis")
            .args(["-tAc", "SELECT count(*) FROM notas"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&psql_out.stdout).trim(), "1", "{}", String::from_utf8_lossy(&psql_out.stderr));
        let ping = Command::new(shims.join("redis-cli")).arg("ping").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&ping.stdout).trim(), "PONG");

        // --- Parar: tudo para e as portas voltam ---
        let t = Instant::now();
        stop(&mut running);
        println!("stop em {:?}", t.elapsed());
        assert!(t.elapsed() < Duration::from_secs(10), "o PostgreSQL deveria desligar rápido (SIGINT)");
        assert!(project::port_is_free(http) && project::port_is_free(db) && project::port_is_free(redis));
        assert!(db_initialized(&base, "pgredis"));
        let log = fs::read_to_string(project::project_dir(&base, "pgredis").join("logs/postgres.log")).unwrap_or_default();
        assert!(!log.contains("PANIC") && !log.contains("FATAL:  data directory"), "{log}");

        // --- Segunda subida: os dados persistem (banco e Redis) ---
        let mut running = start(&base, &p).expect("2º start falhou");
        let body = http_get(http);
        assert!(body.contains("notas=primeira") && body.contains("redis=valor1"), "dados não persistiram: {body}");

        // --- Órfãos: um app morto à força deixa os serviços; o próximo start/abertura os encerra ---
        // Largamos os processos sem pará-los (como um app morto à força); as threads só recolhem
        // o processo depois que ele sair, para não virar zumbi deste teste.
        for mut svc in std::mem::take(&mut running.services) {
            std::thread::spawn(move || {
                let _ = svc.child.wait();
            });
        }
        let found = project_processes(&base, &p.id);
        let names: Vec<String> = found.iter().map(|(_, c)| c.split_whitespace().next().unwrap_or("").rsplit('/').next().unwrap_or("").to_string()).collect();
        println!("órfãos reconhecidos: {names:?}");
        for expected in ["postgres", "valkey-server", "caddy"] {
            assert!(names.iter().any(|n| n == expected), "não reconheceu {expected}: {names:?}");
        }
        assert!(php_fpm_seen(&found), "não reconheceu o php-fpm");
        let t = Instant::now();
        let n = reap_orphans(&base, &p.id);
        println!("{n} órfãos encerrados em {:?}", t.elapsed());
        assert!(project::port_is_free(http) && project::port_is_free(db) && project::port_is_free(redis), "ainda há serviços no ar");

        // --- WordPress não é oferecido com PostgreSQL ---
        let options = crate::wp::WpOptions {
            title: "Site".into(),
            admin_user: "admin".into(),
            admin_password: "senha-forte-123".into(),
            admin_email: "a@b.co".into(),
            locale: "en_US".into(),
            replace_existing: false,
        };
        let wp_err = crate::wp::install_wordpress(&base, &p, &options, &|_| {}).unwrap_err();
        assert!(wp_err.contains("MySQL"), "{wp_err}");

        let _ = fs::remove_dir_all(base.join("projects"));
        let _ = fs::remove_dir_all(base.join("backups"));
    }

    fn install_dir_of(base: &Path, version: &str, file: &str) -> PathBuf {
        crate::php_manager::install_dir(base, version).join(file)
    }

    fn php_fpm_seen(found: &[(i32, String)]) -> bool {
        found.iter().any(|(_, c)| c.starts_with("php-fpm:"))
    }

    /// Instala o WordPress de verdade (WP-CLI + PHP CLI + MySQL), confere o site e os atalhos
    /// do terminal. Baixa ~250 MB na primeira vez (guarda em /tmp/localtusk-test-wp para as próximas).
    /// Rode com: `cargo test --lib wordpress_end_to_end -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn wordpress_end_to_end() {
        let base = std::env::temp_dir().join("localtusk-test-wp");
        let _ = fs::remove_dir_all(project::project_dir(&base, "wp"));
        let show = |p: InstallProgress| {
            if p.stage != "download" {
                println!("  [{}] {} {}", p.id, p.stage, p.message.unwrap_or_default());
            }
        };

        if !extras::is_installed(&base, "caddy") {
            extras::install(&base, "caddy", &show).expect("caddy");
        }
        if mysql_manager::pick(&base, "8.4").is_none() {
            let cat = mysql_manager::catalog(&base).expect("catálogo");
            let my = cat.iter().find(|r| r.series == "8.4" && r.latest).unwrap().version.clone();
            mysql_manager::install(&base, &my, &show).expect("mysql");
        }
        if crate::tools::pick_php(&base, "8.3").map(|i| !i.managed).unwrap_or(true) {
            let cat = crate::php_manager::catalog(&base).expect("catálogo do PHP");
            let php = cat.iter().find(|r| r.minor == "8.3" && r.latest).unwrap().version.clone();
            crate::php_manager::install(&base, &php, &show).expect("php");
        }

        let http = project::free_port(18400, &Default::default()).unwrap();
        let db = project::free_port(13700, &Default::default()).unwrap();
        let p = Project {
            id: "wp".into(),
            name: "Meu WP".into(),
            docroot: "".into(),
            php_version: "8.3".into(),
            db_version: "8.4".into(),
            http_port: http,
            db_port: db,
            db_name: "wp".into(),
            created_at: 0,
            ..Default::default()
        };
        scaffold(&base, &p).unwrap();
        let mut running = start(&base, &p).expect("start falhou");

        // Pasta com arquivos do usuário: só é apagada com a confirmação (replace_existing).
        fs::write(docroot_dir(&base, &p).join("lixo.txt"), "x").unwrap();
        let mut opts = crate::wp::WpOptions {
            title: "Blog de Teste".into(),
            admin_user: "admin".into(),
            admin_password: "SenhaForte123".into(),
            admin_email: "admin@example.com".into(),
            locale: "pt_BR".into(),
            replace_existing: false,
        };
        assert!(crate::wp::install_wordpress(&base, &p, &opts, &show).is_err(), "deveria recusar sem confirmação");
        assert!(docroot_dir(&base, &p).join("lixo.txt").exists());
        opts.replace_existing = true;
        let t = Instant::now();
        crate::wp::install_wordpress(&base, &p, &opts, &show).expect("install_wordpress");
        println!("WordPress instalado em {:?}", t.elapsed());
        assert!(crate::wp::is_wordpress(&base, &p));
        assert!(!docroot_dir(&base, &p).join("lixo.txt").exists(), "o arquivo antigo deveria ter sido apagado");

        // Instalar de novo é recusado.
        assert!(crate::wp::install_wordpress(&base, &p, &opts, &show).is_err());

        // O site é instalado e acessado pelo domínio do projeto (<id>.localhost).
        let host = format!("Host: {}:{http}\r\n", project::site_host(&p));
        let home = http_request(http, "GET", "/", &host, "");
        assert!(home.contains("200 OK"), "home: {}", home.lines().take(5).collect::<Vec<_>>().join("\n"));
        assert!(home.contains("Blog de Teste"), "título não aparece na home");
        let login = http_request(http, "GET", "/wp-login.php", &host, "");
        assert!(login.contains("200 OK") && login.contains("user_login"), "wp-login não abriu");
        // Links permanentes bonitos: /wp-json/ passa pelo index.php.
        let json = http_request(http, "GET", "/wp-json/", &host, "");
        assert!(json.contains("200 OK") && json.contains("Blog de Teste"), "REST API/permalinks não funcionam");
        // Login de verdade.
        let cookie = "Cookie: wordpress_test_cookie=WP%20Cookie%20check\r\n";
        let post = http_request(
            http,
            "POST",
            "/wp-login.php",
            &format!("{host}{cookie}Content-Type: application/x-www-form-urlencoded\r\n"),
            "log=admin&pwd=SenhaForte123&wp-submit=Log+In",
        );
        assert!(post.contains("302") && post.contains("wordpress_logged_in"), "login do admin falhou: {}", post.lines().take(12).collect::<Vec<_>>().join("\n"));

        // Atalhos do terminal.
        let script = crate::wp::prepare_terminal(&base, &p).expect("terminal");
        let shims = script.parent().unwrap().join("shims");
        let sh = |cmd: &str| {
            let out = Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("cd '{}' && {cmd}", runtime_docroot(&base, &p)))
                .env("PATH", format!("{}:/usr/bin:/bin", shims.display()))
                .output()
                .unwrap();
            let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            println!("$ {cmd}\n{}", text.trim());
            assert!(out.status.success(), "{cmd} falhou");
            text
        };
        assert!(sh("php -v").contains("PHP 8.3"));
        assert!(sh("wp core version").trim().chars().next().unwrap().is_ascii_digit());
        assert!(sh("wp option get blogname").contains("Blog de Teste"));
        assert!(sh("mysql -e 'select count(*) as posts from wp_posts'").contains("posts"));

        // Backup e restauração: o WordPress volta em OUTRO projeto (outro id, porta e banco).
        let archive = crate::backup::create(&base, &p, "", "bk", &show).expect("backup");
        let manifest = crate::backup::read_manifest(&archive).expect("manifest");
        assert!(manifest.id == "wp" && manifest.has_wordpress);
        assert_eq!(crate::backup::list(&base, Some("wp")).len(), 1);
        assert!(crate::backup::list(&base, Some("outro")).is_empty());
        println!("backup: {} ({} bytes)", archive.display(), fs::metadata(&archive).unwrap().len());

        let http2 = project::free_port(18500, &Default::default()).unwrap();
        let db2 = project::free_port(13800, &Default::default()).unwrap();
        let p2 = Project {
            id: "wp2".into(),
            name: "Meu WP 2".into(),
            docroot: "".into(),
            php_version: "8.3".into(),
            db_version: "8.4".into(),
            http_port: http2,
            db_port: db2,
            db_name: "wp2".into(),
            created_at: 0,
            ..Default::default()
        };
        scaffold(&base, &p2).unwrap();
        let mut running2 = start(&base, &p2).expect("start do projeto restaurado");
        let warning = crate::backup::restore_into(&base, &p2, crate::backup::RestoreSource::Archive(&archive), "bk", &show).expect("restore");
        assert!(warning.is_none(), "aviso inesperado: {warning:?}");

        let host2 = format!("Host: {}:{http2}\r\n", project::site_host(&p2));
        let home2 = http_request(http2, "GET", "/", &host2, "");
        assert!(home2.contains("200 OK") && home2.contains("Blog de Teste"), "site restaurado não abriu: {}", home2.lines().take(6).collect::<Vec<_>>().join("\n"));
        let post2 = http_request(
            http2,
            "POST",
            "/wp-login.php",
            &format!("{host2}{cookie}Content-Type: application/x-www-form-urlencoded\r\n"),
            "log=admin&pwd=SenhaForte123&wp-submit=Log+In",
        );
        assert!(post2.contains("302") && post2.contains("wordpress_logged_in"), "login no projeto restaurado falhou");
        let cfg = fs::read_to_string(docroot_dir(&base, &p2).join("wp-config.php")).unwrap();
        assert!(cfg.contains("'wp2'") && cfg.contains(&format!("127.0.0.1:{db2}")), "wp-config não aponta para o banco novo");
        stop(&mut running2);
        let _ = fs::remove_dir_all(project::project_dir(&base, "wp2"));

        stop(&mut running);
        let _ = fs::remove_dir_all(project::project_dir(&base, "wp"));
    }

    /// Login automático no wp-admin: código de uso único, vencido, reaproveitado e de quem não é admin.
    /// Mesmas ferramentas (e cache) do `wordpress_end_to_end`. Rode com:
    /// `cargo test --lib auto_login_end_to_end -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn auto_login_end_to_end() {
        let base = std::env::temp_dir().join("localtusk-test-wp");
        let _ = fs::remove_dir_all(project::project_dir(&base, "al"));
        let show = |p: InstallProgress| {
            if p.stage != "download" {
                println!("  [{}] {} {}", p.id, p.stage, p.message.unwrap_or_default());
            }
        };
        if !extras::is_installed(&base, "caddy") {
            extras::install(&base, "caddy", &show).expect("caddy");
        }
        if mysql_manager::pick(&base, "8.4").is_none() {
            let cat = mysql_manager::catalog(&base).expect("catálogo");
            let my = cat.iter().find(|r| r.series == "8.4" && r.latest).unwrap().version.clone();
            mysql_manager::install(&base, &my, &show).expect("mysql");
        }
        if crate::tools::pick_php(&base, "8.3").map(|i| !i.managed).unwrap_or(true) {
            let cat = crate::php_manager::catalog(&base).expect("catálogo do PHP");
            let php = cat.iter().find(|r| r.minor == "8.3" && r.latest).unwrap().version.clone();
            crate::php_manager::install(&base, &php, &show).expect("php");
        }
        let http = project::free_port(18900, &Default::default()).unwrap();
        let db = project::free_port(14200, &Default::default()).unwrap();
        let p = Project {
            id: "al".into(),
            name: "Login".into(),
            docroot: "".into(),
            php_version: "8.3".into(),
            db_version: "8.4".into(),
            http_port: http,
            db_port: db,
            db_name: "al".into(),
            created_at: 0,
            ..Default::default()
        };
        scaffold(&base, &p).unwrap();
        let mut running = start(&base, &p).expect("start falhou");
        let opts = crate::wp::WpOptions {
            title: "Blog Login".into(),
            admin_user: "admin".into(),
            admin_password: "SenhaForte123".into(),
            admin_email: "admin@example.com".into(),
            locale: "en_US".into(),
            replace_existing: true,
        };
        crate::wp::install_wordpress(&base, &p, &opts, &show).expect("install_wordpress");

        assert_eq!(crate::wp::admin_users(&base, &p).unwrap(), vec!["admin".to_string()]);
        crate::wp::install_login_plugin(&base, &p).unwrap();
        crate::wp::install_login_plugin(&base, &p).expect("idempotente");
        let plugin = docroot_dir(&base, &p).join("wp-content/mu-plugins/localtusk-login.php");
        assert!(plugin.is_file());

        let host = format!("Host: {}:{http}\r\n", project::site_host(&p));
        let param = crate::wp::LOGIN_PARAM;
        let visit = |token: &str| http_request(http, "GET", &format!("/wp-login.php?{param}={token}"), &host, "");
        let location = |r: &str| r.lines().find_map(|l| l.strip_prefix("Location: ").or_else(|| l.strip_prefix("location: "))).unwrap_or("").trim().to_string();
        let has_session = |r: &str| r.lines().any(|l| l.to_ascii_lowercase().starts_with("set-cookie: wordpress_logged_in_") && !l.contains("=deleted"));

        // Código válido: entra no wp-admin.
        let token = crate::wp::create_login_token(&base, &p, "admin").unwrap();
        let r = visit(&token);
        assert!(r.contains(" 302"), "{r}");
        assert!(location(&r).ends_with("/wp-admin/"), "Location: {}", location(&r));
        assert!(has_session(&r), "sem cookie de sessão:\n{r}");
        let cookies: Vec<String> = r
            .lines()
            .filter_map(|l| l.strip_prefix("Set-Cookie: ").or_else(|| l.strip_prefix("set-cookie: ")))
            .map(|c| c.split(';').next().unwrap().to_string())
            .collect();
        let admin = http_request(http, "GET", "/wp-admin/", &format!("{host}Cookie: {}\r\n", cookies.join("; ")), "");
        assert!(admin.contains(" 200") && admin.contains("id=\"wpadminbar\""), "wp-admin não abriu logado");

        // Valor que não é texto (`?localtusk-login[]=x`) também volta para o login.
        let arr = http_request(http, "GET", &format!("/wp-login.php?{param}[]=x"), &host, "");
        assert!(arr.contains(" 302") && location(&arr).contains("wp-login.php") && !has_session(&arr), "{arr}");

        // Uso único.
        let again = visit(&token);
        assert!(again.contains(" 302") && location(&again).contains("wp-login.php") && !has_session(&again), "{again}");

        // Código vencido.
        let shims = crate::wp::prepare_terminal(&base, &p).expect("terminal").parent().unwrap().join("shims");
        let wp = |args: &str| {
            let out = Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("cd '{}' && wp {args}", runtime_docroot(&base, &p)))
                .env("PATH", format!("{}:/usr/bin:/bin", shims.display()))
                .output()
                .unwrap();
            assert!(out.status.success(), "wp {args}: {}", String::from_utf8_lossy(&out.stderr));
        };
        let old = "a".repeat(64);
        wp(&format!("transient set {} admin 1", crate::wp::login_token_key(&old)));
        std::thread::sleep(Duration::from_secs(2));
        let expired = visit(&old);
        assert!(expired.contains(" 302") && location(&expired).contains("wp-login.php") && !has_session(&expired), "{expired}");

        // Quem não é administrador não entra.
        wp("user create leitor l@x.test --role=subscriber");
        let reader = crate::wp::create_login_token(&base, &p, "leitor").unwrap();
        let denied = visit(&reader);
        assert!(denied.contains(" 302") && location(&denied).contains("wp-login.php") && !has_session(&denied), "{denied}");

        // Remover com o projeto rodando e parado.
        crate::wp::remove_login_plugin(&base, &p).unwrap();
        assert!(!plugin.exists(), "plugin ficou após remover");
        crate::wp::install_login_plugin(&base, &p).unwrap();
        assert!(plugin.is_file());
        stop(&mut running);
        crate::wp::remove_login_plugin(&base, &p).unwrap();
        assert!(!plugin.exists());
        crate::wp::remove_login_plugin(&base, &p).expect("sem arquivo também é ok");
        let _ = fs::remove_dir_all(project::project_dir(&base, "al"));
    }

    /// Lista (caminho relativo, é link?) de tudo dentro de `dir`, sem seguir links.
    fn snapshot_tree(dir: &Path) -> Vec<(String, bool)> {
        fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, bool)>) {
            for e in fs::read_dir(dir).unwrap().flatten() {
                let path = e.path();
                let meta = fs::symlink_metadata(&path).unwrap();
                let rel = path.strip_prefix(root).unwrap().display().to_string();
                out.push((rel, meta.file_type().is_symlink()));
                if meta.is_dir() {
                    walk(root, &path, out);
                }
            }
        }
        let mut out = vec![];
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    /// Restauração a partir de uma pasta "do Local" (app/public + app/sql/local.sql): o site
    /// abre no endereço do projeto novo e a pasta de origem não é alterada.
    /// Usa as mesmas ferramentas (e o mesmo cache) do `wordpress_end_to_end`. Rode com:
    /// `cargo test --lib local_folder_restore_end_to_end -- --ignored --nocapture`
    #[cfg(unix)]
    #[test]
    #[ignore]
    fn local_folder_restore_end_to_end() {
        use crate::backup::RestoreSource;
        let base = std::env::temp_dir().join("localtusk-test-wp");
        let local = std::env::temp_dir().join("localtusk-test-localsite");
        let _ = fs::remove_dir_all(project::project_dir(&base, "lfsrc"));
        let _ = fs::remove_dir_all(project::project_dir(&base, "lf"));
        let _ = fs::remove_dir_all(&local);
        let show = |p: InstallProgress| {
            if p.stage != "download" {
                println!("  [{}] {} {}", p.id, p.stage, p.message.unwrap_or_default());
            }
        };

        if !extras::is_installed(&base, "caddy") {
            extras::install(&base, "caddy", &show).expect("caddy");
        }
        if mysql_manager::pick(&base, "8.4").is_none() {
            let cat = mysql_manager::catalog(&base).expect("catálogo");
            let my = cat.iter().find(|r| r.series == "8.4" && r.latest).unwrap().version.clone();
            mysql_manager::install(&base, &my, &show).expect("mysql");
        }
        if crate::tools::pick_php(&base, "8.3").map(|i| !i.managed).unwrap_or(true) {
            let cat = crate::php_manager::catalog(&base).expect("catálogo do PHP");
            let php = cat.iter().find(|r| r.minor == "8.3" && r.latest).unwrap().version.clone();
            crate::php_manager::install(&base, &php, &show).expect("php");
        }
        let mk = |id: &str, docroot: &str, http_start: u16, db_start: u16| Project {
            id: id.into(),
            name: id.into(),
            docroot: docroot.into(),
            php_version: "8.3".into(),
            db_version: "8.4".into(),
            http_port: project::free_port(http_start, &Default::default()).unwrap(),
            db_port: project::free_port(db_start, &Default::default()).unwrap(),
            db_name: id.into(),
            created_at: 0,
            ..Default::default()
        };

        // 1) Um WordPress auxiliar vira a pasta "do Local".
        let src = mk("lfsrc", "", 18800, 14100);
        scaffold(&base, &src).unwrap();
        let mut run_src = start(&base, &src).expect("start do projeto auxiliar");
        let opts = crate::wp::WpOptions {
            title: "Blog do Local".into(),
            admin_user: "admin".into(),
            admin_password: "SenhaForte123".into(),
            admin_email: "admin@example.com".into(),
            locale: "pt_BR".into(),
            replace_existing: true,
        };
        crate::wp::install_wordpress(&base, &src, &opts, &show).expect("install_wordpress");

        let sql_dir = local.join("app/sql");
        fs::create_dir_all(&sql_dir).unwrap();
        let dump = sql_dir.join("local.sql");
        let mysqldump = mysql_manager::pick(&base, "8.4").unwrap().mysqld.parent().unwrap().join("mysqldump");
        let out = Command::new(mysqldump)
            .env("MYSQL_PWD", DB_PASSWORD)
            .arg("--host=127.0.0.1")
            .arg(format!("--port={}", src.db_port))
            .arg(format!("--user={DB_USER}"))
            .arg(format!("--result-file={}", dump.display()))
            .arg(&src.db_name)
            .output()
            .unwrap();
        assert!(out.status.success(), "mysqldump: {}", String::from_utf8_lossy(&out.stderr));
        // No Local o site tem outro endereço: troca siteurl/home no dump.
        let old_url = project::site_url(&src);
        let mut sql = fs::read_to_string(&dump).unwrap();
        for name in ["siteurl", "home"] {
            let (from, to) = (format!("'{name}','{old_url}'"), format!("'{name}','http://snap.local'"));
            assert!(sql.contains(&from), "o dump não tem {from}");
            sql = sql.replace(&from, &to);
        }
        // Caminho da pasta no Local gravado no banco, serializado (como Wordfence/upload_path).
        let public = local.join("app/public");
        let old_path = public.display().to_string();
        let val = format!("{old_path}/wp-content/uploads");
        sql.push_str(&format!(
            "\nINSERT INTO `wp_options` (`option_name`, `option_value`, `autoload`) VALUES ('lt_caminho', 'a:1:{{s:4:\"path\";s:{}:\"{val}\";}}', 'no');\n",
            val.len()
        ));
        fs::write(&dump, sql).unwrap();

        crate::local_import::copy_tree(&docroot_dir(&base, &src), &public, &|_| {}).expect("cópia");
        stop(&mut run_src);
        let _ = fs::remove_dir_all(project::project_dir(&base, "lfsrc"));
        let cfg = fs::read_to_string(public.join("wp-config.php")).unwrap();
        let cfg: Vec<String> = cfg
            .lines()
            .map(|l| {
                if l.contains("'DB_NAME'") {
                    "define( 'DB_NAME', 'local' );".to_string()
                } else if l.contains("'DB_HOST'") {
                    "define( 'DB_HOST', 'localhost' );".to_string()
                } else {
                    l.to_string()
                }
            })
            .collect();
        fs::write(public.join("wp-config.php"), cfg.join("\n") + "\n").unwrap();
        // Uma pasta de plugin linkada (como alguns usam no Local).
        let extra = local.join("plugin-externo");
        fs::create_dir_all(&extra).unwrap();
        fs::write(extra.join("leia.txt"), "oi").unwrap();
        std::os::unix::fs::symlink(&extra, public.join("wp-content/plugins/linkado")).unwrap();
        // Wordfence: o PHP-FPM carrega este arquivo antes de tudo (precisa apontar para a cópia).
        fs::write(public.join(".user.ini"), format!("auto_prepend_file = '{old_path}/wordfence-waf.php'\n")).unwrap();
        fs::write(public.join("wordfence-waf.php"), format!("<?php\n// WFWAF_LOG_PATH: {old_path}/wp-content/wflogs/\n")).unwrap();

        let before = snapshot_tree(&local);
        let sql_mtime = fs::metadata(&dump).unwrap().modified().unwrap();

        // 2) Projeto novo recebe a pasta.
        let p = mk("lf", "public", 18850, 14150);
        scaffold(&base, &p).unwrap();
        let mut run = start(&base, &p).expect("start do projeto novo");
        let warning = crate::backup::restore_into(
            &base,
            &p,
            RestoreSource::Folder { public: &public, dump: &dump },
            "lf",
            &show,
        )
        .expect("restore da pasta");
        let warning = warning.expect("deveria avisar da pasta linkada");
        assert!(
            warning.contains("Pastas que eram links no Local foram copiadas") && warning.contains("wp-content/plugins/linkado"),
            "{warning}"
        );
        let copied = docroot_dir(&base, &p).join("wp-content/plugins/linkado");
        assert!(fs::symlink_metadata(&copied).unwrap().is_dir(), "o link deveria virar pasta de verdade");
        assert_eq!(fs::read_to_string(copied.join("leia.txt")).unwrap(), "oi");

        let script = crate::wp::prepare_terminal(&base, &p).expect("terminal");
        let shims = script.parent().unwrap().join("shims");
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("cd '{}' && wp option get siteurl", runtime_docroot(&base, &p)))
            .env("PATH", format!("{}:/usr/bin:/bin", shims.display()))
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let siteurl = text.lines().rev().map(str::trim).find(|l| l.starts_with("http")).unwrap_or("");
        assert_eq!(siteurl, project::site_url(&p), "saída: {text}");

        // O caminho do Local sumiu do banco (inclusive do valor serializado) e do .user.ini.
        let new_path = docroot_dir(&base, &p).display().to_string();
        let sh_out = |cmd: &str| {
            let out = Command::new("/bin/sh")
                .arg("-c")
                .arg(format!("cd '{}' && {cmd}", runtime_docroot(&base, &p)))
                .env("PATH", format!("{}:/usr/bin:/bin", shims.display()))
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).to_string()
        };
        let opt = sh_out("wp option get lt_caminho --format=json").replace("\\/", "/");
        assert!(opt.contains(&format!("{new_path}/wp-content/uploads")), "lt_caminho: {opt}");
        assert!(!opt.contains(&old_path), "lt_caminho ainda tem o caminho do Local: {opt}");
        let hits = sh_out(&format!(
            "mysql --skip-column-names -e \"SELECT COUNT(*) FROM wp_options WHERE option_value LIKE '%{old_path}%'\""
        ));
        assert_eq!(hits.trim(), "0", "o banco ainda tem o caminho do Local");
        // A gravação em disco adiada durante a importação volta ao padrão no fim.
        let durability = sh_out(
            "mysql --skip-column-names -e \"SELECT @@GLOBAL.innodb_flush_log_at_trx_commit, @@GLOBAL.sync_binlog\"",
        );
        assert_eq!(durability.split_whitespace().collect::<Vec<_>>(), ["1", "1"], "durabilidade: {durability}");
        let ini = fs::read_to_string(docroot_dir(&base, &p).join(".user.ini")).unwrap();
        assert!(!ini.contains(&old_path) && ini.contains(&new_path), ".user.ini: {ini}");
        let waf = fs::read_to_string(docroot_dir(&base, &p).join("wordfence-waf.php")).unwrap();
        assert!(!waf.contains(&old_path), "wordfence-waf.php: {waf}");

        let host = format!("Host: {}:{}\r\n", project::site_host(&p), p.http_port);
        let home = http_request(p.http_port, "GET", "/", &host, "");
        assert!(home.contains("200 OK"), "home: {}", home.lines().take(6).collect::<Vec<_>>().join("\n"));
        assert!(home.contains("Blog do Local"), "título não aparece na home");
        assert!(!home.contains("snap.local"), "a home ainda aponta para o endereço do Local");

        assert_eq!(snapshot_tree(&local), before, "a pasta do Local foi alterada");
        assert_eq!(fs::metadata(&dump).unwrap().modified().unwrap(), sql_mtime);

        // 3) Dump inválido: erro, e a origem continua intacta.
        let bad = std::env::temp_dir().join("localtusk-test-localsite-bad.sql");
        fs::write(&bad, "THIS IS NOT SQL;\n").unwrap();
        let err = crate::backup::restore_into(
            &base,
            &p,
            RestoreSource::Folder { public: &public, dump: &bad },
            "lf",
            &|_| {},
        )
        .unwrap_err();
        println!("erro esperado: {err}");
        assert_eq!(fs::read_to_string(&bad).unwrap(), "THIS IS NOT SQL;\n");
        assert_eq!(snapshot_tree(&local), before, "a pasta do Local foi alterada pela falha");
        assert_eq!(fs::metadata(&dump).unwrap().modified().unwrap(), sql_mtime);

        stop(&mut run);
        let _ = fs::remove_file(&bad);
        let _ = fs::remove_dir_all(project::project_dir(&base, "lf"));
        let _ = fs::remove_dir_all(&local);
    }

    /// Blueprint de verdade: um projeto com arquivo e dados vira modelo, e um segundo projeto
    /// (outro id e outras portas) nasce com os mesmos arquivos e o mesmo banco.
    /// Usa o Caddy, o MySQL e o PHP baixados pelo app para o teste. Rode com:
    /// `cargo test --lib blueprint_end_to_end -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn blueprint_end_to_end() {
        let base = seeded_base("bp", &["tools/caddy", "mysql", "php"]);
        let mk = |id: &str, http_start: u16, db_start: u16| {
            let http = project::free_port(http_start, &Default::default()).unwrap();
            let db = project::free_port(db_start, &Default::default()).unwrap();
            Project {
                id: id.into(),
                name: id.into(),
                docroot: "public".into(),
                php_version: "".into(),
                db_version: "".into(),
                http_port: http,
                db_port: db,
                db_name: "modelo".into(),
                created_at: 0,
                ..Default::default()
            }
        };
        let show = |_: InstallProgress| {};
        let page = r#"<?php
$pdo = new PDO('mysql:host='.getenv('DB_HOST').';port='.getenv('DB_PORT').';dbname='.getenv('DB_NAME'), getenv('DB_USER'), getenv('DB_PASSWORD'));
$pdo->exec('CREATE TABLE IF NOT EXISTS notas (id INT PRIMARY KEY AUTO_INCREMENT, texto VARCHAR(50))');
if (isset($_GET['grava'])) { $pdo->exec("INSERT INTO notas (texto) VALUES ('semente')"); }
echo 'notas=' . $pdo->query('SELECT COUNT(*) FROM notas')->fetchColumn();
"#;

        let a = mk("origem", 18600, 13900);
        scaffold(&base, &a).unwrap();
        fs::create_dir_all(docroot_dir(&base, &a)).unwrap();
        fs::write(docroot_dir(&base, &a).join("index.php"), page).unwrap();
        let mut run_a = start(&base, &a).expect("start origem");
        assert!(http_get_path(a.http_port, "/?grava=1").contains("notas=1"));

        let path = crate::blueprint::create(&base, &a, "Modelo de teste", "descrição", "bp", &show).expect("blueprint");
        let info = crate::blueprint::info(&path).unwrap();
        assert_eq!((info.name.as_str(), info.source_name.as_str(), info.docroot.as_str()), ("Modelo de teste", "origem", "public"));
        assert_eq!(crate::blueprint::list(&base).len(), 1);
        // O blueprint não aparece na lista de backups.
        assert!(crate::backup::list(&base, None).is_empty());
        stop(&mut run_a);

        let b = mk("copia", 18700, 14000);
        scaffold(&base, &b).unwrap();
        let mut run_b = start(&base, &b).expect("start cópia");
        let warning = crate::backup::restore_into(&base, &b, crate::backup::RestoreSource::Archive(&path), "bp", &show).expect("restore");
        assert!(warning.is_none(), "aviso inesperado: {warning:?}");
        let body = http_get_path(b.http_port, "/");
        assert!(body.contains("notas=1"), "o novo projeto não herdou os dados: {body}");
        stop(&mut run_b);

        crate::blueprint::delete(&base, &path).unwrap();
        assert!(crate::blueprint::list(&base).is_empty());
        let _ = fs::remove_dir_all(&base);
    }

    fn runtime_docroot(base: &Path, p: &Project) -> String {
        docroot_dir(base, p).display().to_string()
    }

    fn caddy_sample() -> Project {
        Project { id: "loja".into(), http_port: 8000, db_port: 3307, ..Default::default() }
    }

    fn render(p: &Project, lan_hosts: &[String]) -> String {
        render_caddyfile(p, "/p/app", "/p/logs", 9000, None, lan_hosts)
    }

    #[test]
    fn caddyfile_padrao_e_so_http_local() {
        let c = render(&caddy_sample(), &["192.168.0.10".into()]);
        assert!(c.contains("auto_https off"));
        assert!(c.contains(":8000 {\n\tbind 127.0.0.1\n\timport app"));
        assert!(!c.contains("tls internal") && !c.contains("0.0.0.0") && !c.contains("192.168"));
    }

    #[test]
    fn caddyfile_https_usa_a_ca_local_e_nao_redireciona() {
        let mut p = caddy_sample();
        p.https = true;
        p.https_port = 8443;
        let c = render(&p, &["192.168.0.10".into()]);
        assert!(c.contains("auto_https disable_redirects") && c.contains("skip_install_trust"));
        assert!(!c.contains("auto_https off"));
        assert!(c.contains("https://loja.localhost:8443, https://127.0.0.1:8443 {\n\tbind 127.0.0.1\n\ttls internal"));
        assert!(!c.contains("192.168"), "o IP da rede só entra com a rede ligada");
        // O HTTP continua disponível.
        assert!(c.contains(":8000 {"));
    }

    #[test]
    fn caddyfile_rede_escuta_em_todas_as_interfaces_e_inclui_os_nomes_no_certificado() {
        let mut p = caddy_sample();
        p.lan = true;
        p.https = true;
        p.https_port = 8443;
        let c = render(&p, &["192.168.0.10".into(), "meu-mac.local".into()]);
        assert_eq!(c.matches("bind 0.0.0.0").count(), 2, "{c}");
        assert!(!c.contains("bind 127.0.0.1"));
        assert!(c.contains("https://192.168.0.10:8443") && c.contains("https://meu-mac.local:8443"));
    }

    #[test]
    fn caddyfile_rede_sem_https_nao_cria_site_tls() {
        let mut p = caddy_sample();
        p.lan = true;
        let c = render(&p, &["192.168.0.10".into()]);
        assert!(c.contains(":8000 {\n\tbind 0.0.0.0"));
        assert!(!c.contains("tls internal"));
    }

    #[test]
    fn adminer_continua_so_local_mesmo_com_a_rede_ligada() {
        let mut p = caddy_sample();
        p.lan = true;
        let c = render_caddyfile(&p, "/p/app", "/p/logs", 9000, Some((9100, "/tools".into())), &[]);
        let admin = &c[c.find(":9100 {").unwrap()..];
        assert!(admin.starts_with(":9100 {\n\tbind 127.0.0.1"), "{admin}");
    }

    /// HTTPS de verdade: o Caddy emite o certificado da CA local, o PHP enxerga `HTTPS=on` e o
    /// certificado vale para `<id>.localhost` (verificado com o certificado raiz, sem confiar no sistema).
    /// Usa o Caddy, o MySQL e o PHP baixados pelo app para o teste. Rode com:
    /// `cargo test --lib https_end_to_end -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn https_end_to_end() {
        let base = seeded_base("https", &["tools/caddy", "mysql", "php"]);
        let http = project::free_port(18400, &Default::default()).unwrap();
        let https = project::free_port(18450, &Default::default()).unwrap();
        let db = project::free_port(13700, &Default::default()).unwrap();
        let p = Project {
            id: "seguro".into(),
            name: "Seguro".into(),
            http_port: http,
            https_port: https,
            https: true,
            db_port: db,
            db_name: "seguro".into(),
            ..Default::default()
        };
        scaffold(&base, &p).unwrap();
        fs::write(docroot_dir(&base, &p).join("index.php"), "<?php echo 'https=' . ($_SERVER['HTTPS'] ?? 'off');").unwrap();
        let mut running = start(&base, &p).expect("start falhou");

        let root = crate::ssl::root_cert(&base);
        assert!(root.is_file(), "a CA deveria ter sido criada em {}", root.display());
        let url = format!("https://{}:{https}/", project::site_host(&p));
        let out = Command::new("curl")
            .args(["-s", "--max-time", "10", "--cacert"])
            .arg(&root)
            .arg(&url)
            .output()
            .unwrap();
        assert!(out.status.success(), "curl: {} / {}", out.status, String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout), "https=on");
        // Sem a CA, o certificado não é confiável (comprova que não é um site HTTP disfarçado).
        let bad = Command::new("curl").args(["-s", "--max-time", "10", "--cacert", "/dev/null", &url]).status().unwrap();
        assert!(!bad.success());
        // O HTTP continua respondendo.
        assert!(http_get(http).contains("https=off"));
        // E o status da CA sabe que ela existe.
        assert!(crate::ssl::status(&base).ca_ready);

        stop(&mut running);
        assert!(project::port_is_free(https), "Caddy ainda no ar");
        let _ = fs::remove_dir_all(&base);
    }

    /// Acesso pela rede: desligado, o IP da rede recusa a conexão; ligado, responde por HTTP e
    /// por HTTPS (certificado válido para o IP). Rode com:
    /// `cargo test --lib lan_end_to_end -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn lan_end_to_end() {
        let Some(ip) = project::lan_ip() else {
            println!("sem rede local: teste ignorado");
            return;
        };
        let base = seeded_base("lan", &["tools/caddy", "mysql", "php"]);
        let http = project::free_port(18500, &Default::default()).unwrap();
        let https = project::free_port(18550, &Default::default()).unwrap();
        let db = project::free_port(13800, &Default::default()).unwrap();
        let mut p = Project {
            id: "rede".into(),
            name: "Rede".into(),
            http_port: http,
            https_port: https,
            https: true,
            db_port: db,
            db_name: "rede".into(),
            ..Default::default()
        };
        scaffold(&base, &p).unwrap();
        fs::write(docroot_dir(&base, &p).join("index.php"), "<?php echo 'ola ' . $_SERVER['HTTP_HOST'];").unwrap();
        let curl = |extra: &[&str], url: &str| {
            Command::new("curl").args(["-s", "--max-time", "10"]).args(extra).arg(url).output().unwrap()
        };

        // 1) Só local: o IP da rede recusa.
        let mut running = start(&base, &p).expect("start falhou");
        let blocked = curl(&[], &format!("http://{ip}:{http}/"));
        assert!(!blocked.status.success(), "sem a rede ligada o IP da rede deveria recusar");
        assert!(http_get(http).contains("ola"), "o acesso local deveria funcionar");
        stop(&mut running);

        // 2) Rede ligada.
        p.lan = true;
        let mut running = start(&base, &p).expect("start falhou");
        let ok = curl(&[], &format!("http://{ip}:{http}/"));
        assert_eq!(String::from_utf8_lossy(&ok.stdout), format!("ola {ip}:{http}"));
        let root = crate::ssl::root_cert(&base);
        let secure = curl(&["--cacert", root.to_str().unwrap()], &format!("https://{ip}:{https}/"));
        assert!(secure.status.success(), "HTTPS pelo IP: {}", String::from_utf8_lossy(&secure.stderr));
        assert_eq!(String::from_utf8_lossy(&secure.stdout), format!("ola {ip}:{https}"));
        // O banco e o Adminer continuam só locais.
        let blocked_db = std::net::TcpStream::connect_timeout(&SocketAddr::from((ip, db)), Duration::from_secs(2));
        assert!(blocked_db.is_err(), "o MySQL não pode ficar exposto na rede");
        stop(&mut running);
        let _ = fs::remove_dir_all(&base);
    }
}
