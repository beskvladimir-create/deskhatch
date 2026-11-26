//! Tauri commands = the `window.hatch` API. Every OS-touching command checks the app's tier.

use crate::llm;
use crate::manifest::{Manifest, Tier};
use crate::packages;
use crate::store;
use crate::AppCtx;
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, State, Window};
use tauri_plugin_notification::NotificationExt;

type R<T> = Result<T, String>;

fn need<'a>(ctx: &'a AppCtx, tier: Tier, what: &str) -> R<&'a Manifest> {
    let m = ctx
        .manifest
        .as_ref()
        .ok_or_else(|| "not available in the gallery window".to_string())?;
    if m.permissions < tier {
        return Err(format!(
            "{what} requires permissions \"{}\" but app.json declares \"{}\"",
            tier.label(),
            m.permissions.label()
        ));
    }
    Ok(m)
}

fn gallery_only(window: &Window) -> R<()> {
    if window.label() == "gallery" {
        Ok(())
    } else {
        Err("gallery-only command".into())
    }
}

#[derive(Serialize)]
pub struct AppInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub permissions: String,
    pub dir: Option<PathBuf>,
    pub dev: bool,
    pub runtime: String,
    pub platform: String,
    pub llm: llm::Resolved,
    pub hotkeys: std::collections::BTreeMap<String, String>,
}

#[tauri::command]
pub fn app_info(ctx: State<'_, AppCtx>) -> AppInfo {
    let m = ctx.manifest.as_ref();
    AppInfo {
        id: m.map(|m| m.id.clone()).unwrap_or_else(|| "gallery".into()),
        name: m.map(|m| m.name.clone()).unwrap_or_else(|| "deskhatch".into()),
        version: m.map(|m| m.version.clone()).unwrap_or_default(),
        permissions: m.map(|m| m.permissions.label().to_string()).unwrap_or_else(|| "full".into()),
        dir: ctx.app_dir.clone(),
        dev: ctx.dev,
        runtime: env!("CARGO_PKG_VERSION").into(),
        platform: std::env::consts::OS.into(),
        llm: llm::resolve(m, None),
        hotkeys: m.map(|m| m.hotkeys.clone()).unwrap_or_default(),
    }
}

// ---------- LLM ----------

#[tauri::command]
pub async fn llm_chat(
    ctx: State<'_, AppCtx>,
    messages: Value,
    options: Option<llm::ChatOptions>,
) -> R<llm::ChatReply> {
    let msgs: Vec<Value> = match messages {
        Value::String(s) => vec![serde_json::json!({"role": "user", "content": s})],
        Value::Array(a) => a,
        _ => return Err("messages must be a string or an array of {role, content}".into()),
    };
    let opts = options.unwrap_or(llm::ChatOptions {
        model: None,
        temperature: None,
        max_tokens: None,
        system: None,
        json: false,
    });
    llm::chat(ctx.manifest.as_ref(), msgs, opts).await
}

#[tauri::command]
pub async fn llm_models(ctx: State<'_, AppCtx>) -> R<Vec<String>> {
    llm::models(ctx.manifest.as_ref()).await
}

// ---------- notifications / tray / misc ----------

#[tauri::command]
pub fn notify(app: AppHandle, ctx: State<'_, AppCtx>, title: String, body: Option<String>) -> R<()> {
    need(&ctx, Tier::Desktop, "notify")?;
    let mut b = app.notification().builder().title(title);
    if let Some(body) = body {
        b = b.body(body);
    }
    b.show().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn open_url(app: AppHandle, ctx: State<'_, AppCtx>, url: String) -> R<()> {
    let m = need(&ctx, Tier::Desktop, "openUrl")?.clone();
    if !(url.starts_with("https://") || url.starts_with("http://") || url.starts_with("mailto:")) {
        return Err("only http(s) and mailto URLs can be opened".into());
    }
    // A URL carries data out in its query string, so a host outside allowedHosts needs the
    // user's yes every time.
    let host = reqwest::Url::parse(&url).ok().and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()));
    let listed = host.as_deref().is_some_and(|h| {
        m.allowed_hosts.iter().any(|a| a == "*" || a.eq_ignore_ascii_case(h))
    });
    if !listed {
        use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
        let shown: String = url.chars().take(300).collect();
        let yes = app
            .dialog()
            .message(format!("{} wants to open this address in your browser:\n\n{}", m.name, shown))
            .title("Open link?")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom("Open".into(), "Cancel".into()))
            .blocking_show();
        if !yes {
            return Err("the user declined to open the link".into());
        }
    }
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn about(app: AppHandle, ctx: State<'_, AppCtx>) {
    crate::show_about(&app, ctx.manifest.as_ref());
}

