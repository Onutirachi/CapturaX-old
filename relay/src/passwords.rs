//! Senhas do servidor, guardadas como hash com salt em `<dados>/passwords.json`.
//!
//! - **admin**: do dono do servidor. Transmite e troca a senha de uso.
//! - **uso**: o dono passa para quem pode transmitir por este servidor. Trocar corta todo mundo
//!   que entrou com a antiga. Vazia = só o admin transmite.
//!
//! O arquivo é lido a cada verificação, então `reset-admin` (outro processo) vale na hora.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Rodadas do hash: deixa adivinhar por força bruta lento, sem pesar num login normal.
const ROUNDS: u32 = 100_000;

#[derive(Serialize, Deserialize, Default, Clone)]
struct Stored {
    admin: Option<String>,
    usage: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Admin,
    Usage,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Usage => "usage",
        }
    }
}

fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).expect("random");
    buffer.iter().map(|b| format!("{b:02x}")).collect()
}

/// Senha legível para mostrar no terminal (sem caracteres parecidos como 0/O, 1/l).
pub fn generate_password() -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyzABCDEFGHJKMNPQRSTUVWXYZ23456789";
    let mut buffer = [0u8; 20];
    getrandom::fill(&mut buffer).expect("random");
    buffer.iter().map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char).collect()
}

fn digest(salt: &str, password: &str) -> String {
    let mut value = Sha256::digest(format!("{salt}:{password}").as_bytes()).to_vec();
    for _ in 1..ROUNDS {
        value = Sha256::digest(&value).to_vec();
    }
    value.iter().map(|b| format!("{b:02x}")).collect()
}

fn hash(password: &str) -> String {
    let salt = random_hex(16);
    format!("{salt}${}", digest(&salt, password))
}

/// Compara sem vazar pelo tempo quantos caracteres batem.
fn verify(stored: &str, password: &str) -> bool {
    let Some((salt, expected)) = stored.split_once('$') else { return false };
    let actual = digest(salt, password);
    actual.len() == expected.len() && actual.bytes().zip(expected.bytes()).fold(0u8, |diff, (a, b)| diff | (a ^ b)) == 0
}

pub struct Passwords {
    path: PathBuf,
}

impl Passwords {
    pub fn new(data_dir: &Path) -> Self {
        Self { path: data_dir.join("passwords.json") }
    }

    fn load(&self) -> Stored {
        std::fs::read_to_string(&self.path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    fn save(&self, stored: &Stored) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temp = self.path.with_extension("tmp");
        std::fs::write(&temp, serde_json::to_string_pretty(stored).unwrap())?;
        std::fs::rename(temp, &self.path)
    }

    /// Na primeira vez, cria a senha de admin (a do ambiente, ou uma aleatória) e a devolve para
    /// ser mostrada no log. `None` se já existia.
    pub fn ensure_admin(&self, from_env: Option<String>) -> std::io::Result<Option<String>> {
        let mut stored = self.load();
        if stored.admin.is_some() {
            return Ok(None);
        }
        let password = from_env.filter(|p| !p.is_empty()).unwrap_or_else(generate_password);
        stored.admin = Some(hash(&password));
        self.save(&stored)?;
        Ok(Some(password))
    }

    /// Nova senha de admin (perdeu a antiga). Devolve a senha para mostrar no terminal.
    pub fn reset_admin(&self) -> std::io::Result<String> {
        let mut stored = self.load();
        let password = generate_password();
        stored.admin = Some(hash(&password));
        self.save(&stored)?;
        Ok(password)
    }

    /// Vazia = ninguém além do admin transmite.
    pub fn set_usage(&self, password: &str) -> std::io::Result<()> {
        let mut stored = self.load();
        stored.usage = (!password.is_empty()).then(|| hash(password));
        self.save(&stored)
    }

    pub fn usage_is_set(&self) -> bool {
        self.load().usage.is_some()
    }

    pub fn role(&self, password: &str) -> Option<Role> {
        if password.is_empty() {
            return None;
        }
        let stored = self.load();
        if stored.admin.as_deref().is_some_and(|hash| verify(hash, password)) {
            return Some(Role::Admin);
        }
        if stored.usage.as_deref().is_some_and(|hash| verify(hash, password)) {
            return Some(Role::Usage);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_and_reset() {
        let dir = std::env::temp_dir().join(format!("capturax-signal-test-{}", random_hex(4)));
        let passwords = Passwords::new(&dir);
        let admin = passwords.ensure_admin(Some("admin-123".into())).unwrap().unwrap();
        assert_eq!(admin, "admin-123");
        assert!(passwords.ensure_admin(None).unwrap().is_none());
        assert_eq!(passwords.role("admin-123"), Some(Role::Admin));
        assert_eq!(passwords.role("errada"), None);
        assert_eq!(passwords.role(""), None);
        passwords.set_usage("uso-456").unwrap();
        assert_eq!(passwords.role("uso-456"), Some(Role::Usage));
        passwords.set_usage("").unwrap();
        assert_eq!(passwords.role("uso-456"), None);
        let new_admin = passwords.reset_admin().unwrap();
        assert_eq!(passwords.role("admin-123"), None);
        assert_eq!(passwords.role(&new_admin), Some(Role::Admin));
        let _ = std::fs::remove_dir_all(dir);
    }
}
