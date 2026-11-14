//! Mapa do projeto: lê os arquivos PHP da pasta do projeto e descobre quem chama quem.
//!
//! A análise é estática e propositalmente simples (sem executar nada). Ela enxerga:
//! - `require`/`include` com caminho literal, `__DIR__` ou `dirname(__FILE__)`;
//! - classes usadas por `use`, `new`, `Classe::`, `extends`, `implements`, `instanceof`, `catch`
//!   e tipos de parâmetro, retorno e propriedade, ligadas ao arquivo que declara a classe;
//! - hooks do WordPress: `add_action`/`add_filter` com nome e callback literais (função,
//!   `'Classe::metodo'`, `['Classe', 'metodo']`, `[Classe::class, 'metodo']` ou closure) e
//!   quem dispara o hook com `do_action`/`apply_filters`. Em projetos WordPress, o arquivo
//!   principal de cada plugin, os mu-plugins e o `functions.php` do tema são pontos de entrada.
//!
//! O que ela não consegue ver (`require $variavel`, hook com nome montado em variável, rotas
//! por nome) fica registrado em `dynamic_includes`, para a tela avisar que pode ter falsos órfãos.

use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

const MAX_FILES: usize = 5000;
const MAX_BYTES: u64 = 1_000_000;
const SKIP_DIRS: &[&str] = &["vendor", "node_modules", ".git", ".svn", ".hg", ".idea", ".vscode", "storage"];
const EXTENSIONS: &[&str] = &["php", "phtml", "inc"];

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Ponto de entrada: o navegador, o Composer ou um script chama o arquivo direto.
    Entry,
    /// Alguma entrada chega até ele.
    Normal,
    /// Ninguém chama este arquivo.
    Orphan,
    /// Só é chamado por outros arquivos que também não são alcançados por nenhuma entrada.
    Unreachable,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub path: String,
    /// `include` (require/include), `classe` (uso de uma classe declarada no outro arquivo)
    /// ou `hook` (hook do WordPress ligando quem registra/dispara ao dono do callback).
    pub via: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    pub path: String,
    pub kind: Kind,
    /// Por que é uma entrada (só quando `kind` é `entry`).
    pub entry_reason: Option<String>,
    pub lines: u32,
    pub classes: Vec<String>,
    /// Hooks do WordPress que este arquivo registra (`add_action`/`add_filter` literais).
    pub hooks: Vec<String>,
    pub uses: Vec<Link>,
    pub used_by: Vec<Link>,
    /// `require`/`include` cujo caminho depende de variável e que a análise não resolve.
    pub dynamic_includes: u32,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodeMap {
    pub files: Vec<FileInfo>,
    pub total_links: u32,
    pub entries: u32,
    pub orphans: u32,
    pub unreachable: u32,
    pub dynamic_includes: u32,
    /// A pasta tinha mais arquivos que o limite; só parte deles foi lida.
    pub truncated: bool,
    pub has_composer: bool,
    pub notes: Vec<String>,
}

// ------------------------------------------------------------------ leitura do código

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("regex válida"))
}

/// Mantém só o código PHP (entre `<?php` e `?>`), cada trecho terminado em `;`.
fn php_only(src: &str) -> String {
    let mut out = String::new();
    let mut rest = src;
    while let Some(start) = rest.find("<?") {
        let after = &rest[start + 2..];
        let body = if let Some(b) = after.strip_prefix("php") {
            b
        } else if let Some(b) = after.strip_prefix('=') {
            b
        } else {
            rest = after;
            continue;
        };
        match body.find("?>") {
            Some(end) => {
                out.push_str(&body[..end]);
                out.push_str(";\n");
                rest = &body[end + 2..];
            }
            None => {
                out.push_str(body);
                out.push_str(";\n");
                break;
            }
        }
    }
    out
}

/// Tira os comentários e devolve duas versões do código: uma com os textos entre aspas
/// (para ler caminhos de `require`) e outra com esses textos esvaziados (para o resto,
/// assim "class Foo" dentro de uma frase não vira uma classe).
fn clean(code: &str) -> (String, String) {
    let chars: Vec<char> = code.chars().collect();
    let mut with = String::with_capacity(code.len());
    let mut without = String::with_capacity(code.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '/' if next == Some('/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '#' if next != Some('[') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if next == Some('*') => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    if chars[i] == '\n' {
                        with.push('\n');
                        without.push('\n');
                    }
                    i += 1;
                }
                i += 2;
                with.push(' ');
                without.push(' ');
            }
            '\'' | '"' => {
                let quote = c;
                with.push(quote);
                without.push(quote);
                i += 1;
                while i < chars.len() && chars[i] != quote {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        with.push(chars[i]);
                        with.push(chars[i + 1]);
                        i += 2;
                        continue;
                    }
                    with.push(chars[i]);
                    if chars[i] == '\n' {
                        without.push('\n');
                    }
                    i += 1;
                }
                with.push(quote);
                without.push(quote);
                i += 1;
            }
            _ => {
                with.push(c);
                without.push(c);
                i += 1;
            }
        }
    }
    (with, without)
}

/// Callback de um `add_action`/`add_filter` como dá para resolver sem rodar nada.
#[derive(Debug, Clone, PartialEq)]
enum HookCb {
    /// Nome de uma função solta: `'minha_funcao'`.
    Func(String),
    /// Classe do callback, já com os candidatos de nome completo (`'Classe::m'`, `['Classe', 'm']`).
    Class(Vec<String>),
    /// Closure ou método do próprio arquivo (`[$this, 'm']`, `[self::class, 'm']` sem classe clara).
    Own,
}