// ---------- per-app store ----------

fn store_id(ctx: &AppCtx) -> String {
    ctx.manifest
        .as_ref()
        .map(|m| m.id.clone())
        .unwrap_or_else(|| "__gallery".into())
}

#[tauri::command]
pub fn store_get(ctx: State<'_, AppCtx>, key: String) -> Option<Value> {
    store::load_kv(&store_id(&ctx)).get(&key).cloned()
}

#[tauri::command]
pub fn store_set(ctx: State<'_, AppCtx>, key: String, value: Value) -> R<()> {
    let id = store_id(&ctx);
    let mut kv = store::load_kv(&id);
    if value.is_null() {
        kv.remove(&key);
    } else {
        kv.insert(key, value);
    }
    let bytes = serde_json::to_vec(&kv).map_err(|e| e.to_string())?;
    if bytes.len() > 20 * 1024 * 1024 {
        return Err("store limit is 20 MB per app".into());
    }
    store::save_kv(&id, &kv).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn store_all(ctx: State<'_, AppCtx>) -> store::Kv {
    store::load_kv(&store_id(&ctx))
}

// ---------- files (scoped) ----------

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

fn allowed(m: &Manifest, p: &Path, _for_write: bool) -> R<PathBuf> {
    let abs = if p.is_absolute() { p.to_path_buf() } else { home().join(p) };
    // Resolve the deepest existing ancestor so new files can be created inside granted folders.
    let mut probe = abs.clone();
    let mut tail = vec![];
    while !probe.exists() {
        match (probe.file_name(), probe.parent()) {
            (Some(n), Some(par)) => {
                tail.push(n.to_os_string());
                probe = par.to_path_buf();
            }
            _ => break,
        }
    }
    let mut canon = probe.canonicalize().map_err(|e| e.to_string())?;
    for t in tail.iter().rev() {
        canon.push(t);
    }
    let home_c = home().canonicalize().unwrap_or_else(|_| home());
    let ok = match m.permissions {
        // Reads and writes alike: the home folder, plus anything the user picked in a dialog.
        Tier::Full => {
            canon.starts_with(&home_c)
                || store::load_grants(&m.id).folders.iter().any(|g| canon.starts_with(g))
        }
        Tier::Desktop => store::load_grants(&m.id)
            .folders
            .iter()
            .any(|g| canon.starts_with(g)),
        Tier::Web => false,
    };
    // Never touch deskhatch's own data or credential folders.
    if is_forbidden(&canon, &home_c) {
        return Err(format!("access to {} is not allowed", canon.display()));
    }
    if ok {
        Ok(canon)
    } else {
        Err(format!(
            "no access to {}. Ask the user with hatch.fs.pickFolder() first.",
            canon.display()
        ))
    }
}


/// Folders no app may read or write at any tier, even when the user picked them: deskhatch's
/// own data, credentials and tokens, browser profiles, mail and messenger data. Compared
/// case-insensitively, because the default macOS file system is.
const FORBIDDEN_IN_HOME: &[&str] = &[
    ".ssh", ".gnupg", ".aws", ".azure", ".kube", ".docker", ".config/gh", ".config/gcloud",
    ".config/hub", ".git-credentials", ".netrc", ".npmrc", ".pypirc", ".cargo/credentials",
    ".cargo/credentials.toml", ".password-store", ".local/share/keyrings", ".mozilla",
    ".thunderbird", ".config/google-chrome", ".config/chromium", ".config/BraveSoftware",
    ".config/microsoft-edge", ".config/Signal", ".config/discord", ".config/Slack",
    ".config/Element", ".local/share/TelegramDesktop", ".var/app", "snap",
    "Library/Keychains", "Library/Cookies", "Library/Messages", "Library/Mail",
    "Library/Containers", "Library/Group Containers", "Library/Safari",
    "Library/Application Support/Google", "Library/Application Support/Firefox",
    "Library/Application Support/BraveSoftware", "Library/Application Support/Microsoft Edge",
    "Library/Application Support/Telegram Desktop", "Library/Application Support/Signal",
    "Library/Application Support/WhatsApp", "Library/Application Support/discord",
    "Library/Application Support/Slack", "Library/Application Support/Viber",
    "Library/Application Support/AddressBook", "Library/Application Support/com.apple.TCC",
    "Library/Application Support/Claude",
];

pub(crate) fn is_forbidden(canon: &Path, home_c: &Path) -> bool {
    let lower = |p: &Path| p.to_string_lossy().to_lowercase();
    let c = lower(canon);
    let under = |f: &Path| {
        let f = lower(f);
        c == f || c.starts_with(&(f + "/"))
    };
    let root = store::root();
    let root_c = root.canonicalize().unwrap_or(root.clone());
    under(&root) || under(&root_c) || FORBIDDEN_IN_HOME.iter().any(|d| under(&home_c.join(d)) || under(&home().join(d)))
}

#[tauri::command]
pub async fn fs_pick_folder(app: AppHandle, ctx: State<'_, AppCtx>, title: Option<String>) -> R<Option<String>> {
    let m = need(&ctx, Tier::Desktop, "fs.pickFolder")?.clone();
    use tauri_plugin_dialog::DialogExt;
    let picked = app
        .dialog()
        .file()
        .set_title(title.unwrap_or_else(|| format!("Choose a folder for {}", m.name)))
        .blocking_pick_folder();
    match picked {
        Some(fp) => {
            let p = fp.into_path().map_err(|e| e.to_string())?;
            store::add_grant(&m.id, &p).map_err(|e| e.to_string())?;
            Ok(Some(p.to_string_lossy().to_string()))
        }
        None => Ok(None),
    }
}

#[tauri::command]
pub async fn fs_pick_file(app: AppHandle, ctx: State<'_, AppCtx>, title: Option<String>) -> R<Option<String>> {
    let m = need(&ctx, Tier::Desktop, "fs.pickFile")?.clone();
    use tauri_plugin_dialog::DialogExt;
    let picked = app
        .dialog()
        .file()
        .set_title(title.unwrap_or_else(|| format!("Choose a file for {}", m.name)))
        .blocking_pick_file();
    match picked {
        Some(fp) => {
            let p = fp.into_path().map_err(|e| e.to_string())?;
            // Grant that one file, not its whole folder.
            store::add_grant(&m.id, &p).map_err(|e| e.to_string())?;
            Ok(Some(p.to_string_lossy().to_string()))
        }
        None => Ok(None),
    }
}

#[tauri::command]
pub fn fs_grants(ctx: State<'_, AppCtx>) -> Vec<String> {
    match ctx.manifest.as_ref() {
        Some(m) => store::load_grants(&m.id)
            .folders
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect(),
        None => vec![],
    }
}

#[derive(Serialize)]
pub struct Entry {
    name: String,
    path: String,
    is_dir: bool,
    size: u64,
    modified: Option<u64>,
}

#[tauri::command]
pub fn fs_list(ctx: State<'_, AppCtx>, path: String) -> R<Vec<Entry>> {
    let m = need(&ctx, Tier::Desktop, "fs.list")?;
    let p = allowed(m, Path::new(&path), false)?;
    let mut out = vec![];
    for e in std::fs::read_dir(&p).map_err(|e| e.to_string())?.flatten() {
        let md = e.metadata().map_err(|e| e.to_string())?;
        out.push(Entry {
            name: e.file_name().to_string_lossy().to_string(),
            path: e.path().to_string_lossy().to_string(),
            is_dir: md.is_dir(),
            size: md.len(),
            modified: md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs()),
        });
    }
    out.sort_by(|a, b| (!a.is_dir, a.name.to_lowercase()).cmp(&(!b.is_dir, b.name.to_lowercase())));
    Ok(out)
}

