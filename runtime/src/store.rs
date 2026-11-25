//! On-disk layout under ~/.deskhatch and small JSON stores.
//!
//! ~/.deskhatch/
//!   apps/<id>/            unpacked app packages
//!   data/<id>/store.json  per-app key-value store (hatch.store)
//!   grants/<id>.json      folders the user granted to the app
//!   config.json           runtime config (default LLM provider etc.)

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub fn root() -> PathBuf {
    if let Ok(p) = std::env::var("DESKHATCH_HOME") {
        return PathBuf::from(p);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deskhatch")
}

pub fn apps_dir() -> PathBuf {
    root().join("apps")
}
pub fn app_dir(id: &str) -> PathBuf {
    apps_dir().join(id)
}
pub fn data_dir(id: &str) -> PathBuf {
    root().join("data").join(id)
}
pub fn grants_file(id: &str) -> PathBuf {
    root().join("grants").join(format!("{id}.json"))
}
pub fn config_file() -> PathBuf {
    root().join("config.json")
}
#[allow(dead_code)]
pub fn runtime_dir() -> PathBuf {
    root().join("runtime")
}

pub fn ensure_dirs() -> std::io::Result<()> {
    for d in [apps_dir(), root().join("data"), root().join("grants")] {
        fs::create_dir_all(d)?;
    }
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(p: &Path) -> T {
    fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_json<T: Serialize>(p: &Path, v: &T) -> std::io::Result<()> {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = p.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(v)?)?;
    fs::rename(tmp, p)
}

// ---------- runtime config ----------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Default provider when an app does not specify one: "ollama" or "openai" or custom.
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Per-provider endpoints/models, e.g. {"openai": {"endpoint": "...", "model": "..."}}
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

pub fn load_config() -> Config {
    read_json(&config_file())
}
pub fn save_config(c: &Config) -> std::io::Result<()> {
    write_json(&config_file(), c)
}

// ---------- folder grants ----------

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Grants {
    pub folders: Vec<PathBuf>,
}

pub fn load_grants(id: &str) -> Grants {
    read_json(&grants_file(id))
}
pub fn add_grant(id: &str, folder: &Path) -> std::io::Result<()> {
    let mut g = load_grants(id);
    let folder = folder.canonicalize().unwrap_or_else(|_| folder.to_path_buf());
    if !g.folders.iter().any(|f| f == &folder) {
        g.folders.push(folder);
    }
    write_json(&grants_file(id), &g)
}

// ---------- per-app key-value store ----------

pub type Kv = BTreeMap<String, serde_json::Value>;

pub fn load_kv(id: &str) -> Kv {
    read_json(&data_dir(id).join("store.json"))
}
pub fn save_kv(id: &str, kv: &Kv) -> std::io::Result<()> {
    write_json(&data_dir(id).join("store.json"), kv)
}

// ---------- API keys ----------

const KEYRING_SERVICE: &str = "deskhatch";

pub fn set_api_key(provider: &str, key: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, provider).map_err(|e| e.to_string())?;
    if key.is_empty() {
        let _ = entry.delete_credential();
        return Ok(());
    }
    entry.set_password(key).map_err(|e| e.to_string())
}

/// Order: env DESKHATCH_API_KEY_<PROVIDER>, env DESKHATCH_API_KEY, keychain, OPENAI_API_KEY (openai only).
pub fn get_api_key(provider: &str) -> Option<String> {
    let up = provider.to_ascii_uppercase().replace('-', "_");
    if let Ok(k) = std::env::var(format!("DESKHATCH_API_KEY_{up}")) {
        if !k.is_empty() {
            return Some(k);
        }
    }
    if let Ok(k) = std::env::var("DESKHATCH_API_KEY") {
        if !k.is_empty() {
            return Some(k);
        }
    }
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, provider) {
        if let Ok(k) = entry.get_password() {
            if !k.is_empty() {
                return Some(k);
            }
        }
    }
    if provider == "openai" {
        if let Ok(k) = std::env::var("OPENAI_API_KEY") {
            if !k.is_empty() {
                return Some(k);
            }
        }
    }
    None
}
