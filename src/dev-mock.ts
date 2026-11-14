// Simula o backend do Tauri para desenvolver a interface no navegador
// (`pnpm dev` + http://localhost:1420). Só é carregado em desenvolvimento e fora do Tauri.
import { mockIPC } from "@tauri-apps/api/mocks";

const projects = [
  { id: "loja-virtual", name: "Loja Virtual", docroot: "public", phpVersion: "8.3.32", dbVersion: "8.4.13", dbInitialized: true, httpPort: 8000, dbPort: 3307, dbName: "loja_virtual", dbUser: "root", dbPassword: "root", url: "http://loja-virtual.localhost:8000", httpUrl: "http://loja-virtual.localhost:8000", https: false, httpsPort: 0, lan: false, lanUrls: [] as string[], path: "/Users/dev/LocalTusk/projects/loja-virtual", running: true, busy: false, hasAdminneo: true, hasMailpit: true, hasWordpress: false, docrootInUse: true, dbEngine: "mysql", redis: false, redisPort: 0, autoLogin: false, autoLoginUser: "" },
  { id: "blog-wp", name: "Blog WordPress", docroot: "", phpVersion: "8.4.9", dbVersion: "8.4.13", dbInitialized: true, httpPort: 8001, dbPort: 3308, dbName: "blog_wp", dbUser: "root", dbPassword: "root", url: "http://blog-wp.localhost:8001", httpUrl: "http://blog-wp.localhost:8001", https: false, httpsPort: 0, lan: false, lanUrls: [] as string[], path: "/Users/dev/LocalTusk/projects/blog-wp", running: true, busy: false, hasAdminneo: true, hasMailpit: true, hasWordpress: true, docrootInUse: true, dbEngine: "mysql", redis: false, redisPort: 0, autoLogin: false, autoLoginUser: "" },
  { id: "api-legada", name: "API Legada", docroot: "", phpVersion: "8.1.33", dbVersion: "", dbInitialized: false, httpPort: 8002, dbPort: 3309, dbName: "api_legada", dbUser: "root", dbPassword: "root", url: "http://api-legada.localhost:8002", httpUrl: "http://api-legada.localhost:8002", https: false, httpsPort: 0, lan: false, lanUrls: [] as string[], path: "/Users/dev/LocalTusk/projects/api-legada", running: false, busy: false, hasAdminneo: false, hasMailpit: false, hasWordpress: false, docrootInUse: false, dbEngine: "mysql", redis: false, redisPort: 0, autoLogin: false, autoLoginUser: "" },
  { id: "api-pg", name: "API PostgreSQL", docroot: "public", phpVersion: "8.3.32", dbEngine: "postgres", dbVersion: "17.12.0", dbInitialized: true, httpPort: 8003, dbPort: 5433, dbName: "api_pg", dbUser: "root", dbPassword: "root", url: "http://api-pg.localhost:8003", httpUrl: "http://api-pg.localhost:8003", https: false, httpsPort: 0, lan: false, lanUrls: [] as string[], path: "/Users/dev/LocalTusk/projects/api-pg", running: true, busy: false, hasAdminneo: true, hasMailpit: true, hasWordpress: false, docrootInUse: true, redis: true, redisPort: 6380, autoLogin: false, autoLoginUser: "" },
];

let sslTrusted = false;

const phps = ["8.4.9", "8.3.32", "8.1.33"].map((v) => ({ fullVersion: v, version: v.split(".").slice(0, 2).join("."), fpm: `/x/${v}/php-fpm`, managed: true }));
const mysqls = [{ fullVersion: "8.4.13", series: "8.4", mysqld: "/x/mysqld", managed: true }];
const postgreses = [{ fullVersion: "17.12.0", major: "17", postgres: "/x/pg/17/bin/postgres" }];
const valkeys = [{ fullVersion: "9.0.7", server: "/x/valkey/bin/valkey-server" }];
const extras = [
  { id: "caddy", label: "Caddy", description: "Servidor web de cada projeto. Obrigatório.", essential: true, installed: true, version: "2.11.9" },
  { id: "adminneo", label: "Adminer Neo", description: "Administração do MySQL e do PostgreSQL pelo navegador, leve e moderna, com login automático.", essential: false, installed: true, version: "5.2.0" },
  { id: "mailpit", label: "Mailpit", description: "Caixa de e-mails de teste: tudo que o PHP enviar com mail() ou SMTP cai aqui.", essential: false, installed: true, version: "1.27.1" },
  { id: "wpcli", label: "WP-CLI", description: "A linha de comando do WordPress (comando wp). Necessária para o botão \"Instalar WordPress\".", essential: false, installed: false, version: null },
];
const phpCatalog = [["8.5", "8.5.1", "8.5.0"], ["8.4", "8.4.9", "8.4.8", "8.4.7"], ["8.3", "8.3.32", "8.3.31"], ["8.2", "8.2.30"], ["8.1", "8.1.33", "8.1.32"], ["8.0", "8.0.30"]].flatMap(([minor, ...vs]) =>
  vs.map((version, i) => ({ version, minor, size: "33.9M", installed: phps.some((p) => p.fullVersion === version), latest: i === 0 })));
