use crate::apitools::{self, ApiRequest, ApiResponse, RequestsChunk};
use crate::backup::{self, BackupInfo};
use crate::blueprint::{self, BlueprintInfo};
use crate::codemap;
use crate::download::InstallProgress;
use crate::extras::{self, ExtraStatus};
use crate::mysql_manager::{self, MysqlInstall, MysqlRelease};
use crate::php_manager::{self, PhpRelease};
use crate::postgres_manager::{self, PostgresInstall, PostgresRelease};
use crate::valkey_manager::{self, ValkeyInstall, ValkeyRelease};
use crate::project::{self, DB_PASSWORD, DB_USER, Project, resolve_base};
use crate::runtime::{self, Running};
use crate::ssl;
use crate::tools::{self, PhpInstall, ToolStatus};
use crate::wp::{self, WpOptions};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_opener::OpenerExt;

#[derive(Clone)]
pub struct AppState {
    pub base: PathBuf,
    pub projects: Arc<Mutex<Vec<Project>>>,
    pub running: Arc<Mutex<HashMap<String, Running>>>,
    /// Projetos com uma operação (start/stop) em andamento.
    pub busy: Arc<Mutex<HashSet<String>>>,
    /// Versões do PHP sendo baixadas agora.
    pub installing: Arc<Mutex<HashSet<String>>>,
}

