//! Linha de comando de um projeto: PHP CLI, WP-CLI, terminal configurado e a instalação
//! do WordPress (que usa o próprio WP-CLI).
//!
//! Tudo roda com o mesmo PHP, `php.ini` e variáveis de banco do projeto, então `wp` e `php`
//! enxergam exatamente o que o site enxerga.

use crate::download::InstallProgress;
use crate::project::{self, DB_PASSWORD, DB_USER, Project};
use crate::{extras, mysql_manager, php_manager, postgres_manager, runtime, tools, valkey_manager};
use serde::Deserialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Log com a saída dos comandos do WP-CLI executados pelo app.
pub const LOG: &str = "wp-cli.log";

/// Trecho do `index.php` de exemplo criado com o projeto (permite sobrescrevê-lo).
/// Textos que identificam o `index.php` de exemplo (inclui o do nome antigo, LocalPHP, dos projetos já criados).
const SAMPLE_MARKERS: [&str; 2] = ["Seu projeto está rodando no LocalTusk", "Seu projeto está rodando no LocalPHP"];

fn conf_dir(base: &Path, p: &Project) -> PathBuf {
    project::project_dir(base, &p.id).join("conf")
}

/// O projeto já tem um WordPress na pasta pública?
pub fn is_wordpress(base: &Path, p: &Project) -> bool {
    runtime::docroot_dir(base, p).join("wp-includes").join("version.php").is_file()
}

/// Arquivo que guarda o endereço do site que o WordPress tem gravado no banco, para
/// detectar quando o projeto passa de HTTP para HTTPS (ou o contrário).
fn url_marker(base: &Path, p: &Project) -> PathBuf {
    conf_dir(base, p).join("site-url")
}

/// Guarda o endereço atual, se ainda não houver um guardado. Chamado antes de mudar de
/// HTTP para HTTPS, para a mudança ser notada na próxima inicialização.
pub fn remember_site_url(base: &Path, p: &Project) {
    let marker = url_marker(base, p);
    if is_wordpress(base, p) && !marker.is_file() {
        let _ = fs::create_dir_all(conf_dir(base, p));
        let _ = fs::write(marker, project::site_url(p));
    }
}

/// Com o projeto rodando: se o endereço do site mudou (HTTP <-> HTTPS) desde que o
/// WordPress foi configurado, troca o endereço antigo pelo novo no banco.
pub fn sync_site_url(base: &Path, p: &Project, progress: &dyn Fn(InstallProgress)) -> Result<(), String> {
    if !is_wordpress(base, p) {
        return Ok(());
    }
    let marker = url_marker(base, p);
    let want = project::site_url(p);
    match fs::read_to_string(&marker) {
        Ok(saved) if saved.trim() == want => Ok(()),
        Ok(_) => {
            retarget(base, p, progress)?;
            fs::write(&marker, want).map_err(|e| e.to_string())
        }
        Err(_) => {
            // Projeto anterior a este controle: o banco está com o endereço de agora.
            let _ = fs::create_dir_all(conf_dir(base, p));
            fs::write(&marker, want).map_err(|e| e.to_string())
        }
    }
}

/// Depois de restaurar um backup num projeto com outro banco/porta/domínio: aponta o
/// `wp-config.php` para o banco deste projeto e troca o endereço antigo do site pelo atual.
pub fn retarget(base: &Path, p: &Project, progress: &dyn Fn(InstallProgress)) -> Result<(), String> {
    let php = php_cli(base, p, progress)?;
    if extras::wpcli_phar(base).is_none() {
        extras::install(base, "wpcli", progress)?;
    }
    let run = |label: &str, args: &[String]| -> Result<(), String> {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let cmd = wp_command(base, p, &php, &refs)?;
        run_logged(base, p, label, cmd, Duration::from_secs(600))
    };
    let s = |v: &str| v.to_string();
    run("wp config set DB_NAME", &[s("config"), s("set"), s("DB_NAME"), p.db_name.clone()])?;
    run(
        "wp config set DB_HOST",
        &[s("config"), s("set"), s("DB_HOST"), format!("127.0.0.1:{}", p.db_port)],
    )?;

    // Endereço que o WordPress tem gravado no banco restaurado.
    let out = wp_command(base, p, &php, &["option", "get", "siteurl"])?
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("wp option get siteurl: {e}"))?;
    let old = String::from_utf8_lossy(&out.stdout)
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| l.starts_with("http://") || l.starts_with("https://"))
        .unwrap_or("")
        .trim_end_matches('/')
        .to_string();
    let new = project::site_url(p);
    if !old.is_empty() && old != new {
        run(
            "wp search-replace",
            &[s("search-replace"), old, new, s("--all-tables"), s("--skip-columns=guid")],
        )?;
    }
    Ok(())
}