#[tauri::command]
pub fn fs_read_text(ctx: State<'_, AppCtx>, path: String) -> R<String> {
    let m = need(&ctx, Tier::Desktop, "fs.readText")?;
    let p = allowed(m, Path::new(&path), false)?;
    let md = std::fs::metadata(&p).map_err(|e| e.to_string())?;
    if md.len() > 50 * 1024 * 1024 {
        return Err("file larger than 50 MB".into());
    }
    std::fs::read_to_string(&p).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn fs_write_text(ctx: State<'_, AppCtx>, path: String, contents: String) -> R<()> {
    let m = need(&ctx, Tier::Desktop, "fs.writeText")?;
    let p = allowed(m, Path::new(&path), true)?;
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, contents).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn fs_exists(ctx: State<'_, AppCtx>, path: String) -> R<bool> {
    let m = need(&ctx, Tier::Desktop, "fs.exists")?;
    Ok(allowed(m, Path::new(&path), false).map(|p| p.exists()).unwrap_or(false))
}

#[tauri::command]
pub fn fs_mkdir(ctx: State<'_, AppCtx>, path: String) -> R<()> {
    let m = need(&ctx, Tier::Desktop, "fs.mkdir")?;
    let p = allowed(m, Path::new(&path), true)?;
    std::fs::create_dir_all(&p).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn fs_remove(ctx: State<'_, AppCtx>, path: String) -> R<()> {
    let m = need(&ctx, Tier::Desktop, "fs.remove")?;
    let p = allowed(m, Path::new(&path), true)?;
    if p.is_dir() {
        std::fs::remove_dir(&p).map_err(|e| e.to_string())
    } else {
        std::fs::remove_file(&p).map_err(|e| e.to_string())
    }
}

// ---------- gallery ----------

#[tauri::command]
pub fn gallery_list(window: Window) -> R<Vec<packages::Installed>> {
    gallery_only(&window)?;
    Ok(packages::list())
}

#[tauri::command]
pub fn gallery_open(window: Window, id: String) -> R<()> {
    gallery_only(&window)?;
    crate::spawn_app(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn gallery_uninstall(window: Window, id: String) -> R<()> {
    gallery_only(&window)?;
    packages::uninstall(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn gallery_import(app: AppHandle, window: Window) -> R<Option<String>> {
    gallery_only(&window)?;
    use tauri_plugin_dialog::DialogExt;
    let picked = app
        .dialog()
        .file()
        .set_title("Choose a .hatch package, an app.json or an .html file")
        .add_filter("deskhatch app", &["hatch", "zip", "html", "htm", "json"])
        .blocking_pick_file();
    let Some(fp) = picked else { return Ok(None) };
    let mut p = fp.into_path().map_err(|e| e.to_string())?;
    if p.file_name().and_then(|n| n.to_str()) == Some("app.json") {
        p = p.parent().unwrap().to_path_buf();
    }
    if let Some(w) = packages::peek(&p).map_err(|e| e.to_string())?.as_ref().and_then(packages::install_warning) {
        use tauri_plugin_dialog::{MessageDialogButtons, MessageDialogKind};
        let yes = app
            .dialog()
            .message(w)
            .title("Install this app?")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom("Install".into(), "Cancel".into()))
            .blocking_show();
        if !yes {
            return Ok(None);
        }
    }
    let m = packages::import(&p, None).map_err(|e| e.to_string())?;
    let _ = packages::create_launcher(&m);
    Ok(Some(m.id))
}

#[tauri::command]
pub fn gallery_share(window: Window, id: String) -> R<ShareResult> {
    gallery_only(&window)?;
    let out = dirs::download_dir().unwrap_or_else(home);
    let p = packages::export(&id, &out).map_err(|e| e.to_string())?;
    let _ = tauri_plugin_opener::reveal_item_in_dir(&p);
    let m = Manifest::load(&store::app_dir(&id)).map_err(|e| e.to_string())?;
    Ok(ShareResult { path: p.to_string_lossy().to_string(), note: packages::share_note(&m) })
}

#[derive(Serialize)]
pub struct ShareResult {
    pub path: String,
    pub note: String,
}

#[tauri::command]
pub fn gallery_launcher(window: Window, id: String) -> R<String> {
    gallery_only(&window)?;
    let m = Manifest::load(&store::app_dir(&id)).map_err(|e| e.to_string())?;
    // Keep whatever kind of launcher this app already has instead of silently downgrading it.
    let standalone = packages::launcher_is_standalone(&m);
    let p = packages::create_launcher_for(&m, standalone).map_err(|e| e.to_string())?;
    // Rebuilding an icon that was already there looks like nothing happened, so show it.
    if std::env::var_os("DESKHATCH_TEST").is_none() {
        let _ = tauri_plugin_opener::reveal_item_in_dir(&p);
    }
    Ok(p.to_string_lossy().to_string())
}

/// Test-only sink for `--eval`: prints the expression's result and ends the process, so a test
/// can read it from stdout. Refuses unless DESKHATCH_TEST is set.
#[tauri::command]
pub fn test_result(text: String) -> R<()> {
    if std::env::var_os("DESKHATCH_TEST").is_none() {
        return Err("test-only command".into());
    }
    println!("{text}");
    use std::io::Write;
    let _ = std::io::stdout().flush();
    std::process::exit(0);
}

#[tauri::command]
pub fn gallery_reveal(window: Window, id: String) -> R<()> {
    gallery_only(&window)?;
    tauri_plugin_opener::reveal_item_in_dir(store::app_dir(&id)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn config_get(window: Window) -> R<store::Config> {
    gallery_only(&window)?;
    Ok(store::load_config())
}

#[tauri::command]
pub fn config_set(window: Window, config: store::Config) -> R<()> {
    gallery_only(&window)?;
    store::save_config(&config).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn key_set(window: Window, provider: String, key: String) -> R<()> {
    gallery_only(&window)?;
    store::set_api_key(&provider, &key)
}

#[tauri::command]
pub fn key_has(window: Window, provider: String) -> R<bool> {
    gallery_only(&window)?;
    Ok(store::get_api_key(&provider).is_some())
}

#[tauri::command]
pub async fn gallery_new(window: Window, prompt: String, name: Option<String>, tier: Option<String>) -> R<String> {
    gallery_only(&window)?;
    let tier = match tier.as_deref() {
        Some("desktop") => Tier::Desktop,
        Some("full") => Tier::Full,
        _ => Tier::Web,
    };
    let m = crate::gen::generate(&prompt, name.as_deref(), tier, None).await?;
    let _ = packages::create_launcher(&m);
    Ok(m.id)
}

pub fn exit_app(app: &AppHandle) {
    app.exit(0);
}

#[tauri::command]
pub fn quit(app: AppHandle) {
    exit_app(&app);
}

#[tauri::command]
pub fn window_toggle(window: Window) -> R<()> {
    let w = window.app_handle().get_webview_window("main").ok_or("no window")?;
    if w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) {
        w.hide().map_err(|e| e.to_string())
    } else {
        w.show().and_then(|_| w.set_focus()).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_tier_stays_in_home_for_reads_and_writes() {
        let tmp = std::env::temp_dir().join(format!("deskhatch-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("DESKHATCH_HOME", &tmp);
        let m: Manifest = serde_json::from_str(r#"{"id":"t","name":"t","permissions":"full"}"#).unwrap();
        // Outside home: refused for reading as well as writing.
        assert!(allowed(&m, Path::new("/etc/hosts"), false).is_err());
        assert!(allowed(&m, Path::new("/etc/hosts"), true).is_err());
        // Inside home: allowed.
        assert!(allowed(&m, &home().join("deskhatch-test-file"), false).is_ok());
        // Credential folders stay closed, in any letter case.
        assert!(allowed(&m, &home().join(".ssh").join("id_ed25519"), false).is_err());
        assert!(allowed(&m, &home().join(".SSH").join("id_ed25519"), false).is_err());
        // Tokens, browser profiles, mail and messengers stay closed too.
        for p in [".config/gh/hosts.yml", ".kube/config", ".netrc", "Library/Messages/chat.db",
                  "Library/Group Containers/x/y", "Library/Application Support/Telegram Desktop/tdata",
                  ".local/share/TelegramDesktop/tdata", ".config/google-chrome/Default/Cookies"] {
            assert!(allowed(&m, &home().join(p), false).is_err(), "{p} must be closed");
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