impl AppState {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let base = resolve_base(&home);
        let projects = project::load(&base);
        AppState {
            base,
            projects: Arc::new(Mutex::new(projects)),
            running: Arc::new(Mutex::new(HashMap::new())),
            busy: Arc::new(Mutex::new(HashSet::new())),
            installing: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub fn stop_all(&self) {
        let mut running = self.running.lock().unwrap();
        for (_, r) in running.iter_mut() {
            runtime::stop(r);
        }
        running.clear();
    }

    fn view(&self, p: &Project) -> ProjectView {
        let mut running = self.running.lock().unwrap();
        let alive = match running.get_mut(&p.id) {
            Some(r) => r.is_alive(),
            None => false,
        };
        let info = running.get(&p.id).map(|r| r.info.clone()).unwrap_or_default();
        if !alive {
            if let Some(mut r) = running.remove(&p.id) {
                // Algum serviço caiu: derruba o resto para não deixar órfãos.
                runtime::stop(&mut r);
            }
        }
        let busy = self.busy.lock().unwrap().contains(&p.id);
        ProjectView {
            id: p.id.clone(),
            name: p.name.clone(),
            docroot: p.docroot.clone(),
            php_version: p.php_version.clone(),
            db_engine: project::engine_of(p).into(),
            db_version: p.db_version.clone(),
            redis: project::redis_on(p),
            redis_port: p.redis_port,
            db_initialized: runtime::db_initialized(&self.base, &p.id),
            http_port: p.http_port,
            db_port: p.db_port,
            db_name: p.db_name.clone(),
            db_user: DB_USER.into(),
            db_password: DB_PASSWORD.into(),
            url: project::site_url(p),
            http_url: project::http_url(p),
            https: project::https_on(p),
            https_port: p.https_port,
            lan: p.lan,
            lan_urls: project::lan_urls(p),
            path: project::project_dir(&self.base, &p.id).display().to_string(),
            running: alive,
            busy,
            has_adminneo: alive && info.adminneo,
            has_mailpit: alive && info.mail_ui_port.is_some(),
            has_wordpress: wp::is_wordpress(&self.base, p),
            docroot_in_use: wp::docroot_in_use(&self.base, p),
            auto_login: p.auto_login,
            auto_login_user: p.auto_login_user.clone(),
        }
    }

    fn find(&self, id: &str) -> Result<Project, String> {
        self.projects
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .ok_or_else(|| "projeto não encontrado".to_string())
    }

    fn begin(&self, id: &str) -> Result<BusyGuard, String> {
        let mut busy = self.busy.lock().unwrap();
        if !busy.insert(id.to_string()) {
            return Err("operação em andamento neste projeto".into());
        }
        Ok(BusyGuard {
            busy: self.busy.clone(),
            id: id.to_string(),
        })
    }
}

struct BusyGuard {
    busy: Arc<Mutex<HashSet<String>>>,
    id: String,
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.busy.lock().unwrap().remove(&self.id);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectView {
    id: String,
    name: String,
    docroot: String,
    php_version: String,
    /// `mysql` ou `postgres`.
    db_engine: String,
    db_version: String,
    /// O Redis (Valkey) do projeto está ligado.
    redis: bool,
    redis_port: u16,
    /// O banco já foi criado: o tipo e a versão do banco não podem mais ser trocados.
    db_initialized: bool,
    http_port: u16,
    db_port: u16,
    db_name: String,
    db_user: String,
    db_password: String,
    /// Endereço principal (HTTPS quando ligado).
    url: String,
    http_url: String,
    https: bool,
    https_port: u16,
    /// Aceita conexões de outros aparelhos da rede.
    lan: bool,
    /// Endereços para abrir de outros aparelhos (vazio se desligado ou sem rede).
    lan_urls: Vec<String>,
    path: String,
    running: bool,
    busy: bool,
    /// Ferramentas disponíveis nesta execução (dependem de estarem instaladas ao iniciar).
    has_adminneo: bool,
    has_mailpit: bool,
    /// Há um WordPress instalado na pasta pública.
    has_wordpress: bool,
    /// A pasta pública tem arquivos além do `index.php` de exemplo.
    docroot_in_use: bool,
    /// Login automático no wp-admin ligado, e o administrador escolhido.
    auto_login: bool,
    auto_login_user: String,
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn list_projects(state: State<'_, AppState>) -> Vec<ProjectView> {
    let projects = state.projects.lock().unwrap().clone();
    projects.iter().map(|p| state.view(p)).collect()
}

#[tauri::command]
pub fn check_tools(state: State<'_, AppState>) -> Vec<ToolStatus> {
    tools::status(&state.base)
}

#[tauri::command]
pub fn list_php_versions(state: State<'_, AppState>) -> Vec<PhpInstall> {
    tools::php_installs(&state.base)
}

#[tauri::command]
pub fn create_project(
    state: State<'_, AppState>,
    name: String,
    php_version: String,
    db_version: String,
    docroot: String,
    db_engine: Option<String>,
    redis: Option<bool>,
) -> Result<ProjectView, String> {
    let db_engine = db_engine.unwrap_or_else(|| project::ENGINE_MYSQL.into());
    if !project::valid_engine(&db_engine) {
        return Err("tipo de banco inválido".into());
    }
    let redis = redis.unwrap_or(false);
    if redis && valkey_manager::pick(&state.base).is_none() {
        return Err("Instale o Redis (Valkey) em \"PHP e ferramentas\" antes de ligá-lo em um projeto.".into());
    }
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("informe um nome para o projeto".into());
    }
    let docroot = docroot.trim().trim_matches('/').to_string();
    if docroot.contains("..") {
        return Err("pasta pública inválida".into());
    }

    let mut projects = state.projects.lock().unwrap();
    let id = project::unique_id(&name, &projects);
    let reserved = project::reserved_ports(&projects);
    let http_port = project::free_port(8000, &reserved).ok_or("sem portas HTTP livres")?;
    let mut reserved = reserved;
    reserved.insert(http_port);
    let db_port = project::free_port(project::db_port_start(&db_engine), &reserved).ok_or("sem portas de banco livres")?;
    reserved.insert(db_port);
    let redis_port = if redis {
        project::free_port(project::REDIS_PORT_START, &reserved).ok_or("sem portas de Redis livres")?
    } else {
        0
    };

    let p = Project {
        db_name: project::db_name_for(&id),
        id,
        name,
        docroot,
        php_version,
        db_engine,
        db_version,
        http_port,
        db_port,
        redis,
        redis_port,
        created_at: project::now(),
        ..Default::default()
    };
    runtime::scaffold(&state.base, &p)?;
    projects.push(p.clone());
    project::save(&state.base, &projects)?;
    drop(projects);
    Ok(state.view(&p))
}

/// Inicia o projeto (bloqueante). Usado pelo botão "Iniciar" e pela instalação do WordPress.
fn start_blocking(state: &AppState, id: &str) -> Result<ProjectView, String> {
    let guard = state.begin(id)?;
    if state.running.lock().unwrap().contains_key(id) {
        return Err("projeto já está rodando".into());
    }
    let mut p = state.find(id)?;

    // Se alguma porta fixa foi tomada por outro programa, escolhe outra.
    let mut changed = false;
    let others: HashSet<u16> = state
        .projects
        .lock()
        .unwrap()
        .iter()
        .filter(|o| o.id != p.id)
        .flat_map(|o| [o.http_port, o.db_port, o.https_port, o.redis_port])
        .collect();
    if !project::port_is_free(p.http_port) {
        let mut r = others.clone();
        r.extend([p.db_port, p.https_port, p.redis_port]);
        p.http_port = project::free_port(8000, &r).ok_or("sem portas HTTP livres")?;
        changed = true;
    }
    if !project::port_is_free(p.db_port) {
        let mut r = others.clone();
        r.extend([p.http_port, p.https_port, p.redis_port]);
        p.db_port = project::free_port(project::db_port_start(project::engine_of(&p)), &r).ok_or("sem portas de banco livres")?;
        changed = true;
    }
    if project::https_on(&p) && !project::port_is_free(p.https_port) {
        let mut r = others.clone();
        r.extend([p.http_port, p.db_port, p.redis_port]);
        p.https_port = project::free_port(8443, &r).ok_or("sem portas HTTPS livres")?;
        changed = true;
    }
    if project::redis_on(&p) && !project::port_is_free(p.redis_port) {
        let mut r = others;
        r.extend([p.http_port, p.db_port, p.https_port]);
        p.redis_port = project::free_port(project::REDIS_PORT_START, &r).ok_or("sem portas de Redis livres")?;
        changed = true;
    }
    if changed {
        let mut projects = state.projects.lock().unwrap();
        if let Some(slot) = projects.iter_mut().find(|x| x.id == p.id) {
            *slot = p.clone();
        }
        project::save(&state.base, &projects)?;
    }

    let running = runtime::start(&state.base, &p)?;
    state.running.lock().unwrap().insert(id.to_string(), running);
    // Mudou entre HTTP e HTTPS com um WordPress instalado: acerta o endereço gravado no banco.
    // Se falhar, o site continua de pé e a tentativa se repete na próxima vez que iniciar.
    if let Err(e) = wp::sync_site_url(&state.base, &p, &|_| {}) {
        eprintln!("wp: não foi possível atualizar o endereço do site: {e}");
    }
    drop(guard);
    Ok(state.view(&p))
}

/// Altera uma opção de acesso do projeto (HTTPS, rede) e, se ele estiver rodando,
/// reinicia para o Caddy ler a configuração nova.
fn change_access(
    state: &AppState,
    id: &str,
    change: impl FnOnce(&mut Project, &HashSet<u16>) -> Result<(), String>,
) -> Result<ProjectView, String> {
    let was_running = state.running.lock().unwrap().contains_key(id);
    {
        let guard = state.begin(id)?;
        let mut projects = state.projects.lock().unwrap();
        let reserved = project::reserved_ports(&projects);
        let slot = projects.iter_mut().find(|p| p.id == id).ok_or("projeto não encontrado")?;
        // Guarda o endereço atual para poder trocá-lo no WordPress depois.
        wp::remember_site_url(&state.base, slot);
        change(slot, &reserved)?;
        project::save(&state.base, &projects)?;
        drop(projects);
        if was_running {
            let taken = state.running.lock().unwrap().remove(id);
            if let Some(mut r) = taken {
                runtime::stop(&mut r);
            }
        }
        drop(guard);
    }
    if was_running {
        start_blocking(state, id)
    } else {
        let p = state.find(id)?;
        Ok(state.view(&p))
    }
}

/// Liga ou desliga o HTTPS do projeto. Ao ligar, reserva uma porta HTTPS.
#[tauri::command]
pub async fn set_https(state: State<'_, AppState>, id: String, enabled: bool) -> Result<ProjectView, String> {
    let state = state.inner().clone();
    blocking(move || {
        change_access(&state, &id, |p, reserved| {
            if enabled && p.https_port == 0 {
                p.https_port = project::free_port(8443, reserved).ok_or("sem portas HTTPS livres")?;
            }
            p.https = enabled;
            Ok(())
        })
    })
    .await
}

/// Liga ou desliga o acesso ao projeto por outros aparelhos da rede local.
#[tauri::command]
pub async fn set_lan(state: State<'_, AppState>, id: String, enabled: bool) -> Result<ProjectView, String> {
    let state = state.inner().clone();
    blocking(move || {
        change_access(&state, &id, |p, _| {
            p.lan = enabled;
            Ok(())
        })
    })
    .await
}

#[tauri::command]
pub async fn ssl_status(state: State<'_, AppState>) -> Result<ssl::SslStatus, String> {
    let base = state.base.clone();
    blocking(move || Ok(ssl::status(&base))).await
}

/// Faz o sistema confiar na CA do HTTPS local (pede a senha de administrador no macOS).
#[tauri::command]
pub async fn trust_certificate(state: State<'_, AppState>) -> Result<ssl::SslStatus, String> {
    let base = state.base.clone();
    blocking(move || {
        let caddy = extras::caddy_bin(&base)
            .or_else(|| tools::find("caddy", &base))
            .ok_or("Caddy não encontrado. Instale em \"PHP e ferramentas\".")?;
        ssl::trust(&base, &caddy)?;
        Ok(ssl::status(&base))
    })
    .await
}

/// Mostra o certificado raiz no Finder (para instalar em outros aparelhos).
#[tauri::command]
pub fn reveal_ca(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let root = ssl::root_cert(&state.base);
    if !root.is_file() {
        return Err("O certificado ainda não foi criado. Ative o HTTPS primeiro.".into());
    }
    app.opener().reveal_item_in_dir(root).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn start_project(state: State<'_, AppState>, id: String) -> Result<ProjectView, String> {
    let state = state.inner().clone();
    blocking(move || start_blocking(&state, &id)).await
}

#[tauri::command]
pub async fn stop_project(state: State<'_, AppState>, id: String) -> Result<ProjectView, String> {
    let state = state.inner().clone();
    blocking(move || {
        let _guard = state.begin(&id)?;
        let p = state.find(&id)?;
        let taken = state.running.lock().unwrap().remove(&id);
        if let Some(mut r) = taken {
            runtime::stop(&mut r);
        }
        drop(_guard);
        Ok(state.view(&p))
    })
    .await
}

#[tauri::command]
pub async fn delete_project(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let state = state.inner().clone();
    blocking(move || {
        let _guard = state.begin(&id)?;
        state.find(&id)?;
        if let Some(mut r) = state.running.lock().unwrap().remove(&id) {
            runtime::stop(&mut r);
        }
        // Serviços órfãos de uma sessão anterior também segurariam arquivos da pasta.
        runtime::reap_orphans(&state.base, &id);
        let dir = project::project_dir(&state.base, &id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        }
        let mut projects = state.projects.lock().unwrap();
        projects.retain(|p| p.id != id);
        project::save(&state.base, &projects)
    })
    .await
}

#[tauri::command]
pub fn set_php_version(
    state: State<'_, AppState>,
    id: String,
    php_version: String,
) -> Result<ProjectView, String> {
    if state.running.lock().unwrap().contains_key(&id) {
        return Err("pare o projeto antes de trocar a versão do PHP".into());
    }
    let mut projects = state.projects.lock().unwrap();
    let p = projects
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or("projeto não encontrado")?;
    p.php_version = php_version;
    let p = p.clone();
    project::save(&state.base, &projects)?;
    drop(projects);
    Ok(state.view(&p))
}

#[tauri::command]
pub fn open_site(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    path: Option<String>,
) -> Result<(), String> {
    let p = state.find(&id)?;
    let path = path.unwrap_or_default();
    if !path.is_empty() && (!path.starts_with('/') || path.contains(char::is_whitespace)) {
        return Err("caminho inválido".into());
    }
    app.opener()
        .open_url(format!("{}{path}", project::site_url(&p)), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Confere se o administrador salvo ainda está na lista de administradores do site.
fn resolve_admin(saved: &str, admins: &[String]) -> Result<String, String> {
    if admins.iter().any(|a| a == saved) {
        Ok(saved.to_string())
    } else {
        Err(format!(
            "O usuário {saved} não é mais administrador deste site. Escolha outro no login automático."
        ))
    }
}

/// Liga/desliga o login automático e escolhe o administrador. Não reinicia o projeto.
#[tauri::command]
pub async fn set_auto_login(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
    user: String,
) -> Result<ProjectView, String> {
    let state = state.inner().clone();
    blocking(move || {
        let p = {
            let mut projects = state.projects.lock().unwrap();
            let p = projects
                .iter_mut()
                .find(|p| p.id == id)
                .ok_or("projeto não encontrado")?;
            p.auto_login = enabled;
            p.auto_login_user = user;
            let p = p.clone();
            project::save(&state.base, &projects)?;
            p
        };
        let running = state.running.lock().unwrap().contains_key(&id);
        if wp::is_wordpress(&state.base, &p) {
            if enabled {
                if running {
                    wp::install_login_plugin(&state.base, &p)?;
                }
            } else {
                // Falha ao remover não impede de salvar.
                let _ = wp::remove_login_plugin(&state.base, &p);
            }
        }
        Ok(state.view(&p))
    })
    .await
}

/// Logins dos administradores do WordPress do projeto (precisa estar rodando).
#[tauri::command]
pub async fn list_wp_admins(state: State<'_, AppState>, id: String) -> Result<Vec<String>, String> {
    let state = state.inner().clone();
    blocking(move || {
        let p = state.find(&id)?;
        if !state.running.lock().unwrap().contains_key(&id) {
            return Err("Inicie o projeto para listar os usuários.".into());
        }
        wp::admin_users(&state.base, &p)
    })
    .await
}

/// Abre o wp-admin. Com login automático, entra direto; se algo falhar, abre a tela de
/// login normal e devolve o aviso para o app mostrar.
#[tauri::command]
pub async fn open_wp_admin(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<String>, String> {
    let state = state.inner().clone();
    let p = state.find(&id)?;
    let admin_url = format!("{}/wp-admin/", project::site_url(&p));
    let (url, warning) = blocking(move || {
        if !p.auto_login || !wp::is_wordpress(&state.base, &p) {
            return Ok((format!("{}/wp-admin/", project::site_url(&p)), None));
        }
        let attempt = (|| {
            let admins = wp::admin_users(&state.base, &p)?;
            let user = resolve_admin(&p.auto_login_user, &admins)?;
            wp::install_login_plugin(&state.base, &p)?;
            let token = wp::create_login_token(&state.base, &p, &user)?;
            Ok::<String, String>(format!("{}/wp-login.php?{}={token}", project::site_url(&p), wp::LOGIN_PARAM))
        })();
        Ok(match attempt {
            Ok(url) => (url, None),
            Err(msg) => (format!("{}/wp-admin/", project::site_url(&p)), Some(msg)),
        })
    })
    .await
    .unwrap_or_else(|e| (admin_url, Some(e)));
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())?;
    Ok(warning)
}

#[tauri::command]
pub fn open_folder(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.find(&id)?;
    let dir = runtime::app_dir(&state.base, &id);
    app.opener()
        .open_path(dir.display().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn read_log(state: State<'_, AppState>, id: String, file: String) -> Result<String, String> {
    runtime::read_log(&state.base, &id, &file, 300)
}

/// Mapa do projeto: quem chama quem e quais arquivos ninguém usa.
#[tauri::command]
pub async fn analyze_project(state: State<'_, AppState>, id: String) -> Result<codemap::CodeMap, String> {
    let p = state.find(&id)?;
    let root = runtime::app_dir(&state.base, &id);
    blocking(move || Ok(codemap::analyze(&root, &p.docroot))).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    data_dir: String,
    php_dir: String,
    backups_dir: String,
    blueprints_dir: String,
    /// Versões em download neste momento.
    installing: Vec<String>,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        data_dir: state.base.display().to_string(),
        php_dir: php_manager::php_root(&state.base).display().to_string(),
        backups_dir: backup::backups_dir(&state.base).display().to_string(),
        blueprints_dir: blueprint::blueprints_dir(&state.base).display().to_string(),
        installing: state.installing.lock().unwrap().iter().cloned().collect(),
    }
}

#[tauri::command]
pub fn open_data_dir(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    std::fs::create_dir_all(&state.base).map_err(|e| e.to_string())?;
    app.opener()
        .open_path(state.base.display().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Catálogo de versões do PHP disponíveis para download (precisa de internet).
#[tauri::command]
pub async fn php_catalog(state: State<'_, AppState>) -> Result<Vec<PhpRelease>, String> {
    let base = state.base.clone();
    blocking(move || php_manager::catalog(&base)).await
}

/// Executa uma instalação em segundo plano, emitindo `install-progress` e impedindo
/// instalações duplicadas do mesmo item (`key`).
async fn tracked_install(
    app: AppHandle,
    state: AppState,
    key: String,
    job: impl FnOnce(PathBuf, &dyn Fn(InstallProgress)) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    if !state.installing.lock().unwrap().insert(key.clone()) {
        return Err("já está sendo instalado".into());
    }
    let (base, handle) = (state.base.clone(), app.clone());
    let result = blocking(move || {
        job(base, &|p: InstallProgress| {
            let _ = handle.emit("install-progress", p);
        })
    })
    .await;
    state.installing.lock().unwrap().remove(&key);

    let (stage, message) = match &result {
        Ok(()) => ("done", None),
        Err(e) => ("error", Some(e.clone())),
    };
    let _ = app.emit(
        "install-progress",
        InstallProgress {
            id: key,
            stage: stage.into(),
            downloaded: 0,
            total: 0,
            message,
        },
    );
    result
}

#[tauri::command]
pub async fn install_php(
    app: AppHandle,
    state: State<'_, AppState>,
    version: String,
) -> Result<(), String> {
    if !php_manager::valid_version(&version) {
        return Err("versão inválida".into());
    }
    let v = version.clone();
    tracked_install(app, state.inner().clone(), version, move |base, progress| {
        php_manager::install(&base, &v, progress)
    })
    .await
}

#[tauri::command]
pub async fn uninstall_php(state: State<'_, AppState>, version: String) -> Result<(), String> {
    let state = state.inner().clone();
    blocking(move || {
        if !php_manager::managed_versions(&state.base).contains(&version) {
            return Err("esta versão não foi instalada pelo LocalTusk".into());
        }
        // Não remove um PHP que está em uso por um projeto rodando.
        let projects = state.projects.lock().unwrap().clone();
        let running = state.running.lock().unwrap();
        for p in projects.iter().filter(|p| running.contains_key(&p.id)) {
            let in_use = tools::pick_php(&state.base, &p.php_version)
                .map(|i| i.full_version == version)
                .unwrap_or(false);
            if in_use {
                return Err(format!("PHP {version} está em uso pelo projeto \"{}\". Pare-o primeiro.", p.name));
            }
        }
        drop(running);
        php_manager::uninstall(&state.base, &version)
    })
    .await
}

#[tauri::command]
pub fn extras_status(state: State<'_, AppState>) -> Vec<ExtraStatus> {
    extras::status(&state.base)
}

#[tauri::command]
pub async fn install_extra(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    if !extras::IDS.contains(&id.as_str()) {
        return Err("ferramenta desconhecida".into());
    }
    let i = id.clone();
    tracked_install(app, state.inner().clone(), id, move |base, progress| {
        extras::install(&base, &i, progress)
    })
    .await
}

#[tauri::command]
pub async fn uninstall_extra(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let base = state.base.clone();
    blocking(move || extras::uninstall(&base, &id)).await
}

/// Abre uma ferramenta do projeto em execução: `adminneo` ou `mailpit`.
#[tauri::command]
pub fn open_tool(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    tool: String,
) -> Result<(), String> {
    let info = state
        .running
        .lock()
        .unwrap()
        .get(&id)
        .map(|r| r.info.clone())
        .ok_or("inicie o projeto primeiro")?;
    let url = match tool.as_str() {
        "adminneo" if info.adminneo => {
            format!("http://127.0.0.1:{}/adminneo/login.php", info.admin_port.ok_or("indisponível")?)
        }
        "mailpit" if info.mail_ui_port.is_some() => {
            format!("http://127.0.0.1:{}/", info.mail_ui_port.unwrap())
        }
        _ => {
            return Err("ferramenta não disponível neste projeto. Instale-a em \"PHP e ferramentas\" e reinicie o projeto.".into());
        }
    };
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

// ------------------------------------------------------------------ MySQL

#[tauri::command]
pub fn list_mysql_versions(state: State<'_, AppState>) -> Vec<MysqlInstall> {
    mysql_manager::installs(&state.base)
}

/// Catálogo de versões do MySQL (a primeira consulta sonda o servidor e leva alguns segundos).
#[tauri::command]
pub async fn mysql_catalog(state: State<'_, AppState>) -> Result<Vec<MysqlRelease>, String> {
    let base = state.base.clone();
    blocking(move || mysql_manager::catalog(&base)).await
}

#[tauri::command]
pub async fn install_mysql(
    app: AppHandle,
    state: State<'_, AppState>,
    version: String,
) -> Result<(), String> {
    if !mysql_manager::valid_version(&version) {
        return Err("versão inválida".into());
    }
    let v = version.clone();
    // O prefixo evita colidir com ids de versões do PHP (8.4.9).
    tracked_install(app, state.inner().clone(), format!("mysql-{version}"), move |base, progress| {
        mysql_manager::install(&base, &v, progress)
    })
    .await
}

#[tauri::command]
pub async fn uninstall_mysql(state: State<'_, AppState>, version: String) -> Result<(), String> {
    let state = state.inner().clone();
    blocking(move || {
        if !mysql_manager::managed_versions(&state.base).contains(&version) {
            return Err("esta versão não foi instalada pelo LocalTusk".into());
        }
        let projects = state.projects.lock().unwrap().clone();
        let running = state.running.lock().unwrap();
        for p in projects.iter().filter(|p| running.contains_key(&p.id) && !project::is_postgres(p)) {
            let in_use = mysql_manager::pick(&state.base, &p.db_version)
                .map(|i| i.full_version == version)
                .unwrap_or(false);
            if in_use {
                return Err(format!(
                    "MySQL {version} está em uso pelo projeto \"{}\". Pare-o primeiro.",
                    p.name
                ));
            }
        }
        drop(running);
        mysql_manager::uninstall(&state.base, &version)
    })
    .await
}

/// Troca o banco de um projeto (tipo e versão). Só é possível antes de o banco ser criado:
/// um diretório de dados não pode mudar de tipo nem voltar para uma versão anterior.
/// Ao trocar o tipo, o projeto ganha uma porta de banco própria do novo tipo.
#[tauri::command]
pub fn set_database(
    state: State<'_, AppState>,
    id: String,
    db_engine: String,
    db_version: String,
) -> Result<ProjectView, String> {
    if !project::valid_engine(&db_engine) {
        return Err("tipo de banco inválido".into());
    }
    if state.running.lock().unwrap().contains_key(&id) {
        return Err("pare o projeto antes de trocar o banco".into());
    }
    if runtime::db_initialized(&state.base, &id) {
        return Err(
            "o banco deste projeto já foi criado e não pode trocar de tipo nem de versão. Crie um novo projeto para usar outro banco."
                .into(),
        );
    }
    let mut projects = state.projects.lock().unwrap();
    let reserved = project::reserved_ports(&projects);
    let p = projects
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or("projeto não encontrado")?;
    if project::engine_of(p) != db_engine {
        let mut taken = reserved;
        taken.remove(&p.db_port);
        p.db_port = project::free_port(project::db_port_start(&db_engine), &taken).ok_or("sem portas de banco livres")?;
    }
    p.db_engine = db_engine;
    p.db_version = db_version;
    let p = p.clone();
    project::save(&state.base, &projects)?;
    drop(projects);
    Ok(state.view(&p))
}

/// Liga ou desliga o Redis (Valkey) do projeto. Ao ligar, reserva uma porta.
#[tauri::command]
pub async fn set_redis(state: State<'_, AppState>, id: String, enabled: bool) -> Result<ProjectView, String> {
    let state = state.inner().clone();
    blocking(move || {
        if enabled && valkey_manager::pick(&state.base).is_none() {
            return Err("Instale o Redis (Valkey) em \"PHP e ferramentas\" antes de ligá-lo.".into());
        }
        change_access(&state, &id, |p, reserved| {
            if enabled && p.redis_port == 0 {
                p.redis_port = project::free_port(project::REDIS_PORT_START, reserved).ok_or("sem portas de Redis livres")?;
            }
            p.redis = enabled;
            Ok(())
        })
    })
    .await
}

// ------------------------------------------------------------------ PostgreSQL

#[tauri::command]
pub fn list_postgres_versions(state: State<'_, AppState>) -> Vec<PostgresInstall> {
    postgres_manager::installs(&state.base)
}

/// Catálogo embutido de versões do PostgreSQL (instantâneo, não usa a internet).
#[tauri::command]
pub async fn postgres_catalog(state: State<'_, AppState>) -> Result<Vec<PostgresRelease>, String> {
    let base = state.base.clone();
    blocking(move || postgres_manager::catalog(&base)).await
}

#[tauri::command]
pub async fn install_postgres(app: AppHandle, state: State<'_, AppState>, version: String) -> Result<(), String> {
    if !postgres_manager::valid_version(&version) {
        return Err("versão inválida".into());
    }
    let v = version.clone();
    tracked_install(app, state.inner().clone(), format!("postgres-{version}"), move |base, progress| {
        postgres_manager::install(&base, &v, progress)
    })
    .await
}

#[tauri::command]
pub async fn uninstall_postgres(state: State<'_, AppState>, version: String) -> Result<(), String> {
    let state = state.inner().clone();
    blocking(move || {
        if !postgres_manager::managed_versions(&state.base).contains(&version) {
            return Err("esta versão não foi instalada pelo LocalTusk".into());
        }
        let projects = state.projects.lock().unwrap().clone();
        let running = state.running.lock().unwrap();
        for p in projects.iter().filter(|p| running.contains_key(&p.id) && project::is_postgres(p)) {
            let in_use = postgres_manager::pick(&state.base, &p.db_version)
                .map(|i| i.full_version == version)
                .unwrap_or(false);
            if in_use {
                return Err(format!("PostgreSQL {version} está em uso pelo projeto \"{}\". Pare-o primeiro.", p.name));
            }
        }
        drop(running);
        postgres_manager::uninstall(&state.base, &version)
    })
    .await
}

// ------------------------------------------------------------------ Redis (Valkey)

#[tauri::command]
pub fn list_valkey_versions(state: State<'_, AppState>) -> Vec<ValkeyInstall> {
    valkey_manager::installs(&state.base)
}

#[tauri::command]
pub async fn valkey_catalog(state: State<'_, AppState>) -> Result<Vec<ValkeyRelease>, String> {
    let base = state.base.clone();
    blocking(move || valkey_manager::catalog(&base)).await
}

/// Baixa o código-fonte do Valkey e compila (leva cerca de 15 segundos).
#[tauri::command]
pub async fn install_valkey(app: AppHandle, state: State<'_, AppState>, version: String) -> Result<(), String> {
    if !valkey_manager::valid_version(&version) {
        return Err("versão inválida".into());
    }
    let v = version.clone();
    tracked_install(app, state.inner().clone(), format!("valkey-{version}"), move |base, progress| {
        valkey_manager::install(&base, &v, progress)
    })
    .await
}

#[tauri::command]
pub async fn uninstall_valkey(state: State<'_, AppState>, version: String) -> Result<(), String> {
    let state = state.inner().clone();
    blocking(move || {
        if !valkey_manager::managed_versions(&state.base).contains(&version) {
            return Err("esta versão não foi instalada pelo LocalTusk".into());
        }
        let projects = state.projects.lock().unwrap().clone();
        let running = state.running.lock().unwrap();
        if let Some(p) = projects.iter().find(|p| running.contains_key(&p.id) && project::redis_on(p)) {
            return Err(format!("O Redis está em uso pelo projeto \"{}\". Pare-o primeiro.", p.name));
        }
        drop(running);
        valkey_manager::uninstall(&state.base, &version)
    })
    .await
}

// ------------------------------------------------------------------ Terminal e WordPress

/// Abre um terminal na pasta do projeto com `php`, `wp` e `mysql` configurados para ele.
#[tauri::command]
pub async fn open_terminal(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let p = state.find(&id)?;
    let base = state.base.clone();
    blocking(move || wp::open_terminal(&base, &p)).await
}

/// Instala o WordPress na pasta pública do projeto. Se o projeto estiver parado, ele é
/// iniciado antes (o banco precisa estar no ar).
#[tauri::command]
pub async fn install_wordpress(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    options: WpOptions,
) -> Result<(), String> {
    state.find(&id)?;
    let st = state.inner().clone();
    let key = format!("wordpress-{id}");
    tracked_install(app, st.clone(), key.clone(), move |base, progress| {
        if !st.running.lock().unwrap().contains_key(&id) {
            progress(InstallProgress {
                id: key,
                stage: "extract".into(),
                downloaded: 0,
                total: 0,
                message: Some("Iniciando o projeto…".into()),
            });
            start_blocking(&st, &id)?;
        }
        // Relê o projeto: a inicialização pode ter trocado as portas.
        let p = st.find(&id)?;
        wp::install_wordpress(&base, &p, &options, progress)
    })
    .await
}

// ------------------------------------------------------------------ Backup e restauração

/// Para o projeto (bloqueante). Não faz nada se ele já estiver parado.
fn stop_blocking(state: &AppState, id: &str) -> Result<(), String> {
    let _guard = state.begin(id)?;
    let taken = state.running.lock().unwrap().remove(id);
    if let Some(mut r) = taken {
        runtime::stop(&mut r);
    }
    Ok(())
}

/// Impede duas operações longas (backup, blueprint, restauração, WordPress) no mesmo projeto.
fn ensure_idle(state: &AppState, id: &str) -> Result<(), String> {
    let installing = state.installing.lock().unwrap();
    if ["backup", "blueprint", "restore", "wordpress"].iter().any(|k| installing.contains(&format!("{k}-{id}"))) {
        return Err("já há uma operação em andamento neste projeto".into());
    }
    Ok(())
}

/// A pasta pública vem do arquivo de backup/blueprint, então é validada como qualquer entrada externa.
fn clean_docroot(raw: &str) -> Result<String, String> {
    let d = raw.trim().trim_matches('/');
    if d.contains("..") || d.contains('\\') || d.starts_with('~') {
        return Err("O arquivo tem uma pasta pública inválida.".into());
    }
    Ok(d.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreResult {
    project: ProjectView,
    /// Algo secundário não deu certo (o backup foi restaurado mesmo assim).
    warning: Option<String>,
}

#[tauri::command]
pub async fn list_backups(state: State<'_, AppState>, id: Option<String>) -> Result<Vec<BackupInfo>, String> {
    let base = state.base.clone();
    blocking(move || Ok(backup::list(&base, id.as_deref()))).await
}

#[tauri::command]
pub fn open_backups_dir(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let dir = backup::backups_dir(&state.base);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    app.opener().open_path(dir.display().to_string(), None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_backup(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let base = state.base.clone();
    blocking(move || backup::delete(&base, std::path::Path::new(&path))).await
}

/// Cria um backup do projeto em `~/LocalTusk/backups`. Se o projeto estiver parado, é ligado
/// só para exportar o banco e desligado em seguida.
#[tauri::command]
pub async fn create_backup(app: AppHandle, state: State<'_, AppState>, id: String) -> Result<BackupInfo, String> {
    let st = state.inner().clone();
    st.find(&id)?;
    ensure_idle(&st, &id)?;
    let out: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let (job_state, job_id, slot) = (st.clone(), id.clone(), out.clone());
    tracked_install(app, st, format!("backup-{id}"), move |base, progress| {
        let key = format!("backup-{job_id}");
        let was_running = job_state.running.lock().unwrap().contains_key(&job_id);
        if !was_running {
            backup::step(progress, &key, "Iniciando o projeto…");
            start_blocking(&job_state, &job_id)?;
        }
        // Relê o projeto: a inicialização pode ter trocado as portas.
        let p = job_state.find(&job_id)?;
        let result = backup::create(&base, &p, "", &key, progress);
        if !was_running {
            let _ = stop_blocking(&job_state, &job_id);
        }
        *slot.lock().unwrap() = Some(result?);
        Ok(())
    })
    .await?;
    let path = out.lock().unwrap().take().ok_or("o backup não foi gerado")?;
    backup::info(&path)
}

/// Nome de banco aceito vindo de um arquivo externo: letras, números e `_`.
fn valid_db_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A versão do banco guardada no backup/blueprint está instalada aqui?
fn db_version_installed(base: &std::path::Path, engine: &str, version: &str) -> bool {
    if engine == project::ENGINE_POSTGRES {
        postgres_manager::installs(base).iter().any(|i| i.full_version == version || i.major == version)
    } else {
        mysql_manager::installs(base).iter().any(|i| i.full_version == version || i.series == version)
    }
}

/// Cria um projeto novo a partir dos dados de um backup ou blueprint (nunca sobrescreve um existente).
/// Com `new_name` (blueprint), o projeto ganha esse nome e um id derivado dele; o nome do banco
/// do original é mantido, para que o código do site continue achando o banco.
fn project_from_manifest(state: &AppState, m: &backup::Manifest, new_name: Option<&str>) -> Result<Project, String> {
    let name = match new_name {
        Some(n) => n.to_string(),
        None if m.name.trim().is_empty() => "Projeto restaurado".to_string(),
        None => m.name.trim().to_string(),
    };
    let docroot = clean_docroot(&m.docroot)?;

    let mut projects = state.projects.lock().unwrap();
    let wanted = project::slugify(&m.id);
    let id = if new_name.is_some() || projects.iter().any(|p| p.id == wanted) {
        project::unique_id(&name, &projects)
    } else {
        wanted
    };

    // Mantém as portas originais quando estão livres (porta 0 = escolher uma livre).
    let mut reserved = project::reserved_ports(&projects);
    let http_port = project::pick_port(m.http_port, 8000, &reserved).ok_or("sem portas HTTP livres")?;
    reserved.insert(http_port);
    let engine = if project::valid_engine(&m.db_engine) { m.db_engine.clone() } else { project::ENGINE_MYSQL.to_string() };
    let db_port = project::pick_port(m.db_port, project::db_port_start(&engine), &reserved)
        .ok_or("sem portas de banco livres")?;

    // Versões que não estão instaladas aqui viram "a mais recente".
    let php_version = if tools::php_installs(&state.base)
        .iter()
        .any(|i| i.full_version == m.php_version || i.version == m.php_version)
    {
        m.php_version.clone()
    } else {
        String::new()
    };
    let db_version = if db_version_installed(&state.base, &engine, &m.db_version) { m.db_version.clone() } else { String::new() };

    let db_name = if new_name.is_some() && valid_db_name(&m.db_name) { m.db_name.clone() } else { project::db_name_for(&id) };
    let p = Project {
        db_name,
        id,
        name,
        docroot,
        php_version,
        db_engine: engine,
        db_version,
        http_port,
        db_port,
        created_at: project::now(),
        ..Default::default()
    };
    runtime::scaffold(&state.base, &p)?;
    projects.push(p.clone());
    project::save(&state.base, &projects)?;
    Ok(p)
}

/// Restaura um backup.
/// - `target = None`: cria um **projeto novo** com o conteúdo do backup.
/// - `target = Some(id)`: **substitui** os arquivos e o banco desse projeto (antes, é feito
///   um backup de segurança dele).
#[tauri::command]
pub async fn restore_backup(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    target: Option<String>,
) -> Result<RestoreResult, String> {
    let archive = PathBuf::from(&path);
    let manifest = {
        let a = archive.clone();
        blocking(move || backup::read_manifest(&a)).await?
    };
    restore_impl(app, state.inner().clone(), Origin::Archive(archive), manifest, target, None).await
}

/// De onde vem o conteúdo restaurado.
enum Origin {
    /// Backup ou blueprint (`.tar.gz`).
    Archive(PathBuf),
    /// Pasta pública + dump SQL (importador do Local). Só para projetos novos.
    Folder { public: PathBuf, dump: PathBuf },
}

/// Núcleo da restauração. `new_name` só vale sem `target` e marca a criação a partir de um blueprint.
async fn restore_impl(
    app: AppHandle,
    st: AppState,
    origin: Origin,
    manifest: backup::Manifest,
    target: Option<String>,
    new_name: Option<String>,
) -> Result<RestoreResult, String> {
    if target.is_some() && matches!(origin, Origin::Folder { .. }) {
        return Err("uma pasta só pode ser restaurada como um projeto novo".into());
    }
    let new_docroot = clean_docroot(&manifest.docroot)?;

    let is_new = target.is_none();
    let id = match &target {
        Some(t) => {
            st.find(t)?;
            ensure_idle(&st, t)?;
            t.clone()
        }
        None => {
            let st2 = st.clone();
            let m = manifest.clone();
            let name = new_name.clone();
            blocking(move || project_from_manifest(&st2, &m, name.as_deref())).await?.id
        }
    };

    let warning: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let (job_state, job_id, slot) = (st.clone(), id.clone(), warning.clone());
    let result = tracked_install(app, st.clone(), format!("restore-{id}"), move |base, progress| {
        let key = format!("restore-{job_id}");
        let was_running = job_state.running.lock().unwrap().contains_key(&job_id);
        if !was_running {
            backup::step(progress, &key, "Iniciando o projeto…");
            start_blocking(&job_state, &job_id)?;
        }

        if !is_new {
            // Rede de segurança: o estado atual fica guardado antes de ser substituído.
            backup::step(progress, &key, "Criando um backup de segurança…");
            let p = job_state.find(&job_id)?;
            backup::create(&base, &p, "antes-do-restore", &key, progress)?;

            // O site restaurado pode usar outra pasta pública (o Caddy precisa reiniciar).
            if p.docroot != new_docroot {
                stop_blocking(&job_state, &job_id)?;
                {
                    let mut projects = job_state.projects.lock().unwrap();
                    if let Some(slot) = projects.iter_mut().find(|x| x.id == job_id) {
                        slot.docroot = new_docroot.clone();
                    }
                    project::save(&base, &projects)?;
                }
                start_blocking(&job_state, &job_id)?;
            }
        }

        let p = job_state.find(&job_id)?;
        let source = match &origin {
            Origin::Archive(archive) => backup::RestoreSource::Archive(archive),
            Origin::Folder { public, dump } => backup::RestoreSource::Folder { public, dump },
        };
        let restored = backup::restore_into(&base, &p, source, &key, progress);
        if !is_new && !was_running {
            let _ = stop_blocking(&job_state, &job_id);
        }
        *slot.lock().unwrap() = restored.map_err(|e| {
            if is_new {
                e
            } else {
                format!("{e}\n\nO estado anterior do projeto está no backup \"antes-do-restore\" em ~/LocalTusk/backups.")
            }
        })?;
        Ok(())
    })
    .await;

    if let Err(e) = result {
        if is_new {
            // Desfaz o projeto criado pela metade.
            let (s, i) = (st.clone(), id.clone());
            let _ = blocking(move || {
                let _ = stop_blocking(&s, &i);
                let mut projects = s.projects.lock().unwrap();
                projects.retain(|p| p.id != i);
                let _ = project::save(&s.base, &projects);
                let _ = std::fs::remove_dir_all(project::project_dir(&s.base, &i));
                Ok(())
            })
            .await;
        }
        return Err(e);
    }

    let p = st.find(&id)?;
    let warning = warning.lock().unwrap().take();
    Ok(RestoreResult { project: st.view(&p), warning })
}

// ------------------------------------------------------------------ Blueprints

#[tauri::command]
pub async fn list_blueprints(state: State<'_, AppState>) -> Result<Vec<BlueprintInfo>, String> {
    let base = state.base.clone();
    blocking(move || Ok(blueprint::list(&base))).await
}

#[tauri::command]
pub fn open_blueprints_dir(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let dir = blueprint::blueprints_dir(&state.base);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    app.opener().open_path(dir.display().to_string(), None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_blueprint(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let base = state.base.clone();
    blocking(move || blueprint::delete(&base, std::path::Path::new(&path))).await
}

/// Traz para a pasta de blueprints um arquivo recebido de outra pessoa.
#[tauri::command]
pub async fn import_blueprint(state: State<'_, AppState>, path: String) -> Result<BlueprintInfo, String> {
    let base = state.base.clone();
    blocking(move || blueprint::import(&base, std::path::Path::new(&path))).await
}

/// Salva uma cópia do blueprint em `dest`, para compartilhar. Devolve o caminho gravado.
#[tauri::command]
pub async fn export_blueprint(state: State<'_, AppState>, path: String, dest: String) -> Result<String, String> {
    let base = state.base.clone();
    blocking(move || {
        blueprint::export(&base, std::path::Path::new(&path), std::path::Path::new(&dest))
            .map(|p| p.display().to_string())
    })
    .await
}

/// Salva o projeto como blueprint (arquivos + banco + versões). Se o projeto estiver parado,
/// é ligado só para exportar o banco e desligado em seguida.
#[tauri::command]
pub async fn create_blueprint(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    name: String,
    description: String,
) -> Result<BlueprintInfo, String> {
    let name = blueprint::clean_name(&name)?;
    let description = blueprint::clean_description(&description);
    let st = state.inner().clone();
    st.find(&id)?;
    ensure_idle(&st, &id)?;
    let out: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let (job_state, job_id, slot) = (st.clone(), id.clone(), out.clone());
    tracked_install(app, st, format!("blueprint-{id}"), move |base, progress| {
        let key = format!("blueprint-{job_id}");
        let was_running = job_state.running.lock().unwrap().contains_key(&job_id);
        if !was_running {
            backup::step(progress, &key, "Iniciando o projeto…");
            start_blocking(&job_state, &job_id)?;
        }
        let p = job_state.find(&job_id)?;
        let result = blueprint::create(&base, &p, &name, &description, &key, progress);
        if !was_running {
            let _ = stop_blocking(&job_state, &job_id);
        }
        *slot.lock().unwrap() = Some(result?);
        Ok(())
    })
    .await?;
    let path = out.lock().unwrap().take().ok_or("o blueprint não foi gerado")?;
    blueprint::info(&path)
}

/// Avisa quando o blueprint foi feito com versões de PHP/banco que não estão instaladas aqui.
fn missing_versions_note(state: &AppState, m: &backup::Manifest) -> Option<String> {
    let mut missing = vec![];
    if !m.php_version.is_empty()
        && !tools::php_installs(&state.base).iter().any(|i| i.full_version == m.php_version || i.version == m.php_version)
    {
        missing.push(format!("PHP {}", m.php_version));
    }
    let engine = if project::valid_engine(&m.db_engine) { m.db_engine.as_str() } else { project::ENGINE_MYSQL };
    if !m.db_version.is_empty() && !db_version_installed(&state.base, engine, &m.db_version) {
        let label = if engine == project::ENGINE_POSTGRES { "PostgreSQL" } else { "MySQL" };
        missing.push(format!("{label} {}", m.db_version));
    }
    if missing.is_empty() {
        return None;
    }
    let verb = if missing.len() == 1 { "não está instalado" } else { "não estão instalados" };
    Some(format!(
        "O blueprint foi feito com {}, que {verb} aqui. O projeto usa as versões mais recentes instaladas.",
        missing.join(" e ")
    ))
}

/// Cria um projeto novo (nome `name`) com os arquivos, o banco e as versões do blueprint.
#[tauri::command]
pub async fn create_project_from_blueprint(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    name: String,
) -> Result<RestoreResult, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("informe um nome para o projeto".into());
    }
    let st = state.inner().clone();
    let manifest = {
        let p = PathBuf::from(&path);
        blocking(move || blueprint::read(&p)).await?
    };
    let note = missing_versions_note(&st, &manifest);
    let mut result = restore_impl(app, st, Origin::Archive(PathBuf::from(&path)), manifest, None, Some(name)).await?;
    result.warning = match (result.warning.take(), note) {
        (Some(w), Some(n)) => Some(format!("{n}\n\n{w}")),
        (w, n) => w.or(n),
    };
    Ok(result)
}

// ------------------------------------------------------------------ Importar do Local

const MSG_SEM_LOCAL: &str = "Nenhuma instalação do Local encontrada neste computador.";

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Pasta de dados do Local, se existir e tiver a lista de sites.
fn local_data() -> Result<(PathBuf, PathBuf), String> {
    let home = home_dir();
    let dir = crate::local_import::local_data_dir(&home).ok_or(MSG_SEM_LOCAL)?;
    if !dir.join("sites.json").is_file() {
        return Err(MSG_SEM_LOCAL.into());
    }
    Ok((dir, home))
}

/// Versão (série) que um site do Local pede, e o que fazer se ela não estiver instalada.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionNeed {
    pub wanted: String,
    pub installed: bool,
    pub install_version: Option<String>,
    pub fallback: Option<String>,
    pub fallback_older: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalImportCheck {
    pub php: VersionNeed,
    pub mysql: VersionNeed,
}

fn version_need(wanted: String, installed: Vec<String>, install_version: Option<String>) -> VersionNeed {
    let is_installed = installed.contains(&wanted);
    let (fallback, fallback_older) = if is_installed {
        (None, false)
    } else {
        match crate::local_import::fallback_version(&wanted, &installed) {
            Some((v, older)) => (Some(v), older),
            None => (None, false),
        }
    };
    VersionNeed { wanted, installed: is_installed, install_version: if is_installed { None } else { install_version }, fallback, fallback_older }
}

#[tauri::command]
pub async fn list_local_sites(_state: State<'_, AppState>) -> Result<Vec<crate::local_import::LocalSite>, String> {
    blocking(move || {
        let (dir, home) = local_data()?;
        crate::local_import::list_sites(&dir, &home)
    })
    .await
}

/// Compara as versões de PHP e MySQL do site com as instaladas aqui.
#[tauri::command]
pub async fn check_local_site(state: State<'_, AppState>, id: String) -> Result<LocalImportCheck, String> {
    let base = state.base.clone();
    blocking(move || {
        let (dir, home) = local_data()?;
        let site = crate::local_import::find_site(&dir, &home, &id)?;
        // Sem versão no Local: usa a mais nova instalada, sem perguntar (ou oferece a do catálogo).
        let php_have: Vec<String> = tools::php_installs(&base).into_iter().map(|i| i.version).collect();
        let php_cat: Vec<_> = php_manager::catalog(&base).unwrap_or_default().into_iter().filter(|r| r.latest).collect();
        let php_series: Vec<String> = php_cat.iter().map(|r| r.minor.clone()).collect();
        let php_wanted = crate::local_import::wanted_or_newest(
            &crate::local_import::series(&site.php_version),
            &php_have,
            &php_series,
        );
        let php_install = php_cat.into_iter().find(|r| r.minor == php_wanted).map(|r| r.version);

        let mysql_have: Vec<String> = mysql_manager::installs(&base).into_iter().map(|i| i.series).collect();
        let mysql_cat: Vec<_> = mysql_manager::catalog(&base).unwrap_or_default().into_iter().filter(|r| r.latest).collect();
        let mysql_series: Vec<String> = mysql_cat.iter().map(|r| r.series.clone()).collect();
        let mysql_wanted = crate::local_import::wanted_or_newest(
            &crate::local_import::series(&site.mysql_version),
            &mysql_have,
            &mysql_series,
        );
        let mysql_install = mysql_cat.into_iter().find(|r| r.series == mysql_wanted).map(|r| r.version);
        Ok(LocalImportCheck {
            php: version_need(php_wanted, php_have, php_install),
            mysql: version_need(mysql_wanted, mysql_have, mysql_install),
        })
    })
    .await
}

/// Cria um projeto novo com os arquivos e o banco de um site do Local (a origem só é lida).
#[tauri::command]
pub async fn import_local_site(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    name: String,
    php_version: String,
    mysql_version: String,
) -> Result<RestoreResult, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("informe um nome para o projeto".into());
    }
    let site = {
        let id = id.clone();
        blocking(move || {
            let (dir, home) = local_data()?;
            crate::local_import::find_site(&dir, &home, &id)
        })
        .await?
    };
    if let Some(b) = &site.blocker {
        return Err(b.clone());
    }
    let manifest = backup::Manifest {
        format: 1,
        id: project::slugify(&name),
        name: name.clone(),
        docroot: "public".into(),
        php_version,
        db_engine: project::ENGINE_MYSQL.into(),
        db_version: mysql_version,
        db_name: String::new(),
        http_port: 0,
        db_port: 0,
        site_url: String::new(),
        has_wordpress: true,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        kind: String::new(),
        blueprint_name: String::new(),
        description: String::new(),
    };
    let origin = Origin::Folder {
        public: crate::local_import::public_dir(&site),
        dump: crate::local_import::dump_file(&site),
    };
    let mut result = restore_impl(app, state.inner().clone(), origin, manifest, None, Some(name)).await?;
    let notes = site.warnings.join("\n");
    if !notes.is_empty() {
        result.warning = Some(match result.warning.take() {
            Some(w) => format!("{notes}\n\n{w}"),
            None => notes,
        });
    }
    Ok(result)
}

// ------------------------------------------------------------------ Ferramentas de API

/// Requisições que o PHP do projeto recebeu (a partir de `offset`; 0 = as mais recentes).
#[tauri::command]
pub async fn read_requests(state: State<'_, AppState>, id: String, offset: u64) -> Result<RequestsChunk, String> {
    state.find(&id)?;
    let base = state.base.clone();
    blocking(move || apitools::read_requests(&base, &id, offset)).await
}

#[tauri::command]
pub async fn clear_requests(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state.find(&id)?;
    let base = state.base.clone();
    blocking(move || apitools::clear_requests(&base, &id)).await
}

/// Coleção de requisições salvas do projeto (texto JSON; vazio se não houver).
#[tauri::command]
pub async fn load_collection(state: State<'_, AppState>, id: String) -> Result<String, String> {
    state.find(&id)?;
    let base = state.base.clone();
    blocking(move || apitools::load_collection(&base, &id)).await
}

#[tauri::command]
pub async fn save_collection(state: State<'_, AppState>, id: String, content: String) -> Result<(), String> {
    state.find(&id)?;
    let base = state.base.clone();
    blocking(move || apitools::save_collection(&base, &id, &content)).await
}

/// Exporta a coleção do projeto para o arquivo escolhido.
#[tauri::command]
pub async fn export_collection(state: State<'_, AppState>, id: String, path: String) -> Result<(), String> {
    state.find(&id)?;
    let base = state.base.clone();
    blocking(move || apitools::export_collection(&base, &id, std::path::Path::new(&path))).await
}

/// Lê um arquivo de coleção escolhido pelo usuário (para importar).
#[tauri::command]
pub async fn read_collection_file(path: String) -> Result<String, String> {
    blocking(move || apitools::read_collection_file(std::path::Path::new(&path))).await
}

/// Envia uma requisição HTTP (cliente de API).
#[tauri::command]
pub async fn send_request(request: ApiRequest) -> Result<ApiResponse, String> {
    blocking(move || apitools::send(&request)).await
}

#[cfg(test)]
mod auto_login_tests {
    use super::*;

    #[test]
    fn resolve_admin_aceita_quem_ainda_e_admin() {
        let admins = vec!["admin".to_string(), "ana".to_string()];
        assert_eq!(resolve_admin("ana", &admins).unwrap(), "ana");
        assert_eq!(
            resolve_admin("bia", &admins).unwrap_err(),
            "O usuário bia não é mais administrador deste site. Escolha outro no login automático."
        );
    }
}