/// Escapa um texto para uso entre aspas simples em scripts de shell.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// PHP de linha de comando da versão escolhida pelo projeto (baixa o CLI se faltar).
fn php_cli(base: &Path, p: &Project, progress: &dyn Fn(InstallProgress)) -> Result<PathBuf, String> {
    let inst = tools::pick_php(base, &p.php_version)
        .ok_or("Nenhum PHP instalado. Instale uma versão em \"PHP e ferramentas\".")?;
    if inst.managed {
        php_manager::ensure_cli(base, &inst.full_version, progress)?;
    }
    // Projetos com PostgreSQL usam o PHP do pacote "common" (com `pdo_pgsql`), se já baixado.
    php_manager::cli_for(base, &inst, project::is_postgres(p))
        .ok_or_else(|| format!("O PHP {} não tem a versão de linha de comando (CLI).", inst.full_version))
}

fn project_env(p: &Project) -> Vec<(&'static str, String)> {
    let postgres = project::is_postgres(p);
    let mut env = vec![
        ("DB_HOST", "127.0.0.1".into()),
        ("DB_PORT", p.db_port.to_string()),
        ("DB_NAME", p.db_name.clone()),
        ("DB_USER", DB_USER.into()),
        ("DB_PASSWORD", DB_PASSWORD.into()),
        ("DB_ENGINE", project::engine_of(p).into()),
        ("DB_CONNECTION", if postgres { "pgsql" } else { "mysql" }.into()),
    ];
    if project::redis_on(p) {
        env.push(("REDIS_HOST", "127.0.0.1".into()));
        env.push(("REDIS_PORT", p.redis_port.to_string()));
        env.push(("REDIS_URL", format!("redis://127.0.0.1:{}", p.redis_port)));
    }
    env
}

/// Argumentos do PHP CLI que o deixam igual ao PHP do site.
fn php_args(base: &Path, p: &Project) -> Vec<String> {
    let mut args = vec![];
    let ini = conf_dir(base, p).join("php.ini");
    if ini.is_file() {
        args.push("-c".to_string());
        args.push(ini.display().to_string());
    }
    args.push("-d".into());
    args.push("memory_limit=512M".into());
    args
}

fn wp_command(base: &Path, p: &Project, php: &Path, args: &[&str]) -> Result<Command, String> {
    let phar = extras::wpcli_phar(base).ok_or("O WP-CLI não está instalado.")?;
    let docroot = runtime::docroot_dir(base, p);
    let mut c = Command::new(php);
    c.args(php_args(base, p))
        .arg(phar)
        .arg(format!("--path={}", docroot.display()))
        .args(args)
        .current_dir(&docroot)
        .env("WP_CLI_PHP", php)
        .env("WP_CLI_DISABLE_AUTO_CHECK_UPDATE", "1");
    for (k, v) in project_env(p) {
        c.env(k, v);
    }
    Ok(c)
}

/// Roda um comando guardando a saída em `logs/wp-cli.log`. Em caso de erro, devolve as
/// últimas linhas da saída.
fn run_logged(
    base: &Path,
    p: &Project,
    label: &str,
    mut cmd: Command,
    timeout: Duration,
) -> Result<(), String> {
    let log_path = project::project_dir(base, &p.id).join("logs").join(LOG);
    if let Some(dir) = log_path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| e.to_string())?;
    let _ = writeln!(log, "\n$ {label}");

    cmd.stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(log));
    let mut child = cmd.spawn().map_err(|e| format!("{label}: {e}"))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(format!("{label} falhou ({status})\n\n{}", last_output(&log_path)));
            }
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{label}: demorou demais e foi cancelado"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(format!("{label}: {e}")),
        }
    }
}

/// Saída do último comando registrado no log (no máximo 8 linhas).
fn last_output(log: &Path) -> String {
    let text = fs::read_to_string(log).unwrap_or_default();
    let last = text.rfind("\n$ ").map(|i| &text[i + 1..]).unwrap_or(&text);
    let lines: Vec<&str> = last.lines().skip(1).filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(8)..].join("\n")
}