#[derive(Debug, Default)]
struct Parsed {
    lines: u32,
    declared: Vec<String>,
    /// Nomes de classe como aparecem no código, já resolvidos para o nome completo (candidatos).
    class_refs: Vec<Vec<String>>,
    includes: Vec<String>,
    /// Funções nomeadas declaradas no arquivo (`function nome(...)`), para callbacks em texto.
    functions: Vec<String>,
    /// (hook, callback) de cada `add_action`/`add_filter` com nome literal.
    hooks_reg: Vec<(String, HookCb)>,
    /// Hooks disparados com `do_action`/`apply_filters` de nome literal.
    hooks_fired: Vec<String>,
    /// O começo do arquivo tem o cabeçalho `Plugin Name:` de um plugin do WordPress.
    plugin_header: bool,
}

/// Classifica o callback que vem logo depois de `add_action('hook',` no código-fonte.
fn parse_callback(chunk: &str, resolve: &dyn Fn(&str) -> Vec<String>, declared: &[String]) -> Option<HookCb> {
    let c = chunk.trim_start();
    // Closure: `function (...)`, `fn (...)`, `static function (...)`.
    let lower = c.to_ascii_lowercase();
    for kw in ["static function", "static fn", "function", "fn"] {
        if lower.starts_with(kw) && c[kw.len()..].starts_with([' ', '(']) {
            return Some(HookCb::Own);
        }
    }
    // Texto: `'funcao'` ou `'Classe::metodo'`.
    if c.starts_with(['\'', '"']) {
        let q = c.chars().next()?;
        let inner = &c[1..c[1..].find(q).map(|e| e + 1)?];
        return Some(match inner.split_once("::") {
            Some((class, _)) if class.eq_ignore_ascii_case("self") || class.eq_ignore_ascii_case("static") => {
                declared.first().map(|d| HookCb::Class(vec![d.clone()])).unwrap_or(HookCb::Own)
            }
            Some((class, _)) => HookCb::Class(resolve(class)),
            None => HookCb::Func(inner.trim_start_matches('\\').to_string()),
        });
    }
    // Array: `['Classe', 'm']`, `[Classe::class, 'm']`, `[$this, 'm']`, `array('Classe', 'm')`.
    let inner = if let Some(rest) = c.strip_prefix('[') {
        rest
    } else if lower.starts_with("array") {
        c[5..].trim_start().strip_prefix('(')?
    } else {
        return None;
    };
    let inner = inner.trim_start();
    if inner.starts_with("$this") {
        return Some(HookCb::Own);
    }
    if inner.starts_with(['\'', '"']) {
        let q = inner.chars().next()?;
        let class = &inner[1..inner[1..].find(q).map(|e| e + 1)?];
        return Some(HookCb::Class(resolve(class)));
    }
    let ident: String = inner.chars().take_while(|ch| ch.is_alphanumeric() || *ch == '_' || *ch == '\\').collect();
    if ident.is_empty() {
        return None;
    }
    if ident.eq_ignore_ascii_case("self") || ident.eq_ignore_ascii_case("static") {
        return Some(declared.first().map(|d| HookCb::Class(vec![d.clone()])).unwrap_or(HookCb::Own));
    }
    Some(HookCb::Class(resolve(&ident)))
}

const NOT_CLASSES: &[&str] = &[
    "self", "static", "parent", "int", "integer", "string", "bool", "boolean", "float", "double", "array", "mixed",
    "void", "null", "callable", "iterable", "object", "never", "false", "true", "extends", "implements", "function",
    "fn", "array_map", "class", "namespace", "use", "new", "if", "while", "for", "foreach", "switch", "match",
];

fn skip_name(name: &str) -> bool {
    NOT_CLASSES.contains(&name.to_ascii_lowercase().as_str())
}

/// Divide `A|B&C`, `A, B` ou `?A` em nomes soltos.
fn split_names(list: &str) -> Vec<String> {
    list.split(|c: char| matches!(c, ',' | '|' | '&' | '?' | '(' | ')') || c.is_whitespace())
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(String::from)
        .collect()
}

