//! Nomes de arquivos que mudam com o sistema operacional.
//!
//! As versões gerenciadas de PHP/MySQL/PostgreSQL guardam o binário em disco e os outros
//! módulos olham para lá: sem algo como `php-fpm.exe` no Windows, um PHP instalado no
//! Windows seria invisível para o app. Tudo que precisa de um nome de binário passa por aqui.

use std::path::{Path, PathBuf};

/// Sufixo dos executáveis: `.exe` no Windows, vazio nos outros sistemas.
pub const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

/// `zip` no Windows (zip-it, sem ler tar), `tar.gz`/`tar.xz` nos outros sistemas.
pub const UNIX_ONLY_MSG: &str = "Esta parte só funciona no macOS e no Linux por enquanto.";

/// Nome de um executável com o sufixo do sistema (`nam("mysqld")` -> `mysqld.exe` no Windows).
pub fn name(bin: &str) -> String {
    format!("{bin}{EXE}")
}

/// `dir/x.exe` no Windows, `dir/x` nos outros sistemas.
pub fn bin(dir: &Path, bin: &str) -> PathBuf {
    dir.join(name(bin))
}

/// `true` para `mysqld.exe` (e para `mysqld` nos outros sistemas).
pub fn stem(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

/// Versão encontrada numa listagem de binários: `catalog` só é devolvida quando
/// o download funciona nesta máquina (hoje: macOS e Linux; Windows fica de fora até
/// existir um pacote de cada ferramenta).
pub fn platform_supported() -> bool {
    !cfg!(windows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nome_combina_com_o_binario() {
        if cfg!(windows) {
            assert_eq!(name("mysqld"), "mysqld.exe");
            assert_eq!(stem(&bin(Path::new("C:\\x"), "mysqld")), "mysqld");
        } else {
            assert_eq!(name("mysqld"), "mysqld");
            assert_eq!(stem(&bin(Path::new("/x"), "mysqld")), "mysqld");
        }
    }
}