// ------------------------------------------------------------------ WordPress

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WpOptions {
    pub title: String,
    pub admin_user: String,
    pub admin_password: String,
    pub admin_email: String,
    /// Idioma do WordPress (`pt_BR`, `en_US`…).
    pub locale: String,
    /// Apaga o que houver na pasta pública (que não seja o `index.php` de exemplo) antes de instalar.
    #[serde(default)]
    pub replace_existing: bool,
}

fn validate(o: &WpOptions) -> Result<(), String> {
    if o.title.trim().is_empty() {
        return Err("informe o título do site".into());
    }
    let user_ok = !o.admin_user.is_empty()
        && o.admin_user.chars().all(|c| c.is_ascii_alphanumeric() || "._-@".contains(c));
    if !user_ok {
        return Err("o usuário admin só pode ter letras, números e . _ - @".into());
    }
    if o.admin_password.chars().count() < 8 {
        return Err("a senha precisa ter pelo menos 8 caracteres".into());
    }
    if !o.admin_email.contains('@') || o.admin_email.contains(char::is_whitespace) {
        return Err("e-mail inválido".into());
    }
    let mut parts = o.locale.splitn(2, '_');
    let lang_ok = parts.next().is_some_and(|l| (2..=3).contains(&l.len()) && l.chars().all(|c| c.is_ascii_lowercase()));
    let region_ok = parts.next().is_none_or(|r| (2..=3).contains(&r.len()) && r.chars().all(|c| c.is_ascii_uppercase()));
    if !lang_ok || !region_ok {
        return Err("idioma inválido".into());
    }
    Ok(())
}

/// Itens da pasta pública que não são o `index.php` de exemplo criado pelo LocalTusk.
fn foreign_entries(docroot: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(docroot) else {
        return vec![];
    };
    rd.flatten()
        .filter(|e| {
            let is_sample = e.file_name() == "index.php"
                && fs::read_to_string(e.path()).is_ok_and(|c| SAMPLE_MARKERS.iter().any(|m| c.contains(m)));
            !is_sample
        })
        .map(|e| e.path())
        .collect()
}

/// A pasta pública tem arquivos do usuário (além do `index.php` de exemplo)?
pub fn docroot_in_use(base: &Path, p: &Project) -> bool {
    !foreign_entries(&runtime::docroot_dir(base, p)).is_empty()
}