fn parse(source: &str) -> Parsed {
    static NS: OnceLock<Regex> = OnceLock::new();
    static DECL: OnceLock<Regex> = OnceLock::new();
    static USE: OnceLock<Regex> = OnceLock::new();
    static NEW: OnceLock<Regex> = OnceLock::new();
    static STATIC: OnceLock<Regex> = OnceLock::new();
    static HIER: OnceLock<Regex> = OnceLock::new();
    static CATCH: OnceLock<Regex> = OnceLock::new();
    static PARAM: OnceLock<Regex> = OnceLock::new();
    static RET: OnceLock<Regex> = OnceLock::new();
    static PROP: OnceLock<Regex> = OnceLock::new();
    static INC: OnceLock<Regex> = OnceLock::new();
    static FUNC: OnceLock<Regex> = OnceLock::new();
    static REGSQ: OnceLock<Regex> = OnceLock::new();
    static REGDQ: OnceLock<Regex> = OnceLock::new();
    static FIRESQ: OnceLock<Regex> = OnceLock::new();
    static FIREDQ: OnceLock<Regex> = OnceLock::new();

    let mut out = Parsed { lines: source.lines().count() as u32, ..Default::default() };
    let (with, code) = clean(&php_only(source));

    let name = r"\\?[A-Za-z_][\w\\]*";
    let types = format!(r"{name}(?:\s*[|&,]\s*{name})*");

    let namespace = re(&NS, r"\bnamespace\s+([A-Za-z_][\w\\]*)\s*[;{]")
        .captures(&code)
        .map(|c| c[1].to_string())
        .unwrap_or_default();
    let qualify = |n: &str| -> String {
        if namespace.is_empty() { n.to_string() } else { format!("{namespace}\\{n}") }
    };

    for c in re(&DECL, r"(?:^|[^:>\w$\\])(?:class|interface|trait|enum)\s+([A-Za-z_]\w*)").captures_iter(&code) {
        let n = &c[1];
        if !skip_name(n) {
            out.declared.push(qualify(n));
        }
    }

    // Imports (`use A\B as C;`, `use A\{B, C};`) valem para este arquivo inteiro.
    let mut imports: HashMap<String, String> = HashMap::new();
    let mut statement_names: Vec<String> = Vec::new();
    for c in re(&USE, r"(?:^|[;{}])\s*use\s+([^;(]+);").captures_iter(&code) {
        let body = c[1].trim();
        let lower = body.to_ascii_lowercase();
        if lower.starts_with("function ") || lower.starts_with("const ") {
            continue;
        }
        let mut items: Vec<String> = Vec::new();
        if let (Some(open), Some(close)) = (body.find('{'), body.rfind('}')) {
            let prefix = body[..open].trim().trim_end_matches('\\');
            for part in body[open + 1..close].split(',') {
                let part = part.trim();
                if !part.is_empty() {
                    items.push(format!("{prefix}\\{part}"));
                }
            }
        } else {
            items.extend(body.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
        }
        for item in items {
            let (full, alias) = match item.to_ascii_lowercase().find(" as ") {
                Some(p) => (item[..p].trim().to_string(), item[p + 4..].trim().to_string()),
                None => {
                    let f = item.trim().to_string();
                    let a = f.rsplit('\\').next().unwrap_or(&f).to_string();
                    (f, a)
                }
            };
            let full = full.trim_start_matches('\\').to_string();
            imports.insert(alias.to_ascii_lowercase(), full.clone());
            statement_names.push(full);
        }
    }

    // Candidatos de nome completo para um nome escrito no código.
    let resolve = |written: &str| -> Vec<String> {
        if written.starts_with('\\') {
            return vec![written.trim_start_matches('\\').to_string()];
        }
        let first = written.split('\\').next().unwrap_or(written);
        if let Some(full) = imports.get(&first.to_ascii_lowercase()) {
            let rest = &written[first.len()..];
            return vec![format!("{full}{rest}")];
        }
        let mut v = Vec::new();
        if !namespace.is_empty() {
            v.push(format!("{namespace}\\{written}"));
        }
        v.push(written.to_string());
        v
    };

    for n in &statement_names {
        // Importar uma classe já a conta como usada; o uso de trait (`use Trait;` na classe) cai aqui.
        out.class_refs.push(vec![n.clone(), qualify(n)]);
    }
    let mut add = |written: &str| {
        let w = written.trim();
        if !w.is_empty() && !skip_name(w) {
            out.class_refs.push(resolve(w));
        }
    };
    for c in re(&NEW, &format!(r"\bnew\s+({name})")).captures_iter(&code) {
        add(&c[1]);
    }
    for c in re(&STATIC, &format!(r"({name})\s*::")).captures_iter(&code) {
        add(&c[1]);
    }
    for c in re(&HIER, &format!(r"\b(?:extends|implements|instanceof)\s+({types})")).captures_iter(&code) {
        for n in split_names(&c[1]) {
            add(&n);
        }
    }
    for c in re(&CATCH, &format!(r"\bcatch\s*\(\s*({types})")).captures_iter(&code) {
        for n in split_names(&c[1]) {
            add(&n);
        }
    }
    for c in re(&PARAM, &format!(r"[(,]\s*(?:(?:public|protected|private|readonly)\s+)*\??({types})\s+(?:&\s*|\.\.\.)?\$")).captures_iter(&code) {
        for n in split_names(&c[1]) {
            add(&n);
        }
    }
    for c in re(&RET, &format!(r"\)\s*:\s*\??({types})")).captures_iter(&code) {
        for n in split_names(&c[1]) {
            add(&n);
        }
    }
    for c in re(&PROP, &format!(r"\b(?:public|protected|private|var)\s+(?:(?:static|readonly)\s+)*\??({types})\s+\$")).captures_iter(&code) {
        for n in split_names(&c[1]) {
            add(&n);
        }
    }

    for c in re(&INC, r"(?:^|[^\w>:$])(?:require_once|require|include_once|include)\b\s*([^;]+);").captures_iter(&with) {
        let expr = strip_parens(c[1].trim());
        out.includes.push(expr.to_string());
    }

    for c in re(&FUNC, r"\bfunction\s+([A-Za-z_]\w*)\s*\(").captures_iter(&code) {
        out.functions.push(c[1].to_string());
    }

    // Hooks do WordPress: nome e callback estão nas strings, então a leitura usa a versão
    // com texto (`with`). Nome com `$` é montado em variável — fica de fora.
    let re_reg_sq = re(&REGSQ, r"\b(?:add_action|add_filter)\s*\(\s*'([^']+)'\s*,\s*");
    let re_reg_dq = re(&REGDQ, r#"\b(?:add_action|add_filter)\s*\(\s*"([^"]+)"\s*,\s*"#);
    for re_hook in [re_reg_sq, re_reg_dq] {
        for m in re_hook.captures_iter(&with) {
            let (name, whole) = (&m[1], m.get(0).unwrap());
            if name.contains('$') {
                continue;
            }
            let chunk: String = with[whole.end()..].chars().take(140).collect();
            if let Some(cb) = parse_callback(&chunk, &resolve, &out.declared) {
                out.hooks_reg.push((name.to_string(), cb));
            }
        }
    }
    let re_fire_sq = re(&FIRESQ, r"\b(?:do_action|apply_filters|do_action_ref_array|apply_filters_ref_array)\s*\(\s*'([^']+)'");
    let re_fire_dq = re(&FIREDQ, r#"\b(?:do_action|apply_filters|do_action_ref_array|apply_filters_ref_array)\s*\(\s*"([^"]+)""#);
    for re_fire in [re_fire_sq, re_fire_dq] {
        for c in re_fire.captures_iter(&with) {
            if !c[1].contains('$') {
                out.hooks_fired.push(c[1].to_string());
            }
        }
    }
    out.plugin_header = source.chars().take(8192).collect::<String>().contains("Plugin Name:");
    out
}

/// `(expr)` vira `expr` quando os parênteses externos se fecham um no outro.
fn strip_parens(s: &str) -> &str {
    let t = s.trim();
    if t.starts_with('(') && t.ends_with(')') {
        let mut depth = 0i32;
        for (i, ch) in t.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 && i != t.len() - 1 {
                        return t;
                    }
                }
                _ => {}
            }
        }
        return t[1..t.len() - 1].trim();
    }
    t
}