const mysqlCatalog = [["9.7", "9.7.4"], ["8.4", "8.4.13", "8.4.12"], ["8.0", "8.0.46", "8.0.45"]].flatMap(([series, ...vs]) =>
  vs.map((version, i) => ({ version, series, size: "165.4M", installed: version === "8.4.13", latest: i === 0 })));

const postgresCatalog = [["18", "18.7.0"], ["17", "17.12.0"], ["16", "16.16.0"], ["15", "15.20.0"], ["14", "14.25.0"]].map(([major, version], i) => ({ version, major, size: "84 MB", installed: version === "17.12.0", latest: true && i >= 0 }));
const valkeyCatalog = [{ version: "9.0.7", size: "4 MB", installed: true }];

const backups = [
  { fileName: "loja-virtual-20261111-103000.tar.gz", path: "/Users/dev/LocalTusk/backups/loja-virtual-20261111-103000.tar.gz", size: 48_300_000, createdAt: 1794486600, projectId: "loja-virtual", projectName: "Loja Virtual", hasWordpress: false },
  { fileName: "loja-virtual-antes-do-restore-20261110-170211.tar.gz", path: "/Users/dev/LocalTusk/backups/loja-virtual-antes-do-restore-20261110-170211.tar.gz", size: 47_100_000, createdAt: 1794427331, projectId: "loja-virtual", projectName: "Loja Virtual", hasWordpress: false },
];

const blueprints = [
  { fileName: "landing-page.tar.gz", path: "/Users/dev/LocalTusk/blueprints/landing-page.tar.gz", name: "Landing page", description: "Base com formulário de contato e e-mail pelo Mailpit.", size: 3_400_000, createdAt: 1794486600, sourceName: "Landing Teste", phpVersion: "8.3.32", dbVersion: "8.4.13", docroot: "public", hasWordpress: false },
  { fileName: "wordpress-limpo.tar.gz", path: "/Users/dev/LocalTusk/blueprints/wordpress-limpo.tar.gz", name: "WordPress limpo", description: "", size: 61_800_000, createdAt: 1794400000, sourceName: "Blog WordPress", phpVersion: "8.1.33", dbVersion: "8.4.13", docroot: "", hasWordpress: true },
];

const localSites = [
  { id: "aaa111", name: "Padaria do Zé", path: "/home/dev/Local Sites/padaria", domain: "padaria.local", phpVersion: "8.2.29", mysqlVersion: "8.0.35", webServer: "nginx", multisite: false, running: false, dumpSavedAt: 1794486600, dumpStale: false, blocker: null, warnings: [] as string[] },
  { id: "bbb222", name: "Clínica Sorriso", path: "/home/dev/Local Sites/clinica", domain: "clinica.local", phpVersion: "8.3.32", mysqlVersion: "8.4.13", webServer: "nginx", multisite: false, running: true, dumpSavedAt: 1794400000, dumpStale: false, blocker: "Pare o site no Local para importar (se o Local estiver fechado, abra-o e pare o site).", warnings: [] as string[] },
  { id: "ccc333", name: "Rede de Blogs", path: "/home/dev/Local Sites/blogs", domain: "blogs.local", phpVersion: "8.1.33", mysqlVersion: "8.4.0", webServer: "apache", multisite: true, running: false, dumpSavedAt: 1794300000, dumpStale: true, blocker: null, warnings: ["Sites multisite podem precisar de ajustes manuais.", "Regras do .htaccess não são usadas pelo LocalTusk.", "O banco salvo pelo Local pode estar desatualizado. Inicie e pare o site no Local para garantir."] },
];