/// Deixa a pasta pública pronta para receber o WordPress. Se houver arquivos que não são
/// do projeto de exemplo, só segue adiante com `replace` (confirmado pelo usuário), e então
/// apaga o conteúdo da pasta. Um WordPress já instalado nunca é apagado.
fn prepare_docroot(base: &Path, p: &Project, replace: bool) -> Result<(), String> {
    if is_wordpress(base, p) {
        return Err("este projeto já tem o WordPress instalado".into());
    }
    let docroot = runtime::docroot_dir(base, p);
    let foreign = foreign_entries(&docroot);
    if foreign.is_empty() {
        return Ok(());
    }
    if !replace {
        return Err(format!(
            "A pasta pública já tem arquivos ({} item(ns)). Confirme para apagá-los e instalar o WordPress.",
            foreign.len()
        ));
    }
    // Trava de segurança: só apaga dentro da pasta `app` do próprio projeto.
    let app = fs::canonicalize(runtime::app_dir(base, &p.id)).map_err(|e| e.to_string())?;
    let target = fs::canonicalize(&docroot).map_err(|e| e.to_string())?;
    if !target.starts_with(&app) {
        return Err("a pasta pública está fora da pasta do projeto".into());
    }
    for path in foreign {
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        // Links simbólicos são removidos sem seguir o destino.
        let res = if meta.is_dir() { fs::remove_dir_all(&path) } else { fs::remove_file(&path) };
        res.map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

/// Baixa o WordPress, cria o `wp-config.php` com o banco do projeto e roda a instalação.
/// O banco do projeto precisa estar rodando.
pub fn install_wordpress(
    base: &Path,
    p: &Project,
    o: &WpOptions,
    progress: &dyn Fn(InstallProgress),
) -> Result<(), String> {
    if project::is_postgres(p) {
        return Err("O WordPress só funciona com MySQL. Crie o projeto com MySQL para instalá-lo.".into());
    }
    validate(o)?;
    prepare_docroot(base, p, o.replace_existing)?;

    let key = format!("wordpress-{}", p.id);
    let step = |msg: &str| {
        progress(InstallProgress {
            id: key.clone(),
            stage: "extract".into(),
            downloaded: 0,
            total: 0,
            message: Some(msg.into()),
        })
    };

    step("Preparando o PHP CLI e o WP-CLI…");
    let php = php_cli(base, p, progress)?;
    if extras::wpcli_phar(base).is_none() {
        extras::install(base, "wpcli", progress)?;
    }

    let long = Duration::from_secs(300);
    let short = Duration::from_secs(120);
    let url = crate::project::site_url(p);
    let run = |label: &str, args: &[String], timeout: Duration| -> Result<(), String> {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let cmd = wp_command(base, p, &php, &refs)?;
        run_logged(base, p, label, cmd, timeout)
    };
    let s = |v: &str| v.to_string();

    step("Baixando o WordPress…");
    run(
        "wp core download",
        &[s("core"), s("download"), format!("--locale={}", o.locale), s("--force")],
        long,
    )?;

    step("Criando o wp-config.php…");
    run(
        "wp config create",
        &[
            s("config"),
            s("create"),
            format!("--dbname={}", p.db_name),
            format!("--dbuser={DB_USER}"),
            format!("--dbpass={DB_PASSWORD}"),
            format!("--dbhost=127.0.0.1:{}", p.db_port),
            s("--force"),
        ],
        short,
    )?;
    // Modo de depuração útil em desenvolvimento: erros vão para wp-content/debug.log.
    for (name, value) in [("WP_DEBUG", "true"), ("WP_DEBUG_LOG", "true"), ("WP_DEBUG_DISPLAY", "false")] {
        let _ = run(
            "wp config set",
            &[s("config"), s("set"), s(name), s(value), s("--raw")],
            short,
        );
    }

    step("Instalando o WordPress…");
    run(
        "wp core install",
        &[
            s("core"),
            s("install"),
            format!("--url={url}"),
            format!("--title={}", o.title.trim()),
            format!("--admin_user={}", o.admin_user),
            format!("--admin_password={}", o.admin_password),
            format!("--admin_email={}", o.admin_email),
            s("--skip-email"),
        ],
        short,
    )?;

    // Opcionais: não impedem o uso se falharem (ex.: sem internet para o pacote de idioma).
    if o.locale != "en_US" {
        let _ = run(
            "wp language core install",
            &[s("language"), s("core"), s("install"), o.locale.clone(), s("--activate")],
            short,
        );
    }
    // Links permanentes bonitos (o Caddy já reescreve tudo para o index.php).
    let _ = run(
        "wp rewrite structure",
        &[s("rewrite"), s("structure"), s("/%postname%/")],
        short,
    );
    Ok(())
}

// ------------------------------------------------------------------ Terminal

/// Abre um terminal na pasta do projeto com `php`, `wp`, `mysql`… apontando para o PHP,
/// o banco e o `php.ini` deste projeto.
pub fn open_terminal(base: &Path, p: &Project) -> Result<(), String> {
    match std::env::consts::OS {
        "macos" => {
            let script_path = prepare_terminal(base, p)?;
            let status = Command::new("open")
                .arg("-a")
                .arg("Terminal")
                .arg(&script_path)
                .status()
                .map_err(|e| format!("não foi possível abrir o Terminal: {e}"))?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("o Terminal não abriu ({status})"))
            }
        }
        "linux" => {
            let script_path = prepare_terminal(base, p)?;
            open_linux_terminal(&script_path)
        }
        "windows" => {
            let bat = prepare_terminal(base, p)?;
            Command::new("cmd")
                .args(["/C", "start", "", "cmd", "/K", "call", &bat.display().to_string()])
                .spawn()
                .map_err(|e| format!("não foi possível abrir o terminal: {e}"))?;
            Ok(())
        }
        _ => Err("O terminal integrado não está disponível neste sistema por enquanto.".into()),
    }
}

/// Emuladores de terminal do Linux e o jeito de pedir a cada um que rode um programa.
/// O `$TERMINAL` da pessoa vem primeiro; `x-terminal-emulator` é o padrão do Debian/Ubuntu.
fn linux_terminals(script: &Path, preferred: Option<String>) -> Vec<(String, Vec<String>)> {
    let script = script.display().to_string();
    let with = |flag: &[&str]| -> Vec<String> {
        flag.iter().map(|f| f.to_string()).chain(std::iter::once(script.clone())).collect()
    };
    let mut list: Vec<(String, Vec<String>)> = vec![];
    if let Some(t) = preferred.filter(|t| !t.trim().is_empty()) {
        list.push((t, with(&["-e"])));
    }
    for (name, flag) in [
        ("x-terminal-emulator", &["-e"][..]),
        ("gnome-terminal", &["--"][..]),
        ("kgx", &["-e"][..]),
        ("konsole", &["-e"][..]),
        ("xfce4-terminal", &["-x"][..]),
        ("mate-terminal", &["-x"][..]),
        ("tilix", &["-e"][..]),
        ("kitty", &[][..]),
        ("alacritty", &["-e"][..]),
        ("wezterm", &["start", "--"][..]),
        ("xterm", &["-e"][..]),
    ] {
        list.push((name.to_string(), with(flag)));
    }
    list
}