/// Divide por `.` fora de aspas e parênteses.
fn split_concat(expr: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut depth = 0;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in expr.chars() {
        if let Some(q) = quote {
            cur.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                cur.push(ch);
            }
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth -= 1;
                cur.push(ch);
            }
            '.' if depth == 0 => {
                parts.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    parts.push(cur.trim().to_string());
    parts
}

fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Calcula o caminho de um `require` quando ele só depende de texto e constantes mágicas.
fn eval_path(expr: &str, file_dir: &Path) -> Option<String> {
    let mut path = String::new();
    for term in split_concat(expr) {
        let t = term.trim();
        if t.len() >= 2 && (t.starts_with('\'') || t.starts_with('"')) && t.ends_with(t.chars().next()?) {
            let inner = &t[1..t.len() - 1];
            if t.starts_with('"') && inner.contains('$') {
                return None;
            }
            path.push_str(inner);
        } else if t == "__DIR__" {
            path.push_str(&file_dir.to_string_lossy());
        } else if t == "__FILE__" {
            return None;
        } else if let Some(args) = t.strip_prefix("dirname(").and_then(|r| r.strip_suffix(')')) {
            let mut it = args.splitn(2, ',');
            let target = it.next()?.trim();
            let levels: usize = match it.next() {
                Some(n) => n.trim().parse().ok()?,
                None => 1,
            };
            let mut base = match target {
                "__DIR__" => file_dir.to_path_buf(),
                "__FILE__" => file_dir.join("x"),
                _ => return None,
            };
            for _ in 0..levels {
                base = base.parent()?.to_path_buf();
            }
            path.push_str(&base.to_string_lossy());
        } else {
            return None;
        }
    }
    if path.is_empty() { None } else { Some(path) }
}

// ------------------------------------------------------------------ varredura

fn collect(root: &Path, dir: &Path, out: &mut Vec<PathBuf>, truncated: &mut bool) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if out.len() >= MAX_FILES {
            *truncated = true;
            return;
        }
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            collect(root, &p, out, truncated);
        } else if ft.is_file() {
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
            if EXTENSIONS.contains(&ext.as_str()) && e.metadata().map(|m| m.len() <= MAX_BYTES).unwrap_or(false) {
                out.push(p);
            }
        }
    }
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

fn composer_files(root: &Path) -> (bool, Vec<String>) {
    let Ok(text) = fs::read_to_string(root.join("composer.json")) else { return (false, Vec::new()) };
    let mut files = Vec::new();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
        for key in ["autoload", "autoload-dev"] {
            if let Some(list) = v.get(key).and_then(|a| a.get("files")).and_then(|f| f.as_array()) {
                files.extend(list.iter().filter_map(|x| x.as_str()).map(|s| s.trim_start_matches("./").to_string()));
            }
        }
    }
    (true, files)
}

/// Lê o projeto em `root`. `docroot` é a pasta pública relativa a ele (vazia = a própria raiz).
/// Pontos de entrada que só existem num projeto WordPress: arquivo principal de plugin
/// (com o cabeçalho `Plugin Name:`), mu-plugins e o `functions.php` de cada tema.
fn wp_entry_reason(r: &str, plugin_header: bool) -> Option<&'static str> {
    if let Some(rest) = r.strip_prefix("wp-content/mu-plugins/") {
        if !rest.contains('/') && rest.ends_with(".php") {
            return Some("mu-plugin do WordPress");
        }
    }
    if let Some(rest) = r.strip_prefix("wp-content/plugins/") {
        if plugin_header && rest.matches('/').count() <= 1 && rest.ends_with(".php") {
            return Some("plugin do WordPress");
        }
    }
    if let Some(rest) = r.strip_prefix("wp-content/themes/") {
        let mut parts = rest.split('/');
        if let (Some(_), Some("functions.php"), None) = (parts.next(), parts.next(), parts.next()) {
            return Some("tema do WordPress");
        }
    }
    None
}

