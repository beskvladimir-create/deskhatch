//! `app.json` — the manifest of a deskhatch app package. See SPEC.md in the repo root.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    /// Browser-like: no network from the page, no OS access. LLM bridge and per-app store only.
    #[default]
    Web,
    /// Notifications, tray, hotkeys, open-URL, file access limited to folders the user picks.
    Desktop,
    /// Everything in Desktop plus unrestricted file access inside the user's home directory.
    Full,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Web => "web",
            Tier::Desktop => "desktop",
            Tier::Full => "full",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowSpec {
    #[serde(default = "d_width")]
    pub width: f64,
    #[serde(default = "d_height")]
    pub height: f64,
    #[serde(default = "d_true")]
    pub resizable: bool,
    #[serde(default)]
    pub always_on_top: bool,
    #[serde(default)]
    pub transparent: bool,
    #[serde(default = "d_true")]
    pub decorations: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_height: Option<f64>,
}

impl Default for WindowSpec {
    fn default() -> Self {
        Self {
            width: d_width(),
            height: d_height(),
            resizable: true,
            always_on_top: false,
            transparent: false,
            decorations: true,
            min_width: None,
            min_height: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TraySpec {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<String>,
    /// When true, closing the window hides it to the tray instead of quitting.
    #[serde(default)]
    pub hide_on_close: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LlmSpec {
    /// "ollama" | "openai" | any name you also used with `deskhatch key <provider>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// OpenAI-compatible base URL, e.g. http://localhost:11434/v1 or https://api.openai.com/v1
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Default system prompt prepended to every chat() call unless the app passes its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    #[serde(default = "d_schema")]
    pub schema: u32,
    pub id: String,
    pub name: String,
    #[serde(default = "d_version")]
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "d_icon")]
    pub icon: String,
    #[serde(default = "d_entry")]
    pub entry: String,
    #[serde(default)]
    pub window: WindowSpec,
    #[serde(default)]
    pub permissions: Tier,
    #[serde(default)]
    pub tray: TraySpec,
    /// name -> accelerator, e.g. {"toggle": "CmdOrCtrl+Shift+K"}. "toggle" is handled by the
    /// runtime (show/hide window); every hotkey is also delivered to the page as a `hotkey` event.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub hotkeys: BTreeMap<String, String>,
    #[serde(default)]
    pub llm: LlmSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// Hosts the page may talk to directly (desktop/full tiers only). "*" allows any https host.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_hosts: Vec<String>,
}

fn d_schema() -> u32 {
    1
}
fn d_version() -> String {
    "0.1.0".into()
}
fn d_icon() -> String {
    "icon.png".into()
}
fn d_entry() -> String {
    "index.html".into()
}
fn d_width() -> f64 {
    900.0
}
fn d_height() -> f64 {
    640.0
}
fn d_true() -> bool {
    true
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("app.json not found in {0}")]
    Missing(String),
    #[error("app.json is not valid: {0}")]
    Invalid(String),
    #[error("app id must be 1-64 chars of [a-z0-9-], got {0:?}")]
    BadId(String),
    #[error("entry file {0} not found")]
    MissingEntry(String),
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !id.starts_with('-')
}

pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in name.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "app".into()
    } else {
        out.chars().take(64).collect()
    }
}

impl Manifest {
    pub fn load(dir: &Path) -> Result<Manifest, ManifestError> {
        let p = dir.join("app.json");
        let raw = std::fs::read_to_string(&p)
            .map_err(|_| ManifestError::Missing(dir.display().to_string()))?;
        let m: Manifest =
            serde_json::from_str(&raw).map_err(|e| ManifestError::Invalid(e.to_string()))?;
        m.validate(dir)?;
        Ok(m)
    }

    pub fn validate(&self, dir: &Path) -> Result<(), ManifestError> {
        if !valid_id(&self.id) {
            return Err(ManifestError::BadId(self.id.clone()));
        }
        if self.entry.contains("..") || !dir.join(&self.entry).is_file() {
            return Err(ManifestError::MissingEntry(self.entry.clone()));
        }
        Ok(())
    }

    /// Build a minimal manifest for a bare HTML file (used by `deskhatch install page.html`).
    pub fn for_html(name: &str) -> Manifest {
        Manifest {
            schema: 1,
            id: slugify(name),
            name: name.to_string(),
            version: d_version(),
            description: None,
            icon: d_icon(),
            entry: d_entry(),
            window: WindowSpec::default(),
            permissions: Tier::Web,
            tray: TraySpec::default(),
            hotkeys: BTreeMap::new(),
            llm: LlmSpec::default(),
            author: None,
            homepage: None,
            allowed_hosts: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slugify("My Pomodoro Timer!"), "my-pomodoro-timer");
        assert_eq!(slugify("   "), "app");
        assert!(valid_id("a-b-1"));
        assert!(!valid_id("-a"));
        assert!(!valid_id("A"));
    }

    #[test]
    fn tier_order() {
        assert!(Tier::Web < Tier::Desktop && Tier::Desktop < Tier::Full);
    }
}