fn open_linux_terminal(script: &Path) -> Result<(), String> {
    let mut tried = vec![];
    for (name, args) in linux_terminals(script, std::env::var("TERMINAL").ok()) {
        let Some(bin) = tools::find_in_system(&name) else {
            continue;
        };
        tried.push(name.clone());
        let child = Command::new(&bin)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            continue;
        };
        // Alguns terminais falham na hora (sem tela, argumento não aceito): nesse caso tenta o próximo.
        std::thread::sleep(std::time::Duration::from_millis(500));
        match child.try_wait() {
            Ok(Some(status)) if !status.success() => continue,
            _ => return Ok(()),
        }
    }
    if tried.is_empty() {
        Err("Não achei um emulador de terminal. Instale um (por exemplo gnome-terminal, konsole ou xterm) ou defina a variável TERMINAL.".into())
    } else {
        Err(format!("Nenhum terminal abriu (tentei: {}).", tried.join(", ")))
    }
}

/// Gera os atalhos (`conf/shims`) e o script de entrada do terminal; devolve o script.
pub(crate) fn prepare_terminal(base: &Path, p: &Project) -> Result<PathBuf, String> {
    let php = php_cli(base, p, &|_| {})?;
    let conf = conf_dir(base, p);
    let shims = conf.join("shims");
    let shell_dir = conf.join("shell");
    for d in [&shims, &shell_dir] {
        fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }

    let php_extra = php_args(base, p).iter().map(|a| sh_quote(a)).collect::<Vec<_>>().join(" ");
    write_exec(
        &shims.join("php"),
        &format!("#!/bin/sh\nexec {} {php_extra} \"$@\"\n", sh_quote(&php.display().to_string())),
    )?;
    let phar = extras::dir(base, "wpcli").join("wp-cli.phar");
    write_exec(
        &shims.join("wp"),
        &format!(
            "#!/bin/sh\n\
             PHAR={phar}\n\
             if [ ! -f \"$PHAR\" ]; then\n\
             \techo 'O WP-CLI não está instalado. Instale-o em \"PHP e ferramentas\" no LocalTusk.' >&2\n\
             \texit 1\n\
             fi\n\
             export WP_CLI_PHP={shim}\n\
             export WP_CLI_DISABLE_AUTO_CHECK_UPDATE=1\n\
             exec {php} {php_extra} \"$PHAR\" \"$@\"\n",
            phar = sh_quote(&phar.display().to_string()),
            shim = sh_quote(&shims.join("php").display().to_string()),
            php = sh_quote(&php.display().to_string()),
        ),
    )?;

    // Clientes do PostgreSQL: o terminal exporta `PGHOST`, `PGPORT`, `PGUSER`, `PGPASSWORD` e
    // `PGDATABASE`, então os atalhos só precisam apontar para os executáveis.
    if project::is_postgres(p) {
        if let Some(pg) = postgres_manager::pick(base, &p.db_version) {
            for tool in ["psql", "pg_dump", "pg_dumpall", "pg_restore", "createdb", "pg_isready"] {
                if let Some(real) = pg.tool(tool) {
                    write_exec(&shims.join(tool), &format!("#!/bin/sh\nexec {} \"$@\"\n", sh_quote(&real.display().to_string())))?;
                }
            }
        }
    }

    // Cliente do Redis (Valkey) já ligado ao Redis do projeto.
    if project::redis_on(p) {
        if let Some(cli) = valkey_manager::pick(base).and_then(|v| v.cli()) {
            let script = format!(
                "#!/bin/sh\nexec {} -h 127.0.0.1 -p {} \"$@\"\n",
                sh_quote(&cli.display().to_string()),
                p.redis_port
            );
            write_exec(&shims.join("redis-cli"), &script)?;
            write_exec(&shims.join("valkey-cli"), &script)?;
        }
    }

    // Clientes do MySQL já ligados ao banco do projeto (se houver MySQL instalado).
    let mysql_bin_dir = if project::is_postgres(p) {
        None
    } else {
        mysql_manager::pick(base, &p.db_version).and_then(|i| i.mysqld.parent().map(Path::to_path_buf))
    };
    if let Some(bin_dir) = mysql_bin_dir {
        for tool in ["mysql", "mysqldump", "mysqladmin"] {
            let real = bin_dir.join(tool);
            if real.is_file() {
                // O cliente `mysql` já abre o banco do projeto.
                let database = if tool == "mysql" { format!(" --database={}", sh_quote(&p.db_name)) } else { String::new() };
                write_exec(
                    &shims.join(tool),
                    &format!(
                        "#!/bin/sh\nexport MYSQL_PWD={pw}\nexec {real} --host=127.0.0.1 --port={port} --user={user}{database} \"$@\"\n",
                        pw = sh_quote(DB_PASSWORD),
                        real = sh_quote(&real.display().to_string()),
                        port = p.db_port,
                        user = DB_USER,
                    ),
                )?;
            }
        }
    }

    // Os arquivos de inicialização do shell colocam os atalhos no começo do PATH
    // *depois* do ~/.zshrc / ~/.bashrc do usuário, para que nada os esconda.
    fs::write(shell_dir.join(".zshenv"), "[ -r \"$HOME/.zshenv\" ] && source \"$HOME/.zshenv\"\n")
        .map_err(|e| e.to_string())?;
    fs::write(
        shell_dir.join(".zshrc"),
        "export ZDOTDIR=\"$HOME\"\n\
         [ -r \"$HOME/.zshrc\" ] && source \"$HOME/.zshrc\"\n\
         export PATH=\"$LOCALTUSK_SHIMS:$PATH\"\n\
         PROMPT=\"%F{magenta}[$LOCALTUSK_PROJECT]%f $PROMPT\"\n",
    )
    .map_err(|e| e.to_string())?;
    fs::write(
        shell_dir.join("bashrc"),
        "[ -r \"$HOME/.bashrc\" ] && . \"$HOME/.bashrc\"\n\
         export PATH=\"$LOCALTUSK_SHIMS:$PATH\"\n\
         PS1=\"[$LOCALTUSK_PROJECT] $PS1\"\n",
    )
    .map_err(|e| e.to_string())?;


/// Aspas para linhas de comando do `cmd` (batch).
#[cfg(windows)]
fn winq(s: &str) -> String {
    format!("\"{}\"", s.replace('"', ""))
}

#[cfg(windows)]
fn write_bat(path: &Path, lines: &[String]) -> Result<(), String> {
    fs::write(path, lines.join("\r\n") + "\r\n").map_err(|e| format!("{}: {e}", path.display()))
}

/// Gera os atalhos `.bat` e o `terminal.bat` do Windows; devolve o caminho do script.
#[cfg(windows)]
fn write_win_terminal(base: &Path, p: &Project, php: &Path, conf: &Path, shims: &Path) -> Result<PathBuf, String> {
    let bat = |name: &str, lines: Vec<String>| write_bat(&shims.join(format!("{name}.bat")), &lines);
    let extra = php_args(base, p).iter().map(|a| winq(a)).collect::<Vec<_>>().join(" ");
    bat("php", vec!["@echo off".into(), format!("{} {extra} %*", winq(&php.display().to_string()))])?;
    let phar = extras::dir(base, "wpcli").join("wp-cli.phar");
    bat(
        "wp",
        vec![
            "@echo off".into(),
            format!("if not exist {} (echo O WP-CLI nao esta instalado. Instale-o em PHP e ferramentas no LocalTusk. & exit /b 1)", winq(&phar.display().to_string())),
            format!("set \"WP_CLI_PHP={}\"", shims.join("php.bat").display()),
            "set \"WP_CLI_DISABLE_AUTO_CHECK_UPDATE=1\"".into(),
            format!("{} {extra} {} %*", winq(&php.display().to_string()), winq(&phar.display().to_string())),
        ],
    )?;
    if project::is_postgres(p) {
        if let Some(pg) = postgres_manager::pick(base, &p.db_version) {
            for tool in ["psql", "pg_dump", "pg_dumpall", "pg_restore", "createdb", "pg_isready"] {
                if let Some(real) = pg.tool(tool) {
                    bat(tool, vec!["@echo off".into(), format!("{} %*", winq(&real.display().to_string()))])?;
                }
            }
        }
    }
    if project::redis_on(p) {
        if let Some(cli) = valkey_manager::pick(base).and_then(|v| v.cli()) {
            let lines = vec!["@echo off".into(), format!("{} -h 127.0.0.1 -p {} %*", winq(&cli.display().to_string()), p.redis_port)];
            bat("redis-cli", lines.clone())?;
            bat("valkey-cli", lines)?;
        }
    }
    if !project::is_postgres(p) {
        if let Some(bin_dir) = mysql_manager::pick(base, &p.db_version).and_then(|i| i.mysqld.parent().map(Path::to_path_buf)) {
            for tool in ["mysql", "mysqldump", "mysqladmin"] {
                let real = crate::platform::bin(&bin_dir, tool);
                if real.is_file() {
                    let database = if tool == "mysql" { format!(" --database={}", p.db_name) } else { String::new() };
                    bat(
                        tool,
                        vec![
                            "@echo off".into(),
                            format!("set \"MYSQL_PWD={}\"", DB_PASSWORD),
                            format!("{} --host=127.0.0.1 --port={} --user={}{} %*", winq(&real.display().to_string()), p.db_port, DB_USER, database),
                        ],
                    )?;
                }
            }
        }
    }
    let clean = |t: &str| t.replace(['&', '|', '<', '>', '^', '%'], " ");
    let mut out: Vec<String> = vec!["@echo off".into(), format!("set \"LOCALTUSK_PROJECT={}\"", p.id), format!("set \"PATH={};%PATH%\"", shims.display())];
    for (k, v) in project_env(p) {
        out.push(format!("set \"{k}={v}\""));
    }
    if project::is_postgres(p) {
        out.push("set \"PGHOST=127.0.0.1\"".into());
        out.push(format!("set \"PGPORT={}\"", p.db_port));
        out.push(format!("set \"PGUSER={}\"", DB_USER));
        out.push(format!("set \"PGPASSWORD={}\"", DB_PASSWORD));
        out.push(format!("set \"PGDATABASE={}\"", p.db_name));
    } else {
        out.push("set \"MYSQL_HOST=127.0.0.1\"".into());
        out.push(format!("set \"MYSQL_TCP_PORT={}\"", p.db_port));
    }
    out.push(format!("cd /d {}", winq(&runtime::docroot_dir(base, p).display().to_string())));
    out.push("cls".into());
    out.push(format!("echo LocalTusk - {}", clean(&p.name)));
    out.push(format!("echo {}", clean(&terminal_banner(p))));
    out.push("echo.".into());
    write_bat(&conf.join("terminal.bat"), &out)?;
    Ok(conf.join("terminal.bat"))
}

    #[cfg(windows)]
    {
        return write_win_terminal(base, p, &php, &conf, &shims);
    }
    let mut script = String::from("#!/bin/zsh\n");
    script.push_str(&format!("export LOCALTUSK_PROJECT={}\n", sh_quote(&p.id)));
    script.push_str(&format!("export LOCALTUSK_SHIMS={}\n", sh_quote(&shims.display().to_string())));
    for (k, v) in project_env(p) {
        script.push_str(&format!("export {k}={}\n", sh_quote(&v)));
    }
    if project::is_postgres(p) {
        script.push_str(&format!(
            "export PGHOST=127.0.0.1\nexport PGPORT={}\nexport PGUSER={}\nexport PGPASSWORD={}\nexport PGDATABASE={}\n",
            p.db_port,
            sh_quote(DB_USER),
            sh_quote(DB_PASSWORD),
            sh_quote(&p.db_name)
        ));
    } else {
        script.push_str(&format!("export MYSQL_HOST=127.0.0.1\nexport MYSQL_TCP_PORT={}\n", p.db_port));
    }
    script.push_str(&format!("cd {}\n", sh_quote(&runtime::docroot_dir(base, p).display().to_string())));
    script.push_str(&format!(
        "clear\nprintf '\\033[1m🐘 LocalTusk — %s\\033[0m\\n' {}\n\
         echo {}\necho\n",
        sh_quote(&p.name),
        sh_quote(&terminal_banner(p))
    ));
    script.push_str(&format!(
        "case \"$(basename \"$SHELL\")\" in\n\
         \tzsh) export ZDOTDIR={zdot}; exec \"$SHELL\" -i ;;\n\
         \tbash) exec \"$SHELL\" --rcfile {bashrc} -i ;;\n\
         \t*) export PATH=\"$LOCALTUSK_SHIMS:$PATH\"; exec \"$SHELL\" ;;\n\
         esac\n",
        zdot = sh_quote(&shell_dir.display().to_string()),
        bashrc = sh_quote(&shell_dir.join("bashrc").display().to_string()),
    ));
    let script_path = conf.join("terminal.command");
    write_exec(&script_path, &script)?;
    Ok(script_path)
}

