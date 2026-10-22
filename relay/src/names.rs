//! Nomes dos links (`https://<servidor>/<nome>`), guardados em `<dados>/names.json`.
//!
//! Cada nome pertence a um "dono" (o resumo da chave secreta de um CapturaX): só aquele app
//! transmite com ele. Pode ser um apelido escolhido (`carlos`) ou um código gerado (10 caracteres).
//! Cada dono tem um nome por vez. Nome sem uso há `EXPIRE_SECS` fica livre de novo.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 7 dias sem transmitir (nem abrir a janela de transmissão) libera o nome.
const EXPIRE_SECS: u64 = 7 * 24 * 3600;
const AUTO_LENGTH: usize = 10;
/// Caminhos do próprio servidor/página que não podem virar nome.
const RESERVED: &[&str] = &["api", "signal", "assets", "admin", "health", "index", "static", "www", "v", "live", "favicon", "robots"];

#[derive(Serialize, Deserialize, Clone)]
struct Entry {
    owner: String,
    used: u64,
    auto: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ClaimError {
    Taken,
    Invalid,
    Io,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Nome que pode aparecer num link (apelido ou código).
pub fn valid_name(name: &str) -> bool {
    (3..=20).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !RESERVED.contains(&name)
}

fn random_code() -> String {
    const ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
    let mut buffer = [0u8; AUTO_LENGTH];
    getrandom::fill(&mut buffer).expect("random");
    buffer.iter().map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char).collect()
}

pub struct Names {
    path: PathBuf,
    lock: Mutex<()>,
}

impl Names {
    pub fn new(data_dir: &Path) -> Self {
        Self { path: data_dir.join("names.json"), lock: Mutex::new(()) }
    }

    fn load(&self) -> HashMap<String, Entry> {
        let mut names: HashMap<String, Entry> = std::fs::read_to_string(&self.path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let limit = now().saturating_sub(EXPIRE_SECS);
        names.retain(|_, entry| entry.used >= limit);
        names
    }

    fn save(&self, names: &HashMap<String, Entry>) -> Result<(), ClaimError> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|_| ClaimError::Io)?;
        }
        let temp = self.path.with_extension("tmp");
        std::fs::write(&temp, serde_json::to_string(names).unwrap()).map_err(|_| ClaimError::Io)?;
        std::fs::rename(temp, &self.path).map_err(|_| ClaimError::Io)
    }

    /// Reserva (ou renova) o nome do dono. `nickname` vazio/`None` = código automático.
    /// Trocar de nome libera o anterior.
    pub fn claim(&self, owner: &str, nickname: Option<&str>) -> Result<String, ClaimError> {
        let _guard = self.lock.lock().unwrap();
        let mut names = self.load();
        let nickname = nickname.map(|n| n.trim().to_lowercase()).filter(|n| !n.is_empty());
        let name = match nickname {
            Some(nickname) => {
                if !valid_name(&nickname) {
                    return Err(ClaimError::Invalid);
                }
                if names.get(&nickname).is_some_and(|entry| entry.owner != owner) {
                    return Err(ClaimError::Taken);
                }
                names.retain(|name, entry| entry.owner != owner || *name == nickname);
                names.insert(nickname.clone(), Entry { owner: owner.to_string(), used: now(), auto: false });
                nickname
            }
            None => {
                names.retain(|_, entry| entry.owner != owner || entry.auto);
                let existing = names.iter().find(|(_, entry)| entry.owner == owner).map(|(name, _)| name.clone());
                let name = existing.unwrap_or_else(|| loop {
                    let code = random_code();
                    if !names.contains_key(&code) {
                        break code;
                    }
                });
                names.insert(name.clone(), Entry { owner: owner.to_string(), used: now(), auto: true });
                name
            }
        };
        self.save(&names)?;
        Ok(name)
    }

    /// Renova o prazo (ao terminar uma transmissão).
    pub fn touch(&self, name: &str) {
        let _guard = self.lock.lock().unwrap();
        let mut names = self.load();
        if let Some(entry) = names.get_mut(name) {
            entry.used = now();
            let _ = self.save(&names);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_rules() {
        let dir = std::env::temp_dir().join(format!("capturax-names-test-{}", random_code()));
        let names = Names::new(&dir);
        assert_eq!(names.claim("dono-a", Some("Carlos")).unwrap(), "carlos");
        assert_eq!(names.claim("dono-b", Some("carlos")), Err(ClaimError::Taken));
        assert_eq!(names.claim("dono-b", Some("api")), Err(ClaimError::Invalid));
        assert_eq!(names.claim("dono-b", Some("a b")), Err(ClaimError::Invalid));
        assert_eq!(names.claim("dono-b", Some("-x-")), Err(ClaimError::Invalid));
        // Código automático: estável para o mesmo dono.
        let code = names.claim("dono-b", None).unwrap();
        assert_eq!(code.len(), AUTO_LENGTH);
        assert_eq!(names.claim("dono-b", None).unwrap(), code);
        // Trocar de apelido libera o anterior.
        assert_eq!(names.claim("dono-a", Some("carlinhos")).unwrap(), "carlinhos");
        assert_eq!(names.claim("dono-b", Some("carlos")).unwrap(), "carlos");
        let _ = std::fs::remove_dir_all(dir);
    }
}
