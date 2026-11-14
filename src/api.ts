import { invoke } from "@tauri-apps/api/core";

/** Banco de dados de um projeto. */
export type DbEngine = "mysql" | "postgres";

export interface Project {
  id: string;
  name: string;
  docroot: string;
  phpVersion: string;
  /** MySQL (padrão) ou PostgreSQL. */
  dbEngine: DbEngine;
  dbVersion: string;
  /** O banco já foi criado: o tipo e a versão não podem mais ser trocados. */
  dbInitialized: boolean;
  httpPort: number;
  dbPort: number;
  dbName: string;
  dbUser: string;
  dbPassword: string;
  /** Endereço principal (HTTPS quando ligado). */
  url: string;
  httpUrl: string;
  https: boolean;
  httpsPort: number;
  /** Aceita conexões de outros aparelhos da rede. */
  lan: boolean;
  /** Endereços para abrir de outros aparelhos (vazio se desligado ou sem rede). */
  lanUrls: string[];
  path: string;
  running: boolean;
  busy: boolean;
  hasAdminneo: boolean;
  hasMailpit: boolean;
  /** Há um WordPress instalado na pasta pública. */
  hasWordpress: boolean;
  /** A pasta pública tem arquivos além do index.php de exemplo. */
  docrootInUse: boolean;
  /** Redis (Valkey) ligado neste projeto. */
  redis: boolean;
  redisPort: number;
  /** Login automático no wp-admin ligado. */
  autoLogin: boolean;
  /** Administrador usado no login automático. */
  autoLoginUser: string;
}

export interface SslStatus {
  /** A autoridade de certificação local já foi criada. */
  caReady: boolean;
  /** O sistema confia nela (o navegador não avisa). */
  trusted: boolean;
  /** Caminho do certificado raiz, para instalar em outros aparelhos. */
  caPath: string;
  /** Dá para instalar a confiança automaticamente neste sistema. */
  canTrust: boolean;
}

export interface PhpInstall {
  fullVersion: string;
  version: string;
  fpm: string;
  /** Instalada pelo LocalTusk (true) ou vinda do sistema, ex.: Homebrew (false). */
  managed: boolean;
}

export interface PhpRelease {
  version: string;
  minor: string;
  size: string;
  installed: boolean;
  latest: boolean;
}

export interface MysqlInstall {
  fullVersion: string;
  series: string;
  mysqld: string;
  /** Instalado pelo LocalTusk (true) ou vindo do sistema, ex.: Homebrew (false). */
  managed: boolean;
}

export interface MysqlRelease {
  version: string;
  series: string;
  size: string;
  installed: boolean;
  latest: boolean;
}

export interface PostgresInstall {
  fullVersion: string;
  major: string;
  postgres: string;
}

export interface PostgresRelease {
  version: string;
  major: string;
  size: string;
  installed: boolean;
  latest: boolean;
}

export interface ValkeyInstall {
  fullVersion: string;
  server: string;
}

export interface ValkeyRelease {
  version: string;
  size: string;
  installed: boolean;
}

export interface InstallProgress {
  /** Versão do PHP (8.3.32), `mysql-<versão>`, `postgres-<versão>`, `valkey-<versão>`, `wordpress-<projeto>` ou id da ferramenta (mailpit). */
  id: string;
  stage: "download" | "extract" | "verify" | "done" | "error";
  downloaded: number;
  total: number;
  message: string | null;
}

export interface AppInfo {
  dataDir: string;
  phpDir: string;
  backupsDir: string;
  blueprintsDir: string;
  installing: string[];
}

/** Papel de um arquivo no mapa do projeto. */
export type FileKind = "entry" | "normal" | "orphan" | "unreachable";

export interface CodeLink {
  path: string;
  /** `include` (require/include) ou `classe` (uso de uma classe declarada no outro arquivo). */
  via: "include" | "classe";
}

export interface CodeFile {
  path: string;
  kind: FileKind;
  entryReason: string | null;
  lines: number;
  classes: string[];
  uses: CodeLink[];
  usedBy: CodeLink[];
  dynamicIncludes: number;
}

