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
    #[error("app.json is not allowed: {0}")]
    Unsafe(String),
}

/// A relative path that stays inside the package: only plain components, no `..`, no root.
pub fn safe_rel_path(p: &str) -> bool {
    use std::path::Component;
    let path = Path::new(p);
    !p.is_empty()
        && !p.contains('\\')
        && path.components().all(|c| matches!(c, Component::Normal(_)))
}

fn has_control(s: &str) -> bool {
    s.chars().any(|c| c.is_control())
}

/// A host for allowedHosts: "*", "host.name[:port]" or "https://host.name[:port]".
fn valid_host_entry(h: &str) -> bool {
    if h == "*" {
        return true;
    }
    // https only: plain http would travel unencrypted, and the network guard tunnels only TLS.
    let rest = h.strip_prefix("https://").unwrap_or(h);
    let (host, port) = match rest.rsplit_once(':') {
        Some((a, b)) => (a, Some(b)),
        None => (rest, None),
    };
    !host.is_empty()
        && host.len() <= 253
        && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && port.is_none_or(|p| !p.is_empty() && p.len() <= 5 && p.chars().all(|c| c.is_ascii_digit()))
}

/// Global hotkeys need a real modifier, so an app cannot listen for plain typing, and may not
/// take the shortcuts everyone relies on.
fn valid_hotkey(acc: &str) -> bool {
    let parts: Vec<String> = acc.split('+').map(|p| p.trim().to_ascii_lowercase()).collect();
    let mods = ["cmdorctrl", "commandorcontrol", "ctrl", "control", "cmd", "command", "super", "meta", "alt", "option"];
    let has_mod = parts.iter().any(|p| mods.contains(&p.as_str()));
    let Some(key) = parts.last() else { return false };
    let only_primary = parts.len() == 2;
    let reserved = ["c", "v", "x", "a", "z", "q", "w", "tab", "space", "keyc", "keyv", "keyx", "keya", "keyz", "keyq", "keyw"];
    has_mod && !(only_primary && reserved.contains(&key.as_str()))
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !id.starts_with('-')
}

/// A display name made safe for launchers and dialogs: whitespace and control characters
/// collapse to single spaces, at most 60 characters, never empty, never "deskhatch".
pub fn clean_name(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_control() || c.is_whitespace() {
            if !out.ends_with(' ') && !out.is_empty() {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    let out: String = out.trim().chars().take(60).collect();
    let out = out.trim().to_string();
    if out.is_empty() {
        "App".into()
    } else if out.eq_ignore_ascii_case("deskhatch") {
        "deskhatch app".into()
    } else {
        out
    }
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
        let bad = |why: &str| Err(ManifestError::Unsafe(why.to_string()));
        // Text fields end up in launchers (.desktop files, Info.plist), dialogs and the gallery.
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 64 || has_control(&self.name) {
            return bad("name must be 1-64 characters with no line breaks or control characters");
        }
        if name.eq_ignore_ascii_case("deskhatch") {
            return bad("the name \"deskhatch\" is reserved");
        }
        for (what, v) in [
            ("version", Some(self.version.as_str())),
            ("description", self.description.as_deref()),
            ("author", self.author.as_deref()),
            ("homepage", self.homepage.as_deref()),
            ("tray.tooltip", self.tray.tooltip.as_deref()),
        ] {
            if let Some(v) = v {
                if has_control(v) || v.chars().count() > 500 {
                    return Err(ManifestError::Unsafe(format!("{what} has line breaks or control characters, or is too long")));
                }
            }
        }
        if !safe_rel_path(&self.entry) || !dir.join(&self.entry).is_file() {
            return Err(ManifestError::MissingEntry(self.entry.clone()));
        }
        if !safe_rel_path(&self.icon) {
            return bad("icon must be a path inside the package");
        }
        if self.allowed_hosts.len() > 32 || !self.allowed_hosts.iter().all(|h| valid_host_entry(h)) {
            return bad("allowedHosts entries must be host names, optionally with https:// and a port (no http://)");
        }
        if self.hotkeys.len() > 5 {
            return bad("at most 5 hotkeys");
        }
        for (k, v) in &self.hotkeys {
            if has_control(k) || k.len() > 64 || !valid_hotkey(v) {
                return Err(ManifestError::Unsafe(format!(
                    "hotkey {k:?} = {v:?} must use Ctrl, Alt, Cmd or Super and may not take a common shortcut"
                )));
            }
        }
        if let Some(e) = self.llm.endpoint.as_deref() {
            if e.contains('?') || e.contains('#') || has_control(e) {
                return bad("llm.endpoint may not contain a query or fragment");
            }
        }
        for v in [self.llm.provider.as_deref(), self.llm.model.as_deref()].into_iter().flatten() {
            if has_control(v) || v.len() > 200 {
                return bad("llm.provider and llm.model must be short, single-line values");
            }
        }
        Ok(())
    }

    /// Build a minimal manifest for a bare HTML file (used by `deskhatch install page.html`).
    pub fn for_html(name: &str) -> Manifest {
        let name = clean_name(name);
        Manifest {
            schema: 1,
            id: slugify(&name),
            name,
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
