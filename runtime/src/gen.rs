//! `deskhatch new "..."`: ask the configured model for a single-file HTML app and install it.

use crate::llm::{self, ChatOptions};
use crate::manifest::{slugify, Manifest, Tier};
use crate::packages;
use crate::store;
use serde_json::json;
use std::path::Path;

pub const HATCH_API_DOC: &str = r#"
The page runs inside deskhatch, a desktop runtime. A global `hatch` object is available (also as `window.hatch`):

  await hatch.llm.chat("prompt")                           -> string reply from the user's configured model
  await hatch.llm.chat([{role:"user",content:"hi"}], {system:"...", json:true}) -> string
  await hatch.store.get("key")  /  await hatch.store.set("key", value)  -> persistent per-app JSON store (use this instead of localStorage)
  await hatch.notify("Title", "Body")                      -> desktop notification       (permissions: desktop or full)
  await hatch.fs.pickFolder()                              -> asks the user for a folder, returns its path (desktop/full)
  await hatch.fs.list(path) / readText(path) / writeText(path, text) / exists(path) / mkdir(path) / remove(path)
  await hatch.openUrl("https://...")                       -> opens in the default browser (desktop/full)
  hatch.on("hotkey", (name) => ...)                        -> global hotkeys declared in app.json
  hatch.app                                                -> {id, name, version, permissions, platform, ...}

Rules for the HTML you write:
  - ONE complete HTML file: <!doctype html>, inline <style> and inline <script>. No external CDNs, fonts or network calls; everything must work offline.
  - No frameworks. Plain modern JS (ES2020), CSS with flex/grid. Make it look polished: good spacing, system font stack, light/dark via prefers-color-scheme.
  - Put a meaningful <title>. The window is about 900x640 by default; be responsive.
  - Persist user data with hatch.store, never localStorage.
  - Wrap hatch calls in try/catch and show errors in the UI.
  - Do not include markdown fences or commentary outside the HTML.
"#;

pub async fn generate(
    prompt: &str,
    name: Option<&str>,
    tier: Tier,
    model: Option<&str>,
) -> Result<Manifest, String> {
    let system = format!(
        "You are a senior front-end engineer. You produce a complete, self-contained, single-file HTML desktop app for the user's request.\n{HATCH_API_DOC}\nPermission tier of this app: {}. Only use APIs allowed for that tier.",
        tier.label()
    );
    let user = format!("Build this app:\n\n{prompt}\n\nReturn only the HTML file.");
    let reply = llm::chat(
        None,
        vec![json!({"role": "user", "content": user})],
        ChatOptions {
            model: model.map(|s| s.to_string()),
            temperature: Some(0.4),
            max_tokens: Some(16000),
            system: Some(system),
            json: false,
        },
    )
    .await?;

    let html = extract_html(&reply.text).ok_or("the model did not return an HTML document")?;
    let title = name
        .map(|s| s.to_string())
        .or_else(|| html_title(&html))
        .unwrap_or_else(|| "My App".into());
    let mut m = Manifest::for_html(&title);
    // Never overwrite an installed app: it would inherit that app's data and folder access.
    m.id = packages::free_id(&slugify(&m.name));
    m.permissions = tier;
    m.description = Some(prompt.chars().map(|c| if c.is_control() { ' ' } else { c }).take(200).collect());
    m.llm.model = model.map(|s| s.to_string());

    let dir = store::app_dir(&m.id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("index.html"), html).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("app.json"), serde_json::to_vec_pretty(&m).unwrap()).map_err(|e| e.to_string())?;
    packages::write_default_icon(&dir.join("icon.png"), &m.name).map_err(|e| e.to_string())?;
    std::fs::write(
        dir.join("PROMPT.md"),
        format!("# Prompt\n\n{prompt}\n\n_model: {} ({})_\n", reply.model, reply.provider),
    )
    .map_err(|e| e.to_string())?;
    // Re-validate what we wrote.
    Manifest::load(&dir).map_err(|e| e.to_string())
}

fn extract_html(text: &str) -> Option<String> {
    let t = text.trim();
    // fenced block first
    if let Some(start) = t.find("```") {
        let after = &t[start + 3..];
        let nl = after.find('\n').unwrap_or(0);
        let body = &after[nl..];
        if let Some(end) = body.rfind("```") {
            let inner = body[..end].trim();
            if inner.to_ascii_lowercase().contains("<html") || inner.to_ascii_lowercase().contains("<!doctype") {
                return Some(inner.to_string());
            }
        }
    }
    let lower = t.to_ascii_lowercase();
    let start = lower.find("<!doctype").or_else(|| lower.find("<html"))?;
    let end = lower.rfind("</html>").map(|i| i + 7).unwrap_or(t.len());
    Some(t[start..end].to_string())
}

fn html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let s = lower.find("<title")?;
    let s = lower[s..].find('>')? + s + 1;
    let e = lower[s..].find("</title>")? + s;
    let t = html[s..e].trim();
    (!t.is_empty()).then(|| t.chars().take(60).collect())
}

#[allow(dead_code)]
pub fn write_prompt_doc(dir: &Path) -> std::io::Result<()> {
    std::fs::write(dir.join("HATCH-API.md"), HATCH_API_DOC)
}