pub fn analyze(root: &Path, docroot: &str) -> CodeMap {
    let mut paths = Vec::new();
    let mut truncated = false;
    collect(root, root, &mut paths, &mut truncated);

    let rels: Vec<String> = paths.iter().map(|p| rel(root, p)).collect();
    let known: HashSet<String> = rels.iter().cloned().collect();
    let known_lower: HashMap<String, String> = rels.iter().map(|r| (r.to_ascii_lowercase(), r.clone())).collect();
    let find_file = |candidate: &str| -> Option<String> {
        known.get(candidate).cloned().or_else(|| known_lower.get(&candidate.to_ascii_lowercase()).cloned())
    };

    let parsed: Vec<Parsed> = paths
        .iter()
        .map(|p| parse(&fs::read(p).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()))
        .collect();

    // Qual arquivo declara cada classe (nomes de classe não diferenciam maiúsculas).
    let mut class_file: HashMap<String, String> = HashMap::new();
    for (i, p) in parsed.iter().enumerate() {
        for c in &p.declared {
            class_file.entry(c.to_ascii_lowercase()).or_insert_with(|| rels[i].clone());
        }
    }

    let mut uses: BTreeMap<String, BTreeSet<(String, &'static str)>> = BTreeMap::new();
    let mut used_by: BTreeMap<String, BTreeSet<(String, &'static str)>> = BTreeMap::new();
    let mut dynamic_per_file: HashMap<String, u32> = HashMap::new();
    let mut total_dynamic = 0u32;
    let mut link = |from: &str, to: &str, via: &'static str| {
        if from == to {
            return;
        }
        uses.entry(from.to_string()).or_default().insert((to.to_string(), via));
        used_by.entry(to.to_string()).or_default().insert((from.to_string(), via));
    };

    for (i, p) in parsed.iter().enumerate() {
        let from = &rels[i];
        let file_dir = paths[i].parent().unwrap_or(root).to_path_buf();
        for expr in &p.includes {
            let resolved = eval_path(expr, &file_dir).and_then(|s| {
                let raw = PathBuf::from(&s);
                let candidates = if raw.is_absolute() {
                    vec![lexical(&raw)]
                } else {
                    vec![lexical(&file_dir.join(&raw)), lexical(&root.join(&raw))]
                };
                candidates.into_iter().find_map(|c| c.strip_prefix(root).ok().and_then(|r| find_file(&r.to_string_lossy().replace('\\', "/"))))
            });
            match resolved {
                Some(to) => link(from, &to, "include"),
                None => {
                    // Só conta como "dinâmico" o que não é um caminho fixo: um arquivo fixo que
                    // não existe é um erro do projeto, não uma limitação da análise.
                    if eval_path(expr, &file_dir).is_none() {
                        *dynamic_per_file.entry(from.clone()).or_default() += 1;
                        total_dynamic += 1;
                    }
                }
            }
        }
        for candidates in &p.class_refs {
            if let Some(to) = candidates.iter().find_map(|c| class_file.get(&c.to_ascii_lowercase())) {
                link(from, to, "classe");
            }
        }
    }

    // Hooks do WordPress. Primeiro resolve o dono de cada callback registrado: função solta
    // (índice de funções), classe (índice de classes) ou o próprio arquivo (closure/$this).
    let mut function_file: HashMap<String, String> = HashMap::new();
    for (i, p) in parsed.iter().enumerate() {
        for f in &p.functions {
            function_file.entry(f.to_ascii_lowercase()).or_insert_with(|| rels[i].clone());
        }
    }
    let mut hook_targets: HashMap<String, Vec<String>> = HashMap::new();
    let mut reg_targets: Vec<Vec<Option<String>>> = Vec::new();
    for (i, p) in parsed.iter().enumerate() {
        let mut mine = Vec::new();
        for (hook, cb) in &p.hooks_reg {
            let target = match cb {
                HookCb::Own => Some(rels[i].clone()),
                HookCb::Func(name) => function_file
                    .get(&name.to_ascii_lowercase())
                    .cloned()
                    .or_else(|| name.rsplit('\\').next().and_then(|last| function_file.get(&last.to_ascii_lowercase()).cloned())),
                HookCb::Class(cands) => cands.iter().find_map(|c| class_file.get(&c.to_ascii_lowercase()).cloned()),
            };
            if let Some(t) = &target {
                hook_targets.entry(hook.clone()).or_default().push(t.clone());
            }
            mine.push(target);
        }
        reg_targets.push(mine);
    }
    // Quem registra aponta para o dono do callback; quem dispara o hook também.
    for (i, p) in parsed.iter().enumerate() {
        let from = rels[i].clone();
        for target in &reg_targets[i] {
            if let Some(to) = target {
                let to = to.clone();
                link(&from, &to, "hook");
            }
        }
        for hook in &p.hooks_fired {
            if let Some(targets) = hook_targets.get(hook) {
                for to in targets.clone() {
                    link(&from, &to, "hook");
                }
            }
        }
    }

    // Pontos de entrada.
    let (has_composer, composer_entries) = composer_files(root);
    let doc = docroot.trim_matches('/').to_string();
    let entry_reason = |r: &str| -> Option<String> {
        if !doc.is_empty() && (r == doc || r.starts_with(&format!("{doc}/"))) {
            return Some("pasta pública".into());
        }
        let file = r.rsplit('/').next().unwrap_or(r);
        if file.eq_ignore_ascii_case("index.php") {
            return Some("index.php".into());
        }
        if doc.is_empty() && !r.contains('/') && r.ends_with(".php") {
            return Some("página na raiz do site".into());
        }
        if composer_entries.iter().any(|f| f == r) {
            return Some("autoload do Composer".into());
        }
        if r.starts_with("bin/") || r.starts_with("tests/") || r.starts_with("test/") {
            return Some(if r.starts_with("bin/") { "script de linha de comando" } else { "testes" }.into());
        }
        None
    };

    let mut reasons: HashMap<&str, String> = HashMap::new();
    for r in &rels {
        if let Some(why) = entry_reason(r) {
            reasons.insert(r.as_str(), why);
        }
    }

    // WordPress: o núcleo carrega mu-plugins, plugins ativos e o functions.php do tema sem
    // nenhum require visível a partir do docroot — sem isto, metade do projeto vira "órfão".
    let is_wp = known.contains("wp-config.php")
        || known.contains("wp-settings.php")
        || rels.iter().any(|r| r.starts_with("wp-includes/"));
    if is_wp {
        for (i, r) in rels.iter().enumerate() {
            if reasons.contains_key(r.as_str()) {
                continue;
            }
            if let Some(why) = wp_entry_reason(r, parsed[i].plugin_header) {
                reasons.insert(r.as_str(), why.to_string());
            }
        }
    }

    // Alcançados a partir das entradas.
    let mut reached: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    for r in reasons.keys() {
        reached.insert(r.to_string());
        queue.push_back(r.to_string());
    }
    while let Some(cur) = queue.pop_front() {
        if let Some(next) = uses.get(&cur) {
            for (to, _) in next {
                if reached.insert(to.clone()) {
                    queue.push_back(to.clone());
                }
            }
        }
    }
    let has_entries = !reasons.is_empty();

    let mut files = Vec::new();
    let (mut orphans, mut unreachable, mut entries, mut total_links) = (0, 0, 0, 0);
    for (i, r) in rels.iter().enumerate() {
        let incoming = used_by.get(r).map(|s| s.len()).unwrap_or(0);
        let kind = if reasons.contains_key(r.as_str()) {
            entries += 1;
            Kind::Entry
        } else if incoming == 0 {
            orphans += 1;
            Kind::Orphan
        } else if has_entries && !reached.contains(r) {
            unreachable += 1;
            Kind::Unreachable
        } else {
            Kind::Normal
        };
        let to_links = |set: Option<&BTreeSet<(String, &'static str)>>| -> Vec<Link> {
            set.map(|s| s.iter().map(|(p, v)| Link { path: p.clone(), via: v.to_string() }).collect()).unwrap_or_default()
        };
        let out_links = to_links(uses.get(r));
        total_links += out_links.len() as u32;
        files.push(FileInfo {
            path: r.clone(),
            kind,
            entry_reason: reasons.get(r.as_str()).cloned(),
            lines: parsed[i].lines,
            classes: parsed[i].declared.clone(),
            hooks: {
                let mut h: Vec<String> = parsed[i].hooks_reg.iter().map(|(n, _)| n.clone()).collect();
                h.sort();
                h.dedup();
                h
            },
            uses: out_links,
            used_by: to_links(used_by.get(r)),
            dynamic_includes: dynamic_per_file.get(r).copied().unwrap_or(0),
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));

    let mut notes = Vec::new();
    if files.is_empty() {
        notes.push("Não encontrei arquivos PHP nesta pasta.".to_string());
    }
    if truncated {
        notes.push(format!("A pasta tem mais de {MAX_FILES} arquivos PHP; só os primeiros foram lidos."));
    }
    if total_dynamic > 0 {
        notes.push(format!(
            "{total_dynamic} require/include usam variáveis e não puderam ser seguidos. Arquivos carregados assim aparecem como órfãos por engano."
        ));
    }
    if !has_entries && !files.is_empty() {
        notes.push("Nenhum ponto de entrada (index.php, pasta pública ou autoload do Composer) foi encontrado.".to_string());
    }
    if is_wp {
        notes.push(
            "Projeto WordPress: segui os hooks (add_action/add_filter) com nome e callback escritos no código, e marquei plugins, mu-plugins e o functions.php do tema como pontos de entrada. Hooks com nome montado em variável continuam invisíveis."
                .to_string(),
        );
    }

    CodeMap { files, total_links, entries, orphans, unreachable, dynamic_includes: total_dynamic, truncated, has_composer, notes }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn project() -> PathBuf {
        let root = std::env::temp_dir().join(format!("localtusk-codemap-{}-{}", std::process::id(), crate::project::now()));
        let _ = fs::remove_dir_all(&root);
        write(&root, "public/index.php", "<?php\nrequire __DIR__ . '/../src/bootstrap.php';\nuse App\\Service\\Mailer;\n$m = new Mailer();\n");
        write(&root, "src/bootstrap.php", "<?php\nnamespace App;\nrequire_once dirname(__DIR__) . '/lib/helpers.php';\n// require 'fantasma.php';\n$x = \"class Frase aqui\";\n");
        write(&root, "lib/helpers.php", "<?php function ajuda() {}\n");
        write(&root, "src/Service/Mailer.php", "<?php\nnamespace App\\Service;\nclass Mailer extends Base implements \\JsonSerializable {\n  public function __construct(private Logger $log) {}\n}\n");
        write(&root, "src/Service/Base.php", "<?php\nnamespace App\\Service;\nabstract class Base {}\n");
        write(&root, "src/Service/Logger.php", "<?php\nnamespace App\\Service;\nclass Logger {}\n");
        write(&root, "src/Sobra.php", "<?php\nnamespace App;\nuse App\\SoPeloSobra;\nclass Sobra { function f() { return SoPeloSobra::x(); } }\n");
        write(&root, "src/SoPeloSobra.php", "<?php\nnamespace App;\nclass SoPeloSobra { static function x() {} }\n");
        write(&root, "src/Dinamico.php", "<?php\n$f = 'a';\nrequire $f . '.php';\n");
        write(&root, "vendor/lib/Ignorado.php", "<?php class Ignorado {}\n");
        root
    }

    fn get<'a>(m: &'a CodeMap, path: &str) -> &'a FileInfo {
        m.files.iter().find(|f| f.path == path).unwrap_or_else(|| panic!("sem {path}"))
    }

    #[test]
    fn segue_require_e_classes_e_acha_orfaos() {
        let root = project();
        let m = analyze(&root, "public");

        assert!(m.files.iter().all(|f| !f.path.starts_with("vendor/")), "vendor fica de fora");
        assert_eq!(get(&m, "public/index.php").kind, Kind::Entry);
        assert_eq!(get(&m, "public/index.php").entry_reason.as_deref(), Some("pasta pública"));

        // require com __DIR__ e dirname(__DIR__)
        let idx = get(&m, "public/index.php");
        assert!(idx.uses.contains(&Link { path: "src/bootstrap.php".into(), via: "include".into() }));
        let boot = get(&m, "src/bootstrap.php");
        assert!(boot.uses.contains(&Link { path: "lib/helpers.php".into(), via: "include".into() }));
        // comentário e texto entre aspas não contam
        assert!(boot.classes.is_empty());
        assert_eq!(boot.uses.len(), 1);

        // classes: use + new, extends, tipo de parâmetro
        assert!(idx.uses.contains(&Link { path: "src/Service/Mailer.php".into(), via: "classe".into() }));
        let mailer = get(&m, "src/Service/Mailer.php");
        let alvos: Vec<_> = mailer.uses.iter().map(|l| l.path.as_str()).collect();
        assert!(alvos.contains(&"src/Service/Base.php") && alvos.contains(&"src/Service/Logger.php"), "{alvos:?}");
        assert_eq!(get(&m, "src/Service/Base.php").used_by[0].path, "src/Service/Mailer.php");

        // órfão e "só chamado por órfão"
        assert_eq!(get(&m, "src/Sobra.php").kind, Kind::Orphan);
        assert_eq!(get(&m, "src/SoPeloSobra.php").kind, Kind::Unreachable);
        assert_eq!(get(&m, "src/Service/Logger.php").kind, Kind::Normal);

        // include dinâmico é contado, não vira ligação
        assert_eq!(m.dynamic_includes, 1);
        assert_eq!(get(&m, "src/Dinamico.php").dynamic_includes, 1);
        assert!(m.notes.iter().any(|n| n.contains("variáveis")));
        assert_eq!((m.orphans, m.unreachable), (2, 1));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn sem_pasta_publica_paginas_da_raiz_sao_entradas() {
        let root = std::env::temp_dir().join(format!("localtusk-codemap-raiz-{}-{}", std::process::id(), crate::project::now()));
        let _ = fs::remove_dir_all(&root);
        write(&root, "contato.php", "<html><?php include 'includes/topo.php'; ?></html>");
        write(&root, "includes/topo.php", "<?php echo 1;");
        write(&root, "includes/velho.php", "<?php echo 2;");
        write(&root, "composer.json", r#"{"autoload":{"files":["includes/velho.php"]}}"#);
        let m = analyze(&root, "");
        assert!(m.has_composer);
        assert_eq!(get(&m, "contato.php").entry_reason.as_deref(), Some("página na raiz do site"));
        assert_eq!(get(&m, "includes/topo.php").kind, Kind::Normal);
        // listado no autoload do Composer, então é entrada
        assert_eq!(get(&m, "includes/velho.php").entry_reason.as_deref(), Some("autoload do Composer"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_expressoes_de_caminho() {
        let dir = Path::new("/p/src/a");
        assert_eq!(eval_path("'x.php'", dir).as_deref(), Some("x.php"));
        assert_eq!(eval_path("__DIR__ . '/x.php'", dir).as_deref(), Some("/p/src/a/x.php"));
        assert_eq!(eval_path("dirname(__DIR__, 2) . \"/y.php\"", dir).as_deref(), Some("/p/y.php"));
        assert_eq!(eval_path("dirname(__FILE__) . '/z.php'", dir).as_deref(), Some("/p/src/a/z.php"));
        assert_eq!(eval_path("$base . '/x.php'", dir), None);
        assert_eq!(eval_path("\"$dir/x.php\"", dir), None);
        assert_eq!(strip_parens("(__DIR__ . '/a.php')"), "__DIR__ . '/a.php'");
        assert_eq!(strip_parens("(a) . (b)"), "(a) . (b)");
    }

    #[test]
    fn hierarquia_com_varios_nomes_e_grupos_de_use() {
        let p = parse("<?php\nnamespace A;\nuse B\\{C, D as E};\nclass X extends Y implements I, J { use T; }\n");
        let refs: Vec<String> = p.class_refs.iter().flatten().cloned().collect();
        for esperado in ["B\\C", "B\\D", "A\\Y", "A\\I", "A\\J", "A\\T"] {
            assert!(refs.iter().any(|r| r == esperado), "faltou {esperado} em {refs:?}");
        }
        assert_eq!(p.declared, vec!["A\\X".to_string()]);
    }

    #[test]
    fn wordpress_hooks_e_entradas_de_plugin_e_tema() {
        let root = std::env::temp_dir().join(format!("localtusk-codemap-wp-{}-{}", std::process::id(), crate::project::now()));
        let _ = fs::remove_dir_all(&root);
        write(&root, "wp-config.php", "<?php define('ABSPATH', __DIR__ . '/');\n");
        write(&root, "wp-includes/plugin.php", "<?php do_action('init');\napply_filters('the_content', $c);\n");
        write(
            &root,
            "wp-content/plugins/loja/loja.php",
            "<?php\n/*\nPlugin Name: Loja\n*/\nadd_action('init', 'loja_init');\nadd_filter('the_content', ['Loja\\Filtros', 'embelezar']);\nadd_action('loja_pronto', function () {});\n",
        );
        write(&root, "wp-content/plugins/loja/includes/boot.php", "<?php function loja_init() {}\n");
        write(&root, "wp-content/plugins/loja/includes/Filtros.php", "<?php\nnamespace Loja;\nclass Filtros { public static function embelezar($c) {} }\n");
        write(&root, "wp-content/plugins/loja/includes/sem-uso.php", "<?php echo 9;\n");
        write(&root, "wp-content/mu-plugins/cedo.php", "<?php // roda sempre\n");
        write(&root, "wp-content/themes/tema/functions.php", "<?php add_action('wp_enqueue_scripts', ['Assets', 'registrar']);\n");
        write(&root, "wp-content/themes/tema/inc/Assets.php", "<?php class Assets { public static function registrar() {} }\n");

        let m = analyze(&root, "");

        // Entradas do WordPress.
        let loja = get(&m, "wp-content/plugins/loja/loja.php");
        assert_eq!(loja.kind, Kind::Entry);
        assert_eq!(loja.entry_reason.as_deref(), Some("plugin do WordPress"));
        assert_eq!(loja.hooks, vec!["init".to_string(), "loja_pronto".to_string(), "the_content".to_string()]);
        assert_eq!(get(&m, "wp-content/mu-plugins/cedo.php").entry_reason.as_deref(), Some("mu-plugin do WordPress"));
        assert_eq!(get(&m, "wp-content/themes/tema/functions.php").entry_reason.as_deref(), Some("tema do WordPress"));

        // Callback em texto liga o plugin ao arquivo que declara a função.
        let boot = get(&m, "wp-content/plugins/loja/includes/boot.php");
        assert_eq!(boot.kind, Kind::Normal);
        assert!(boot.used_by.contains(&Link { path: "wp-content/plugins/loja/loja.php".into(), via: "hook".into() }), "{:?}", boot.used_by);
        // Quem dispara o hook também liga ao dono do callback.
        assert!(boot.used_by.contains(&Link { path: "wp-includes/plugin.php".into(), via: "hook".into() }), "{:?}", boot.used_by);

        // Callback em array com classe.
        let filtros = get(&m, "wp-content/plugins/loja/includes/Filtros.php");
        assert_eq!(filtros.kind, Kind::Normal);
        assert!(filtros.used_by.contains(&Link { path: "wp-content/plugins/loja/loja.php".into(), via: "hook".into() }));
        let assets = get(&m, "wp-content/themes/tema/inc/Assets.php");
        assert_eq!(assets.kind, Kind::Normal);
        assert!(assets.used_by.contains(&Link { path: "wp-content/themes/tema/functions.php".into(), via: "hook".into() }));

        // Arquivo de verdade sem uso continua órfão, e a nota explica o que foi seguido.
        assert_eq!(get(&m, "wp-content/plugins/loja/includes/sem-uso.php").kind, Kind::Orphan);
        assert!(m.notes.iter().any(|n| n.contains("WordPress")), "{:?}", m.notes);
        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod real {
    /// Roda no projeto indicado em `LOCALTUSK_MAP_DIR` (e `LOCALTUSK_MAP_DOCROOT`) e imprime um resumo.
    /// `LOCALTUSK_MAP_DIR=/caminho cargo test --lib codemap::real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn projeto_real() {
        let Ok(dir) = std::env::var("LOCALTUSK_MAP_DIR") else { return };
        let docroot = std::env::var("LOCALTUSK_MAP_DOCROOT").unwrap_or_default();
        let t = std::time::Instant::now();
        let m = super::analyze(std::path::Path::new(&dir), &docroot);
        println!("{} arquivos, {} ligações, {} entradas, {} órfãos, {} inalcançáveis, {} dinâmicos, em {:?}", m.files.len(), m.total_links, m.entries, m.orphans, m.unreachable, m.dynamic_includes, t.elapsed());
        for n in &m.notes { println!("nota: {n}"); }
        let mut orfaos: Vec<_> = m.files.iter().filter(|f| matches!(f.kind, super::Kind::Orphan)).take(15).collect();
        orfaos.sort_by(|a, b| a.path.cmp(&b.path));
        for f in orfaos { println!("órfão: {}", f.path); }
        let mut top: Vec<_> = m.files.iter().collect();
        top.sort_by_key(|f| std::cmp::Reverse(f.used_by.len()));
        for f in top.iter().take(5) { println!("mais usado: {} ({} usos)", f.path, f.used_by.len()); }
    }
}