/** Quem chama quem no projeto, lido dos arquivos PHP (aba Mapa). */
export interface CodeMap {
  files: CodeFile[];
  totalLinks: number;
  entries: number;
  orphans: number;
  unreachable: number;
  dynamicIncludes: number;
  truncated: boolean;
  hasComposer: boolean;
  notes: string[];
}

/** Modelo de projeto (arquivos + banco + versões) guardado em `~/LocalTusk/blueprints`. */
export interface BlueprintInfo {
  fileName: string;
  path: string;
  name: string;
  description: string;
  size: number;
  /** Segundos desde 1970. */
  createdAt: number;
  /** Projeto a partir do qual foi criado. */
  sourceName: string;
  phpVersion: string;
  /** Vazio nos modelos antigos (sempre MySQL). */
  dbEngine?: DbEngine;
  dbVersion: string;
  docroot: string;
  hasWordpress: boolean;
}

export interface BackupInfo {
  fileName: string;
  path: string;
  size: number;
  /** Segundos desde 1970. */
  createdAt: number;
  projectId: string;
  projectName: string;
  hasWordpress: boolean;
}

export type HeaderPair = [string, string];

/** Requisição que o PHP do projeto recebeu (inspetor da aba API). */
export interface CapturedRequest {
  /** Início, em segundos desde 1970 (com fração). */
  time: number;
  method: string;
  /** Caminho e query, como chegou (`/api/users?page=2`). */
  uri: string;
  host: string;
  remote: string;
  requestHeaders: HeaderPair[];
  requestBody: string | null;
  requestBodyTruncated: boolean;
  requestSize: number;
  /** Campos de um formulário multipart (o corpo bruto não existe nesse caso). */
  form: Record<string, unknown> | null;
  files: { field: string; name: string; size: number }[];
  status: number;
  responseHeaders: HeaderPair[];
  responseBody: string | null;
  responseBodyTruncated: boolean;
  responseSize: number;
  durationMs: number;
}

export interface RequestsChunk {
  entries: CapturedRequest[];
  offset: number;
}

export interface ApiRequestOptions {
  method: string;
  url: string;
  headers: HeaderPair[];
  body?: string | null;
  followRedirects?: boolean;
  timeoutSecs?: number;
}

export interface ApiResponse {
  status: number;
  statusText: string;
  headers: HeaderPair[];
  body: string;
  binary: boolean;
  truncated: boolean;
  size: number;
  durationMs: number;
}

export interface RestoreResult {
  project: Project;
  /** Algo secundário falhou, mas o backup foi restaurado. */
  warning: string | null;
}

/** Site do Local (by Flywheel) que dá para importar. */
export interface LocalSite {
  id: string;
  name: string;
  path: string;
  domain: string;
  phpVersion: string;
  mysqlVersion: string;
  webServer: string;
  multisite: boolean;
  running: boolean;
  /** Segundos desde 1970 em que o Local salvou o banco (0 se nunca salvou). */
  dumpSavedAt: number;
  dumpStale: boolean;
  /** Motivo pelo qual o site não pode ser importado agora. */
  blocker: string | null;
  warnings: string[];
}

export interface VersionNeed {
  /** Série usada pelo site (ex.: 8.2). */
  wanted: string;
  installed: boolean;
  /** Versão completa para instalar (patch mais novo da série), se houver no catálogo. */
  installVersion: string | null;
  /** Série instalada mais próxima, se houver. */
  fallback: string | null;
  /** A alternativa é mais antiga que a versão do site. */
  fallbackOlder: boolean;
}

export interface LocalImportCheck {
  php: VersionNeed;
  mysql: VersionNeed;
}

export interface ExtraStatus {
  id: "caddy" | "adminneo" | "mailpit" | "wpcli";
  label: string;
  description: string;
  /** Necessária para rodar qualquer projeto. */
  essential: boolean;
  installed: boolean;
  version: string | null;
}

export type ToolId = "adminneo" | "mailpit";

