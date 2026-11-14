# 🐘 LocalTusk

Ambientes de desenvolvimento PHP locais, **sem Docker**, com a facilidade do LocalWP — para qualquer projeto PHP.
Gratuito e open source (MIT). Funciona no **macOS** (Apple Silicon e Intel) e no **Linux** (beta, x86_64 e ARM64).

Cada projeto roda com processos nativos isolados: **Caddy** (web) + **PHP-FPM** + **MySQL** ou **PostgreSQL** (e **Redis**, se você ligar), cada um com sua porta e seus dados em `~/LocalTusk/projects/<projeto>/`.

> **Veio do LocalPHP?** O app mudou de nome na v0.6.0. Na primeira abertura, a pasta `~/LocalPHP` é renomeada sozinha para `~/LocalTusk`, com projetos, bancos, backups e blueprints intactos.

## Estado atual (MVP)

- [x] Criar, iniciar, parar e apagar projetos
- [x] Porta HTTP e porta do banco automáticas, sem conflito entre projetos
- [x] Escolha da versão do PHP por projeto (entre as instaladas)
- [x] MySQL isolado por projeto (`root` / `root`) e variáveis `DB_*` no PHP
- [x] **PostgreSQL** como alternativa ao MySQL, escolhido por projeto (14 a 18, em `~/LocalTusk/bin/postgres/`); mesmas variáveis `DB_*` (mais `DB_ENGINE`/`DB_CONNECTION`), Adminer Neo, backup, blueprints e `psql` no terminal. O PHP com `pdo_pgsql` é baixado sozinho na primeira vez
- [x] **Redis** por projeto (interruptor), servido pelo **Valkey**: o app baixa o código-fonte e o compila (precisa das ferramentas de linha de comando do Xcode no macOS, ou de `make` e `gcc` no Linux); variáveis `REDIS_*` no PHP e `redis-cli` no terminal; escuta só em `127.0.0.1`
- [x] Instalar qualquer versão do PHP 8.x pelo app (binários estáticos, em `~/LocalTusk/bin/php/`)
- [x] Instalar e escolher a versão do MySQL (8.0, 8.4 LTS, 9.x) pelo app, em `~/LocalTusk/bin/mysql/`
- [x] Caddy baixado pelo app, com verificação de checksum
- [x] WP-CLI gerenciado (checksum SHA-512) e PHP CLI baixado junto com cada versão do PHP
- [x] Botão **Instalar WordPress**: baixa, cria o `wp-config.php` com o banco do projeto e instala (idioma, usuário e senha à sua escolha)
- [x] **Abrir terminal** do projeto, com `php`, `wp`, `mysql`/`psql` e `redis-cli` já apontando para o PHP e o banco dele (macOS e Linux)
- [x] Adminer Neo com login automático no banco do projeto
- [x] Mailpit: `mail()` e SMTP do PHP caem numa caixa de e-mails de teste por projeto
- [x] Logs ao vivo (acesso, erros do PHP, MySQL/PostgreSQL, Redis, Mailpit, Caddy)
- [x] Domínio por projeto: `<nome-do-projeto>.localhost:<porta>` (funciona sem configurar nada nem pedir senha)
- [x] **HTTPS** por projeto com um interruptor: o Caddy cria uma autoridade local e o app a instala como confiável no macOS (pede a senha uma vez; no Linux ele mostra onde está o certificado para você instalar); o site abre em `https://<nome>.localhost:<porta>` com cadeado
- [x] **Acessível na rede**: ligue para abrir o site em outros aparelhos da mesma rede (IP e nome `.local`); só o site é exposto, banco, Adminer Neo e Mailpit continuam locais. Para HTTPS sem aviso em outro aparelho, instale nele o certificado raiz (o app mostra o arquivo)
- [ ] Domínios `.test` sem porta (opcional, exige configuração única com senha de administrador)
- [ ] Git integrado
- [x] Download do MySQL e do PostgreSQL no macOS e no Linux (no Windows ainda não)
- [x] Backup e restauração de projetos (arquivos + banco) em `~/LocalTusk/backups`
- [x] **Blueprints**: salve um projeto (arquivos, banco e versões de PHP/MySQL) como modelo em `~/LocalTusk/blueprints` e crie novos projetos já prontos a partir dele; dá para compartilhar o blueprint como arquivo e importar o de outra pessoa
- [x] **Importar do Local**: lê os sites do [Local](https://localwp.com) (Linux e macOS; só leitura, nada na pasta dele é alterado) e cria um projeto do LocalTusk com os arquivos de `app/public` (links simbólicos viram cópias) e o banco salvo em `app/sql/local.sql`, com `siteurl`/`home` trocados para o endereço novo. O site precisa estar parado no Local; se faltar a versão do PHP ou do MySQL, o app oferece instalar ou usar a mais próxima
- [x] Aba **API**: inspetor que mostra cada requisição recebida pelo projeto (cabeçalhos, corpo, formulário/arquivos, resposta, tempo; copiar como cURL e reenviar) e um cliente de API (métodos, cabeçalhos, corpo JSON/form/texto, redirecionamentos, histórico) com **coleção** de requisições salvas por projeto (`collection.json`), exportável e importável (também dá para salvar direto do inspetor), com layout que se adapta à largura da janela
- [x] Aba **Mapa**: lê os arquivos PHP do projeto (sem executar nada), segue `require`/`include` e classes ligadas por `use`, `new`, `extends` e afins, e mostra quem chama quem em **lista** e em **grafo** (React Flow), com os arquivos que nenhum outro usa marcados como possíveis órfãos. A leitura é estática: não enxerga hooks do WordPress, rotas nem `require` com variável, então o resultado é uma pista, não uma ordem para apagar
- [x] CI no GitHub Actions (`.github/workflows/ci.yml`): compila e testa em macOS, Linux e Windows
- [x] **Linux (beta)**: PHP, MySQL, PostgreSQL, Redis, WordPress, HTTPS e rede passam nos testes ponta a ponta no Ubuntu 24.04 ARM64; o x86_64 e a interface gráfica ainda têm pouco uso real
- [x] Instaladores publicados pelo GitHub Actions (`.github/workflows/release.yml`): `.dmg` (Apple Silicon e Intel), `.AppImage`, `.deb` e `.rpm` (x86_64 e ARM64)
- [ ] Windows (o código compila, mas o app ainda não baixa o MySQL nem o PostgreSQL lá)

## Pré-requisitos

**macOS:** nenhum. **Linux:** `libaio` e `libnuma` (MySQL), `make` e `gcc` (Redis, que é compilado) e o WebKitGTK 4.1 para a janela do app (`libwebkit2gtk-4.1`). No Ubuntu: `sudo apt install libaio1t64 libnuma1 build-essential libwebkit2gtk-4.1-0`.

PHP, MySQL, Caddy, Adminer Neo e Mailpit são baixados pelo próprio app
(tela "PHP e ferramentas"), sem Homebrew, apt e Docker. Se você já tiver `caddy`, `mysql` ou `php`
do Homebrew, o app os reconhece como alternativa (marcados como "sistema").

Observações sobre o PostgreSQL e o Redis:

- O PostgreSQL vem de [theseus-rs/postgresql-binaries](https://github.com/theseus-rs/postgresql-binaries); o banco de um projeto também não troca de tipo nem de versão depois de criado. O catálogo fica em `src-tauri/resources/postgres-versions.json` (`python3 scripts/update_postgres_versions.py` para atualizar).
- O WordPress só funciona em projetos MySQL. Backups e blueprints guardam o tipo do banco, e restaurar em outro tipo é recusado.
- O Valkey é compilado na instalação (cerca de 15 segundos); sem o Xcode CLT (`xcode-select --install`) no macOS, ou sem `make` e `gcc` no Linux, o app avisa.
- No Linux ARM64, o MySQL só tem o pacote completo (cerca de 800 MB instalado); no x86_64, a partir do 8.4.5, há um pacote mínimo de cerca de 80 MB.

Observações sobre o MySQL:

- O banco de um projeto é criado na primeira inicialização e **não pode trocar de versão** depois
  (o MySQL não faz downgrade). Para usar outra versão, crie um novo projeto.
- O catálogo de versões vem de `src-tauri/resources/mysql-versions.json`, embutido no app. Para
  atualizá-lo quando saírem novas versões: `python3 scripts/update_mysql_versions.py`
  (faz poucas requisições a `cdn.mysql.com`, com pausas, para não ser bloqueado).

Para desenvolver: [Rust](https://rustup.rs) e [pnpm](https://pnpm.io).

## Instaladores e releases

O workflow `release.yml` compila e anexa os instaladores a uma release do GitHub quando se envia uma tag `vX.Y.Z` igual à versão do `package.json`, do `tauri.conf.json` e do `Cargo.toml`. O app do macOS é assinado só de forma ad-hoc (sem conta de desenvolvedor da Apple): no primeiro uso, abra com clique direito → Abrir. O site (`site/`) lê a última release e mostra o arquivo certo para o sistema e a arquitetura do visitante.

## Desenvolvimento

```bash
pnpm install
pnpm tauri dev
```

Para ajustar só a interface no navegador (sem o Tauri), abra <http://localhost:1420> com o `pnpm dev`
rodando: em desenvolvimento, `src/dev-mock.ts` simula o backend com projetos de exemplo.

Testes ponta a ponta do motor (sobem Caddy + PHP-FPM + MySQL de verdade e baixam as ferramentas, uma vez, para `/tmp/localtusk-test-tools`; funcionam igual no macOS e no Linux):

```bash
cd src-tauri
cargo test --lib start_serve_stop -- --ignored --nocapture
cargo test --lib install_and_serve -- --ignored --nocapture
cargo test --lib extras_end_to_end -- --ignored --nocapture
cargo test --lib managed_stack -- --ignored --nocapture   # Caddy + MySQL + PHP, tudo baixado
cargo test --lib postgres_redis_end_to_end -- --ignored --nocapture   # PostgreSQL + Valkey + PHP (baixa ~250 MB e compila o Valkey; cache em /tmp/localtusk-test-pgredis)
cargo test --lib wordpress_end_to_end -- --ignored --nocapture   # WP-CLI + WordPress de verdade (cache em /tmp/localtusk-test-wp)
cargo test --lib local_folder_restore_end_to_end -- --ignored --nocapture   # importar de uma pasta "do Local" (reaproveita o cache de /tmp/localtusk-test-wp)
```

## Estrutura

```
src/             interface (React + TypeScript)
src-tauri/src/
  tools.rs       descobre PHP-FPM, Caddy e MySQL
  mysql_manager.rs catálogo e instalação de versões do MySQL
  postgres_manager.rs catálogo e instalação do PostgreSQL
  valkey_manager.rs   baixa e compila o Valkey (Redis)
  download.rs    download com progresso
  php_manager.rs catálogo e instalação de versões do PHP
  extras.rs      Caddy, Adminer Neo, Mailpit e WP-CLI
  wp.rs          PHP CLI, WP-CLI, terminal do projeto e instalação do WordPress
  project.rs     modelo do projeto, portas, persistência
  runtime.rs     gera configs e controla os processos
  commands.rs    comandos expostos à interface
prototipo/       protótipo original em shell + Docker (referência)
```

## Apoie

O LocalTusk é gratuito e continua completo para todo mundo. Se ele ajuda no seu trabalho, você pode apoiar o projeto com um Pix pelo [LivePix](https://livepix.gg/jhowmac).

## Licença

[MIT](LICENSE)