const captured = [
  { time: 1794486601.2, method: "GET", uri: "/api/produtos?page=2&limit=10", host: "loja-virtual.localhost:8000", remote: "127.0.0.1", requestHeaders: [["Host", "loja-virtual.localhost:8000"], ["Accept", "application/json"], ["User-Agent", "curl/8.7.1"]], requestBody: "", requestBodyTruncated: false, requestSize: 0, form: null, files: [], status: 200, responseHeaders: [["Content-Type", "application/json"], ["X-Powered-By", "PHP/8.3.32"]], responseBody: JSON.stringify({ data: [{ id: 1, nome: "Camiseta", preco: 59.9 }, { id: 2, nome: "Caneca", preco: 24.5 }], page: 2 }), responseBodyTruncated: false, responseSize: 112, durationMs: 18.4 },
  { time: 1794486608.9, method: "POST", uri: "/api/pedidos", host: "loja-virtual.localhost:8000", remote: "127.0.0.1", requestHeaders: [["Host", "loja-virtual.localhost:8000"], ["Content-Type", "application/json"], ["Authorization", "Bearer eyJhbGciOi..."]], requestBody: JSON.stringify({ cliente: "Ana", itens: [{ produto: 1, qtd: 2 }] }), requestBodyTruncated: false, requestSize: 58, form: null, files: [], status: 201, responseHeaders: [["Content-Type", "application/json"], ["Location", "/api/pedidos/42"]], responseBody: JSON.stringify({ id: 42, status: "criado" }), responseBodyTruncated: false, responseSize: 28, durationMs: 41.7 },
  { time: 1794486615.3, method: "PUT", uri: "/api/pedidos/999", host: "loja-virtual.localhost:8000", remote: "127.0.0.1", requestHeaders: [["Content-Type", "application/json"]], requestBody: "{\"status\":\"pago\"}", requestBodyTruncated: false, requestSize: 17, form: null, files: [], status: 404, responseHeaders: [["Content-Type", "application/json"]], responseBody: JSON.stringify({ erro: "Pedido não encontrado" }), responseBodyTruncated: false, responseSize: 32, durationMs: 6.1 },
  { time: 1794486622.0, method: "GET", uri: "/api/relatorio", host: "loja-virtual.localhost:8000", remote: "127.0.0.1", requestHeaders: [], requestBody: "", requestBodyTruncated: false, requestSize: 0, form: null, files: [], status: 500, responseHeaders: [["Content-Type", "text/html; charset=UTF-8"]], responseBody: "<br />\n<b>Fatal error</b>: Uncaught Error: Call to undefined function gerar() in /app/api/relatorio.php:12", responseBodyTruncated: false, responseSize: 98, durationMs: 9.8 },
];

let collectionText = JSON.stringify({ format: "localtusk-collection", version: 1, requests: [
  { id: "c1", name: "Listar tarefas", method: "GET", url: "/tarefas?limite=5", headers: [{ key: "Accept", value: "application/json", enabled: true }], bodyMode: "none", body: "", followRedirects: false },
  { id: "c2", name: "Criar tarefa", method: "POST", url: "/tarefas", headers: [{ key: "Accept", value: "application/json", enabled: true }], bodyMode: "json", body: "{\"titulo\":\"Nova\"}", followRedirects: false },
] });

const logs = `[10/Nov/2026:17:02:11] GET / 200 14ms
[10/Nov/2026:17:02:12] GET /assets/app.css 200 2ms
[10/Nov/2026:17:02:14] POST /api/cart 201 38ms
[10/Nov/2026:17:02:20] GET /produtos?page=2 200 22ms
[10/Nov/2026:17:02:31] GET /favicon.ico 404 1ms`;