/// Linha de boas-vindas do terminal: quais comandos já apontam para os serviços do projeto.
fn terminal_banner(p: &Project) -> String {
    let mut cmds = if project::is_postgres(p) { "php, psql e pg_dump" } else { "php, wp (WP-CLI) e mysql" }.to_string();
    if project::redis_on(p) {
        cmds.push_str(" e redis-cli");
    }
    let what = if project::is_postgres(p) { "o PHP e o PostgreSQL deste projeto" } else { "o PHP e o banco deste projeto" };
    format!("{cmds} usam {what}.")
}

fn write_exec(path: &Path, content: &str) -> Result<(), String> {
    fs::write(path, content).map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminais_do_linux_recebem_o_script_e_respeitam_o_terminal_da_pessoa() {
        let list = linux_terminals(Path::new("/x/terminal.command"), Some("meu-term".into()));
        assert_eq!(list[0].0, "meu-term");
        assert_eq!(list[0].1, vec!["-e", "/x/terminal.command"]);
        // Todos terminam com o caminho do script, e o gnome-terminal usa `--`.
        assert!(list.iter().all(|(_, a)| a.last().map(String::as_str) == Some("/x/terminal.command")));
        let gnome = list.iter().find(|(n, _)| n == "gnome-terminal").unwrap();
        assert_eq!(gnome.1, vec!["--", "/x/terminal.command"]);
        // Sem preferência, o padrão do Debian vem primeiro.
        assert_eq!(linux_terminals(Path::new("/x"), None)[0].0, "x-terminal-emulator");
        assert_eq!(linux_terminals(Path::new("/x"), Some("  ".into()))[0].0, "x-terminal-emulator");
    }

    #[test]
    fn wordpress_recusa_projeto_postgres() {
        let mut p = project("pg-wp");
        p.db_engine = "postgres".into();
        let o = WpOptions {
            title: "Site".into(),
            admin_user: "admin".into(),
            admin_password: "senha-forte-123".into(),
            admin_email: "a@b.co".into(),
            locale: "en_US".into(),
            replace_existing: false,
        };
        let err = install_wordpress(Path::new("/nao/existe"), &p, &o, &|_| {}).unwrap_err();
        assert!(err.contains("MySQL"), "{err}");
    }

    fn project(id: &str) -> Project {
        Project {
            id: id.into(),
            name: id.into(),
            docroot: "public".into(),
            php_version: "".into(),
            db_version: "".into(),
            http_port: 18999,
            db_port: 13999,
            db_name: id.into(),
            created_at: 0,
            ..Default::default()
        }
    }

    #[test]
    fn docroot_so_e_apagada_com_confirmacao() {
        let base = std::env::temp_dir().join(format!("localtusk-test-docroot-{}", project::now()));
        let p = project("wp-docroot");
        runtime::scaffold(&base, &p).unwrap();
        let docroot = runtime::docroot_dir(&base, &p);

        // Só o index.php de exemplo: livre, sem pedir confirmação.
        assert!(!docroot_in_use(&base, &p));
        prepare_docroot(&base, &p, false).unwrap();

        // Arquivos do usuário: recusa sem confirmação e não apaga nada.
        fs::write(docroot.join("meu.php"), "<?php // importante").unwrap();
        fs::create_dir_all(docroot.join("pasta/sub")).unwrap();
        fs::write(docroot.join("pasta/sub/a.txt"), "x").unwrap();
        assert!(docroot_in_use(&base, &p));
        assert!(prepare_docroot(&base, &p, false).is_err());
        assert!(docroot.join("meu.php").exists());

        // Com confirmação: apaga tudo, inclusive subpastas, mas a pasta pública continua existindo.
        prepare_docroot(&base, &p, true).unwrap();
        assert!(docroot.is_dir());
        assert!(!docroot.join("meu.php").exists());
        assert!(!docroot.join("pasta").exists());
        assert!(!docroot_in_use(&base, &p));

        // Um WordPress existente nunca é apagado.
        fs::create_dir_all(docroot.join("wp-includes")).unwrap();
        fs::write(docroot.join("wp-includes/version.php"), "<?php").unwrap();
        fs::write(docroot.join("outro.txt"), "x").unwrap();
        assert!(prepare_docroot(&base, &p, true).is_err());
        assert!(docroot.join("outro.txt").exists());

        let _ = fs::remove_dir_all(&base);
    }
}