export interface WpOptions {
  title: string;
  adminUser: string;
  adminPassword: string;
  adminEmail: string;
  /** Idioma do WordPress, ex.: pt_BR. */
  locale: string;
  /** Apaga o que houver na pasta pública antes de instalar. */
  replaceExisting: boolean;
}

export interface ToolStatus {
  name: string;
  found: boolean;
  path: string | null;
  installHint: string;
}

export const LOG_FILES = [
  "access.log",
  "php-error.log",
  "php-fpm.log",
  "mysql.log",
  "postgres.log",
  "valkey.log",
  "mailpit.log",
  "wp-cli.log",
  "caddy.log",
] as const;

export const api = {
  listProjects: () => invoke<Project[]>("list_projects"),
  checkTools: () => invoke<ToolStatus[]>("check_tools"),
  listPhpVersions: () => invoke<PhpInstall[]>("list_php_versions"),
  createProject: (
    name: string,
    phpVersion: string,
    dbVersion: string,
    docroot: string,
    dbEngine: DbEngine = "mysql",
    redis = false,
  ) => invoke<Project>("create_project", { name, phpVersion, dbVersion, docroot, dbEngine, redis }),
  startProject: (id: string) => invoke<Project>("start_project", { id }),
  stopProject: (id: string) => invoke<Project>("stop_project", { id }),
  deleteProject: (id: string) => invoke<void>("delete_project", { id }),
  setPhpVersion: (id: string, phpVersion: string) =>
    invoke<Project>("set_php_version", { id, phpVersion }),
  setDatabase: (id: string, dbEngine: DbEngine, dbVersion: string) =>
    invoke<Project>("set_database", { id, dbEngine, dbVersion }),
  setRedis: (id: string, enabled: boolean) => invoke<Project>("set_redis", { id, enabled }),
  setHttps: (id: string, enabled: boolean) => invoke<Project>("set_https", { id, enabled }),
  setLan: (id: string, enabled: boolean) => invoke<Project>("set_lan", { id, enabled }),
  sslStatus: () => invoke<SslStatus>("ssl_status"),
  trustCertificate: () => invoke<SslStatus>("trust_certificate"),
  revealCa: () => invoke<void>("reveal_ca"),
  listMysqlVersions: () => invoke<MysqlInstall[]>("list_mysql_versions"),
  mysqlCatalog: () => invoke<MysqlRelease[]>("mysql_catalog"),
  installMysql: (version: string) => invoke<void>("install_mysql", { version }),
  uninstallMysql: (version: string) => invoke<void>("uninstall_mysql", { version }),
  listPostgresVersions: () => invoke<PostgresInstall[]>("list_postgres_versions"),
  postgresCatalog: () => invoke<PostgresRelease[]>("postgres_catalog"),
  installPostgres: (version: string) => invoke<void>("install_postgres", { version }),
  uninstallPostgres: (version: string) => invoke<void>("uninstall_postgres", { version }),
  listValkeyVersions: () => invoke<ValkeyInstall[]>("list_valkey_versions"),
  valkeyCatalog: () => invoke<ValkeyRelease[]>("valkey_catalog"),
  installValkey: (version: string) => invoke<void>("install_valkey", { version }),
  uninstallValkey: (version: string) => invoke<void>("uninstall_valkey", { version }),
  openSite: (id: string, path?: string) => invoke<void>("open_site", { id, path }),
  setAutoLogin: (id: string, enabled: boolean, user: string) =>
    invoke<Project>("set_auto_login", { id, enabled, user }),
  listWpAdmins: (id: string) => invoke<string[]>("list_wp_admins", { id }),
  openWpAdmin: (id: string) => invoke<string | null>("open_wp_admin", { id }),
  openTerminal: (id: string) => invoke<void>("open_terminal", { id }),
  installWordpress: (id: string, options: WpOptions) =>
    invoke<void>("install_wordpress", { id, options }),
  readRequests: (id: string, offset: number) => invoke<RequestsChunk>("read_requests", { id, offset }),
  clearRequests: (id: string) => invoke<void>("clear_requests", { id }),
  loadCollection: (id: string) => invoke<string>("load_collection", { id }),
  saveCollection: (id: string, content: string) => invoke<void>("save_collection", { id, content }),
  exportCollection: (id: string, path: string) => invoke<void>("export_collection", { id, path }),
  readCollectionFile: (path: string) => invoke<string>("read_collection_file", { path }),
  sendRequest: (request: ApiRequestOptions) => invoke<ApiResponse>("send_request", { request }),
  listBackups: (id?: string) => invoke<BackupInfo[]>("list_backups", { id }),
  createBackup: (id: string) => invoke<BackupInfo>("create_backup", { id }),
  /** Sem `target`, cria um projeto novo; com `target`, substitui o conteúdo desse projeto. */
  restoreBackup: (path: string, target?: string) => invoke<RestoreResult>("restore_backup", { path, target }),
  deleteBackup: (path: string) => invoke<void>("delete_backup", { path }),
  openBackupsDir: () => invoke<void>("open_backups_dir"),
  listBlueprints: () => invoke<BlueprintInfo[]>("list_blueprints"),
  createBlueprint: (id: string, name: string, description: string) =>
    invoke<BlueprintInfo>("create_blueprint", { id, name, description }),
  createProjectFromBlueprint: (path: string, name: string) =>
    invoke<RestoreResult>("create_project_from_blueprint", { path, name }),
  listLocalSites: () => invoke<LocalSite[]>("list_local_sites"),
  checkLocalSite: (id: string) => invoke<LocalImportCheck>("check_local_site", { id }),
  importLocalSite: (id: string, name: string, phpVersion: string, mysqlVersion: string) =>
    invoke<RestoreResult>("import_local_site", { id, name, phpVersion, mysqlVersion }),
  deleteBlueprint: (path: string) => invoke<void>("delete_blueprint", { path }),
  importBlueprint: (path: string) => invoke<BlueprintInfo>("import_blueprint", { path }),
  /** Devolve o caminho gravado (a extensão `.tar.gz` é acrescentada se faltar). */
  exportBlueprint: (path: string, dest: string) => invoke<string>("export_blueprint", { path, dest }),
  openBlueprintsDir: () => invoke<void>("open_blueprints_dir"),
  openFolder: (id: string) => invoke<void>("open_folder", { id }),
  readLog: (id: string, file: string) => invoke<string>("read_log", { id, file }),
  analyzeProject: (id: string) => invoke<CodeMap>("analyze_project", { id }),
  appInfo: () => invoke<AppInfo>("app_info"),
  openDataDir: () => invoke<void>("open_data_dir"),
  phpCatalog: () => invoke<PhpRelease[]>("php_catalog"),
  installPhp: (version: string) => invoke<void>("install_php", { version }),
  uninstallPhp: (version: string) => invoke<void>("uninstall_php", { version }),
  extrasStatus: () => invoke<ExtraStatus[]>("extras_status"),
  installExtra: (id: string) => invoke<void>("install_extra", { id }),
  uninstallExtra: (id: string) => invoke<void>("uninstall_extra", { id }),
  openTool: (id: string, tool: ToolId) => invoke<void>("open_tool", { id, tool }),
};

/** Encontra o PHP que de fato rodaria para o valor salvo no projeto. */
export function resolvePhp(phps: PhpInstall[], wanted: string): PhpInstall | null {
  return (
    phps.find((p) => p.fullVersion === wanted) ??
    phps.find((p) => p.version === wanted) ??
    phps[0] ??
    null
  );
}

/** Encontra o MySQL que de fato rodaria para o valor salvo no projeto. */
export function resolveMysql(list: MysqlInstall[], wanted: string): MysqlInstall | null {
  return (
    list.find((m) => m.fullVersion === wanted) ??
    list.find((m) => m.series === wanted) ??
    list[0] ??
    null
  );
}

/** Encontra o PostgreSQL que de fato rodaria para o valor salvo no projeto (versão completa ou principal). */
export function resolvePostgres(list: PostgresInstall[], wanted: string): PostgresInstall | null {
  return (
    list.find((m) => m.fullVersion === wanted) ??
    list.find((m) => m.major === wanted) ??
    list[0] ??
    null
  );
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}