/** Com `?slow` na URL, as listas demoram a chegar: serve para ver os esqueletos de carregamento. */
const slow = new URLSearchParams(location.search).has("slow");
const L = (path: string, via: "include" | "classe" = "include") => ({ path, via });
const codeMap = {
  files: [
    { path: "public/index.php", kind: "entry", entryReason: "pasta pública", lines: 24, classes: [], uses: [L("src/bootstrap.php"), L("src/Controllers/HomeController.php", "classe")], usedBy: [], dynamicIncludes: 0 },
    { path: "src/bootstrap.php", kind: "normal", entryReason: null, lines: 41, classes: [], uses: [L("src/config.php"), L("lib/helpers.php"), L("src/Database.php", "classe")], usedBy: [L("public/index.php")], dynamicIncludes: 0 },
    { path: "src/config.php", kind: "normal", entryReason: null, lines: 18, classes: [], uses: [], usedBy: [L("src/bootstrap.php")], dynamicIncludes: 0 },
    { path: "lib/helpers.php", kind: "normal", entryReason: null, lines: 63, classes: [], uses: [], usedBy: [L("src/bootstrap.php")], dynamicIncludes: 0 },
    { path: "src/Database.php", kind: "normal", entryReason: null, lines: 77, classes: ["App\\Database"], uses: [], usedBy: [L("src/bootstrap.php", "classe"), L("src/Models/User.php", "classe")], dynamicIncludes: 0 },
    { path: "src/Controllers/HomeController.php", kind: "normal", entryReason: null, lines: 52, classes: ["App\\Controllers\\HomeController"], uses: [L("src/Models/User.php", "classe")], usedBy: [L("public/index.php", "classe")], dynamicIncludes: 0 },
    { path: "src/Models/User.php", kind: "normal", entryReason: null, lines: 96, classes: ["App\\Models\\User"], uses: [L("src/Database.php", "classe")], usedBy: [L("src/Controllers/HomeController.php", "classe")], dynamicIncludes: 0 },
    { path: "src/Legacy/Exporter.php", kind: "orphan", entryReason: null, lines: 134, classes: ["App\\Legacy\\Exporter"], uses: [L("src/Legacy/Csv.php", "classe")], usedBy: [], dynamicIncludes: 0 },
    { path: "src/Legacy/Csv.php", kind: "unreachable", entryReason: null, lines: 58, classes: ["App\\Legacy\\Csv"], uses: [], usedBy: [L("src/Legacy/Exporter.php", "classe")], dynamicIncludes: 0 },
    { path: "src/teste-antigo.php", kind: "orphan", entryReason: null, lines: 12, classes: [], uses: [], usedBy: [], dynamicIncludes: 1 },
  ],
  totalLinks: 12, entries: 1, orphans: 2, unreachable: 1, dynamicIncludes: 1, truncated: false, hasComposer: true,
  notes: ["1 require/include usam variáveis e não puderam ser seguidos. Arquivos carregados assim aparecem como órfãos por engano."],
};

const later = <T>(value: T) => new Promise<T>((resolve) => setTimeout(() => resolve(value), slow ? 9000 : 0));

function lanUrlsFor(p: (typeof projects)[number]) {
  return ["192.168.0.23", "meu-mac.local"].flatMap((h) => [`http://${h}:${p.httpPort}`, ...(p.https ? [`https://${h}:${p.httpsPort}`] : [])]);
}

