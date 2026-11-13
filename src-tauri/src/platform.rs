//! Nomes de arquivos que mudam com o sistema operacional.
//!
//! As versões gerenciadas de PHP/MySQL/PostgreSQL guardam o binário em disco e os outros
//! módulos olham para lá: sem algo como `php-fpm.exe` no Windows, um PHP instalado no
//! Windows seria invisível para o app. Tudo que precisa de um nome de binário passa por aqui.

use std::path::{Path, PathBuf};

/// Sufixo dos executáveis: `.exe` no Windows, vazio nos outros sistemas.
pub const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

/// Nome de um executável com o sufixo do sistema (`name("mysqld")` -> `mysqld.exe` no Windows).
pub fn name(bin: &str) -> String {
    format!("{bin}{EXE}")
}

/// `dir/x.exe` no Windows, `dir/x` nos outros sistemas.
pub fn bin(dir: &Path, bin: &str) -> PathBuf {
    dir.join(name(bin))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nome_combina_com_o_binario() {
        if cfg!(windows) {
            assert_eq!(name("mysqld"), "mysqld.exe");
        } else {
            assert_eq!(name("mysqld"), "mysqld");
        }
    }
}