export function installDevMock() {
  mockIPC(
    (cmd, args) => {
      const a = (args ?? {}) as Record<string, unknown>;
      switch (cmd) {
        case "list_projects": return later(projects);
        case "check_tools": return [];
        case "list_php_versions": return phps;
        case "list_mysql_versions": return mysqls;
        case "list_postgres_versions": return postgreses;
        case "list_valkey_versions": return valkeys;
        case "postgres_catalog": return postgresCatalog;
        case "valkey_catalog": return valkeyCatalog;
        case "set_database": {
          const p = projects.find((x) => x.id === a.id)!;
          Object.assign(p, { dbEngine: a.dbEngine, dbVersion: a.dbVersion, dbPort: a.dbEngine === "postgres" ? 5434 : 3309 });
          return later(p);
        }
        case "set_redis": {
          const p = projects.find((x) => x.id === a.id)!;
          p.redis = a.enabled as boolean;
          if (p.redis && !p.redisPort) p.redisPort = 6381;
          return later(p);
        }
        case "extras_status": return extras;
        case "app_info": return { dataDir: "/Users/dev/LocalTusk", phpDir: "/Users/dev/LocalTusk/bin/php", backupsDir: "/Users/dev/LocalTusk/backups", blueprintsDir: "/Users/dev/LocalTusk/blueprints", installing: [] };
        case "read_requests": {
          return (a.offset as number) > 0 ? { entries: [], offset: 1 } : { entries: captured, offset: 1 };
        }
        case "clear_requests": return null;
        case "load_collection": return collectionText;
        case "save_collection": collectionText = a.content as string; return null;
        case "export_collection": return null;
        case "read_collection_file": return collectionText;
        case "send_request": {
          const r = a.request as { method: string; url: string };
          return { status: r.url.includes("erro") ? 500 : 200, statusText: r.url.includes("erro") ? "Internal Server Error" : "OK", headers: [["content-type", "application/json"], ["server", "Caddy"]], body: JSON.stringify({ ok: true, metodo: r.method, url: r.url, itens: [1, 2, 3] }), binary: false, truncated: false, size: 71, durationMs: 23 };
        }
        case "list_backups": return backups.filter((b) => !a.id || b.projectId === a.id);
        case "create_backup": return backups[0];
        case "restore_backup": return { project: projects[0], warning: null };
        case "list_blueprints": return later(blueprints);
        case "create_blueprint": {
          const bp = { fileName: "novo.tar.gz", path: "/Users/dev/LocalTusk/blueprints/novo.tar.gz", name: a.name as string, description: a.description as string, size: 12_000_000, createdAt: 1794490000, sourceName: "Loja Virtual", phpVersion: "8.3.32", dbVersion: "8.4.13", docroot: "public", hasWordpress: false };
          blueprints.unshift(bp);
          return bp;
        }
        case "create_project_from_blueprint": return { project: projects[0], warning: "O blueprint foi feito com PHP 8.1.33, que não está instalado aqui. O projeto usa as versões mais recentes instaladas." };
        case "list_local_sites":
          // Com "?semlocal" na URL, simula o Local não instalado.
          return new URLSearchParams(location.search).has("semlocal") ? Promise.reject("Nenhuma instalação do Local encontrada neste computador.") : later(localSites);
        case "check_local_site": {
          const s = localSites.find((x) => x.id === a.id)!;
          const missingPhp = s.phpVersion.startsWith("8.2");
          return later({
            php: missingPhp
              ? { wanted: "8.2", installed: false, installVersion: "8.2.30", fallback: "8.3", fallbackOlder: false }
              : { wanted: s.phpVersion.split(".").slice(0, 2).join("."), installed: true, installVersion: null, fallback: null, fallbackOlder: false },
            mysql: s.mysqlVersion.startsWith("8.0")
              ? { wanted: "8.0", installed: false, installVersion: "8.0.46", fallback: "8.4", fallbackOlder: false }
              : { wanted: "8.4", installed: true, installVersion: null, fallback: null, fallbackOlder: false },
          });
        }
        case "import_local_site": return new Promise((resolve) => setTimeout(() => resolve({ project: projects[0], warning: "Pastas ligadas por link foram copiadas como pastas comuns: wp-content/uploads." }), 800));
        case "install_php":
        case "install_mysql": return new Promise((resolve) => setTimeout(() => resolve(null), 1200));
        case "delete_blueprint": {
          const i = blueprints.findIndex((b) => b.path === a.path);
          if (i >= 0) blueprints.splice(i, 1);
          return null;
        }
        case "import_blueprint": return blueprints[0];
        case "export_blueprint": return a.dest;
        case "plugin:dialog|save": return "/Users/dev/Desktop/blueprint.tar.gz";
        case "plugin:dialog|open": return "/Users/dev/LocalTusk/backups/loja-virtual-20261110-170211.tar.gz";
        case "php_catalog": return phpCatalog;
        case "mysql_catalog": return mysqlCatalog;
        case "read_log": return logs;
        case "analyze_project": return later(codeMap);
        case "ssl_status": return { caReady: sslTrusted || projects.some((x) => x.https), trusted: sslTrusted, caPath: "/Users/dev/LocalTusk/ssl/caddy/pki/authorities/local/root.crt", canTrust: true };
        case "trust_certificate": {
          // Com "?cancelar" na URL, simula a pessoa cancelando o pedido de senha do macOS.
          if (new URLSearchParams(location.search).has("cancelar")) return Promise.reject("Autorização cancelada. O HTTPS continua ligado, mas o navegador vai avisar que o certificado não é confiável.");
          sslTrusted = true;
          return { caReady: true, trusted: true, caPath: "/Users/dev/LocalTusk/ssl/caddy/pki/authorities/local/root.crt", canTrust: true };
        }
        case "reveal_ca": return null;
        case "set_https": {
          const p = projects.find((x) => x.id === a.id)!;
          p.https = a.enabled as boolean;
          if (p.https && !p.httpsPort) p.httpsPort = 8443 + projects.indexOf(p);
          p.url = p.https ? `https://${p.id}.localhost:${p.httpsPort}` : p.httpUrl;
          p.lanUrls = p.lan ? lanUrlsFor(p) : [];
          return later(p);
        }
        case "set_auto_login": {
          const p = projects.find((x) => x.id === a.id)!;
          p.autoLogin = a.enabled as boolean;
          p.autoLoginUser = a.user as string;
          return later(p);
        }
        case "list_wp_admins": return later(["admin", "editor-chefe"]);
        case "open_wp_admin": return null;
        case "set_lan": {
          const p = projects.find((x) => x.id === a.id)!;
          p.lan = a.enabled as boolean;
          p.lanUrls = p.lan ? lanUrlsFor(p) : [];
          return later(p);
        }
        case "start_project":
        case "stop_project": {
          const p = projects.find((x) => x.id === a.id)!;
          // O estado só muda depois da espera, como no app de verdade.
          return later(p).then(() => {
            p.running = cmd === "start_project";
            p.hasAdminneo = p.hasMailpit = p.running;
            return p;
          });
        }
        default: return null;
      }
    },
    { shouldMockEvents: true },
  );
}
