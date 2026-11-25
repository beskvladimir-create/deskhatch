//! deskhatch runtime. One binary that is both the CLI and the app host.
//!
//!   deskhatch                      open the gallery of installed apps
//!   deskhatch --app <id>           run an installed app
//!   deskhatch --dev <dir>          run an app folder with live reload
//!   deskhatch install <src>        import a folder / .hatch / .html and create a launcher
//!   deskhatch new "<prompt>"       generate an app with the configured model and install it
//!   deskhatch list|uninstall|export|config|key|models|doctor
//!   deskhatch <file.hatch>         (file association) install and run

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod gen;
mod llm;
mod manifest;
mod packages;
mod store;

use manifest::{Manifest, Tier};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

const GALLERY_HTML: &str = include_str!("../gallery/index.html");
const SHIM_JS: &str = include_str!("../gallery/hatch.js");

/// Managed state: which app this process hosts (None = gallery).
pub struct AppCtx {
    pub manifest: Option<Manifest>,
    pub app_dir: Option<PathBuf>,
    pub dev: bool,
}

fn usage() -> ! {
    eprintln!(
        "deskhatch {}\n\nUSAGE:\n  deskhatch                         open the gallery\n  deskhatch --app <id>              run an installed app\n  deskhatch --dev <dir>             run an app folder with live reload\n  deskhatch install <src> [--name N] [--no-launcher] [--run]\n                                    src = folder with app.json | file.hatch | page.html\n  deskhatch new \"<prompt>\" [--name N] [--tier web|desktop|full] [--model M] [--run]\n  deskhatch list\n  deskhatch uninstall <id>\n  deskhatch export <id> [--out <dir>]\n  deskhatch config [--provider P] [--model M] [--endpoint URL]\n  deskhatch key <provider>          read an API key from stdin and store it in the OS keychain\n  deskhatch models                  list models from the configured endpoint\n  deskhatch doctor                  print paths and configuration\n  deskhatch <file.hatch>            install and run a package",
        env!("CARGO_PKG_VERSION")
    );
    std::process::exit(2)
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = store::ensure_dirs() {
        eprintln!("deskhatch: cannot create {}: {e}", store::root().display());
    }

    match args.first().map(|s| s.as_str()) {
        None => run_gui(None, false),
        Some("--app") => {
            let id = args.get(1).cloned().unwrap_or_else(|| usage());
            let dir = store::app_dir(&id);
            match Manifest::load(&dir) {
                Ok(_) => run_gui(Some(dir), false),
                Err(e) => fatal(&format!("{e}. Installed apps: {}", ids().join(", "))),
            }
        }
        Some("--dev") => {
            let dir = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| usage()));
            let dir = dir.canonicalize().unwrap_or(dir);
            if !dir.join("app.json").exists() {
                if dir.join("index.html").exists() {
                    let name = dir
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Dev App")
                        .to_string();
                    let m = Manifest::for_html(&name);
                    let _ = std::fs::write(dir.join("app.json"), serde_json::to_vec_pretty(&m).unwrap());
                    if !dir.join("icon.png").exists() {
                        let _ = packages::write_default_icon(&dir.join("icon.png"), &name);
                    }
                    eprintln!("deskhatch: wrote a default app.json into {}", dir.display());
                } else {
                    fatal("--dev expects a folder with index.html");
                }
            }
            run_gui(Some(dir), true)
        }
        Some("install") => {
            let src = args.get(1).cloned().unwrap_or_else(|| usage());
            let name = flag(&args, "--name");
            let m = packages::import(Path::new(&src), name.as_deref()).unwrap_or_else(|e| fatal(&e.to_string()));
            println!("installed {} ({}) -> {}", m.name, m.id, store::app_dir(&m.id).display());
            if !has(&args, "--no-launcher") {
                match packages::create_launcher(&m) {
                    Ok(p) => println!("launcher    {}", p.display()),
                    Err(e) => eprintln!("launcher skipped: {e}"),
                }
            }
            if has(&args, "--run") {
                spawn_app(&m.id).unwrap_or_else(|e| fatal(&e.to_string()));
            } else {
                println!("run:        deskhatch --app {}", m.id);
            }
        }
        Some("new") => {
            let prompt = args.get(1).cloned().unwrap_or_else(|| usage());
            let name = flag(&args, "--name");
            let tier = match flag(&args, "--tier").as_deref() {
                Some("desktop") => Tier::Desktop,
                Some("full") => Tier::Full,
                _ => Tier::Web,
            };
            let model = flag(&args, "--model");
            let r = llm::resolve(None, model.as_deref());
            eprintln!("generating with {} / {} at {} ...", r.provider, r.model, r.endpoint);
            let m = tauri::async_runtime::block_on(gen::generate(&prompt, name.as_deref(), tier, model.as_deref()))
                .unwrap_or_else(|e| fatal(&e));
            println!("created {} ({}) -> {}", m.name, m.id, store::app_dir(&m.id).display());
            match packages::create_launcher(&m) {
                Ok(p) => println!("launcher {}", p.display()),
                Err(e) => eprintln!("launcher skipped: {e}"),
            }
            if has(&args, "--run") {
                spawn_app(&m.id).unwrap_or_else(|e| fatal(&e.to_string()));
            } else {
                println!("run:     deskhatch --app {}", m.id);
            }
        }
        Some("list") => {
            let apps = packages::list();
            if apps.is_empty() {
                println!("no apps installed. Try: deskhatch install examples/pomodoro");
            }
            for a in apps {
                println!("{:<24} {:<8} {:<8} {}", a.id, a.version, a.permissions, a.name);
            }
        }
        Some("uninstall") => {
            let id = args.get(1).cloned().unwrap_or_else(|| usage());
            packages::uninstall(&id).unwrap_or_else(|e| fatal(&e.to_string()));
            println!("removed {id}");
        }
        Some("export") => {
            let id = args.get(1).cloned().unwrap_or_else(|| usage());
            let out = flag(&args, "--out").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
            let p = packages::export(&id, &out).unwrap_or_else(|e| fatal(&e.to_string()));
            println!("{}", p.display());
        }
        Some("config") => {
            let mut c = store::load_config();
            let mut changed = false;
            if let Some(p) = flag(&args, "--provider") {
                c.provider = Some(p);
                changed = true;
            }
            let prov = c.provider.clone().unwrap_or_else(|| "ollama".into());
            if let Some(m) = flag(&args, "--model") {
                c.providers.entry(prov.clone()).or_default().model = Some(m);
                changed = true;
            }
            if let Some(e) = flag(&args, "--endpoint") {
                c.providers.entry(prov.clone()).or_default().endpoint = Some(e);
                changed = true;
            }
            if changed {
                store::save_config(&c).unwrap_or_else(|e| fatal(&e.to_string()));
            }
            let r = llm::resolve(None, None);
            println!("provider  {}\nmodel     {}\nendpoint  {}\napi key   {}\nconfig    {}", r.provider, r.model, r.endpoint, if r.has_key { "set" } else { "none" }, store::config_file().display());
        }
        Some("key") => {
            let provider = args.get(1).cloned().unwrap_or_else(|| usage());
            eprintln!("paste the API key for {provider} and press Enter (input is not echoed back):");
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).ok();
            let key = line.trim();
            store::set_api_key(&provider, key).unwrap_or_else(|e| fatal(&e));
            if key.is_empty() {
                println!("key for {provider} removed");
            } else {
                println!("key for {provider} stored in the OS keychain");
            }
        }
        Some("models") => {
            let list = tauri::async_runtime::block_on(llm::models(None)).unwrap_or_else(|e| fatal(&e));
            for m in list {
                println!("{m}");
            }
        }
        Some("doctor") => {
            let r = llm::resolve(None, None);
            println!("deskhatch {}\nhome      {}\napps      {}\nruntime   {}\nprovider  {} ({}) key: {}\nmodel     {}", env!("CARGO_PKG_VERSION"), store::root().display(), packages::list().len(), std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(), r.provider, r.endpoint, if r.has_key { "set" } else { "none" }, r.model);
        }
        Some("--version") | Some("-V") => println!("deskhatch {}", env!("CARGO_PKG_VERSION")),
        Some("--help") | Some("-h") | Some("help") => usage(),
        Some(other) => {
            // A path (file association / drag and drop onto the binary).
            let p = Path::new(other);
            if p.exists() {
                let m = packages::import(p, None).unwrap_or_else(|e| fatal(&e.to_string()));
                let _ = packages::create_launcher(&m);
                run_gui(Some(store::app_dir(&m.id)), false)
            } else {
                eprintln!("unknown command or missing file: {other}\n");
                usage()
            }
        }
    }
}

fn ids() -> Vec<String> {
    packages::list().into_iter().map(|a| a.id).collect()
}

fn fatal(msg: &str) -> ! {
    eprintln!("deskhatch: {msg}");
    std::process::exit(1)
}

pub fn spawn_app(id: &str) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe).arg("--app").arg(id).spawn()?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// GUI
// ---------------------------------------------------------------------------------------------

fn run_gui(app_dir: Option<PathBuf>, dev: bool) {
    let manifest = app_dir.as_ref().map(|d| Manifest::load(d).unwrap_or_else(|e| fatal(&e.to_string())));
    let ctx = AppCtx { manifest, app_dir, dev };

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(ctx)
        .register_uri_scheme_protocol("hatch", |ctx, request| {
            let state = ctx.app_handle().state::<AppCtx>();
            serve(&state, request)
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::llm_chat,
            commands::llm_models,
            commands::notify,
            commands::open_url,
            commands::about,
            commands::quit,
            commands::window_toggle,
            commands::store_get,
            commands::store_set,
            commands::store_all,
            commands::fs_pick_folder,
            commands::fs_pick_file,
            commands::fs_grants,
            commands::fs_list,
            commands::fs_read_text,
            commands::fs_write_text,
            commands::fs_exists,
            commands::fs_mkdir,
            commands::fs_remove,
            commands::gallery_list,
            commands::gallery_open,
            commands::gallery_uninstall,
            commands::gallery_import,
            commands::gallery_share,
            commands::gallery_launcher,
            commands::gallery_reveal,
            commands::gallery_new,
            commands::config_get,
            commands::config_set,
            commands::key_set,
            commands::key_has,
        ])
        .setup(|app| {
            let state = app.state::<AppCtx>();
            match (&state.manifest, &state.app_dir) {
                (Some(m), Some(dir)) => setup_app(app, m.clone(), dir.clone(), state.dev)?,
                _ => setup_gallery(app)?,
            }
            Ok(())
        });

    let app = builder
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| fatal(&format!("failed to start: {e}")));

    app.run(|app, event| {
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Opened { urls } = &event {
            for u in urls {
                if let Ok(p) = u.to_file_path() {
                    match packages::import(&p, None) {
                        Ok(m) => {
                            let _ = packages::create_launcher(&m);
                            let _ = spawn_app(&m.id);
                            let _ = app.emit("hatch:refresh", ());
                        }
                        Err(e) => eprintln!("deskhatch: cannot open {}: {e}", p.display()),
                    }
                }
            }
        }
        let _ = (app, event);
    });
}

fn load_icon(path: &Path) -> Option<tauri::image::Image<'static>> {
    tauri::image::Image::from_path(path).ok()
}

fn setup_app(app: &mut tauri::App, m: Manifest, dir: PathBuf, dev: bool) -> tauri::Result<()> {
    let icon = load_icon(&dir.join(&m.icon));
    let mut wb = WebviewWindowBuilder::new(app, "main", WebviewUrl::CustomProtocol("hatch://localhost/".parse().unwrap()))
        .title(&m.name)
        .inner_size(m.window.width, m.window.height)
        .resizable(m.window.resizable)
        .always_on_top(m.window.always_on_top)
        .decorations(m.window.decorations)
        .initialization_script(SHIM_JS);
    if let (Some(w), Some(h)) = (m.window.min_width, m.window.min_height) {
        wb = wb.min_inner_size(w, h);
    }
    if let Some(ic) = icon.clone() {
        wb = wb.icon(ic)?;
    }
    let hide_on_close = m.tray.enabled && m.tray.hide_on_close;
    let window = wb.build()?;

    if hide_on_close {
        let w2 = window.clone();
        window.on_window_event(move |event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = w2.hide();
            }
        });
    }

    #[cfg(target_os = "macos")]
    if let Some(ic) = icon.clone() {
        set_dock_icon(&ic);
    }

    if m.tray.enabled && m.permissions >= Tier::Desktop {
        let show = MenuItem::with_id(app, "show", format!("Open {}", m.name), true, None::<&str>)?;
        let about = MenuItem::with_id(app, "about", "About deskhatch", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&show, &about, &PredefinedMenuItem::separator(app)?, &quit])?;
        let mut tb = TrayIconBuilder::with_id("tray")
            .menu(&menu)
            .show_menu_on_left_click(false)
            .tooltip(m.tray.tooltip.clone().unwrap_or_else(|| m.name.clone()))
            .on_menu_event(|app, ev| match ev.id.as_ref() {
                "show" => toggle_main(app, true),
                "about" => show_about(app, app.state::<AppCtx>().manifest.as_ref()),
                "quit" => app.exit(0),
                _ => {}
            })
            .on_tray_icon_event(|tray, ev| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                    toggle_main(tray.app_handle(), false);
                }
            });
        if let Some(ic) = icon.clone().or_else(|| app.default_window_icon().cloned()) {
            tb = tb.icon(ic);
        }
        tb.build(app)?;
    }

    if !m.hotkeys.is_empty() && m.permissions >= Tier::Desktop {
        use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
        let mut table: Vec<(Shortcut, String)> = vec![];
        for (name, accel) in &m.hotkeys {
            match accel.parse::<Shortcut>() {
                Ok(s) => table.push((s, name.clone())),
                Err(e) => eprintln!("deskhatch: bad hotkey {name}={accel}: {e}"),
            }
        }
        let table2 = table.clone();
        app.handle().plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    if let Some((_, name)) = table2.iter().find(|(s, _)| s == shortcut) {
                        if name == "toggle" {
                            toggle_main(app, false);
                        }
                        let _ = app.emit("hatch:hotkey", name.clone());
                    }
                })
                .build(),
        )?;
        for (s, name) in table {
            if let Err(e) = app.global_shortcut().register(s) {
                eprintln!("deskhatch: cannot register hotkey {name}: {e}");
            }
        }
    }

    if dev {
        let entry = dir.join(&m.entry);
        let w = window.clone();
        std::thread::spawn(move || {
            let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
            let mut last = mtime(&entry);
            loop {
                std::thread::sleep(std::time::Duration::from_millis(600));
                let now = mtime(&entry);
                if now != last {
                    last = now;
                    let _ = w.eval("location.reload()");
                }
            }
        });
    }
    Ok(())
}

fn setup_gallery(app: &mut tauri::App) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, "gallery", WebviewUrl::CustomProtocol("hatch://localhost/".parse().unwrap()))
        .title("deskhatch")
        .inner_size(960.0, 680.0)
        .min_inner_size(640.0, 420.0)
        .initialization_script(SHIM_JS)
        .build()?;
    Ok(())
}

fn toggle_main(app: &AppHandle, force_show: bool) {
    if let Some(w) = app.get_webview_window("main") {
        let visible = w.is_visible().unwrap_or(false);
        let focused = w.is_focused().unwrap_or(false);
        if !force_show && visible && focused {
            let _ = w.hide();
        } else {
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
    }
}

pub fn show_about(app: &AppHandle, m: Option<&Manifest>) {
    use tauri_plugin_dialog::DialogExt;
    let text = match m {
        Some(m) => format!(
            "{} {}\n{}\n\nPermissions: {}\n\nMade with deskhatch {} — the open runtime that turns AI-generated HTML into real desktop apps.\nhttps://github.com/beskvladimir-create/deskhatch",
            m.name,
            m.version,
            m.description.clone().unwrap_or_default(),
            m.permissions.label(),
            env!("CARGO_PKG_VERSION")
        ),
        None => format!(
            "deskhatch {}\n\nThe open runtime that turns AI-generated HTML into real desktop apps.\nhttps://github.com/beskvladimir-create/deskhatch",
            env!("CARGO_PKG_VERSION")
        ),
    };
    app.dialog().message(text).title("About").show(|_| {});
}

#[cfg(target_os = "macos")]
fn set_dock_icon(icon: &tauri::image::Image<'_>) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;
    // Re-encode RGBA -> PNG so NSImage can read it.
    let w = icon.width();
    let h = icon.height();
    let Some(buf) = image::RgbaImage::from_raw(w, h, icon.rgba().to_vec()) else { return };
    let mut png = std::io::Cursor::new(Vec::new());
    if image::DynamicImage::ImageRgba8(buf).write_to(&mut png, image::ImageFormat::Png).is_err() {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else { return };
    let data = NSData::with_bytes(&png.into_inner());
    if let Some(img) = NSImage::initWithData(NSImage::alloc(), &data) {
        let ns_app = NSApplication::sharedApplication(mtm);
        unsafe { ns_app.setApplicationIconImage(Some(&img)) };
    }
}

// ---------------------------------------------------------------------------------------------
// hatch:// protocol
// ---------------------------------------------------------------------------------------------

fn csp_for(m: Option<&Manifest>) -> String {
    let tier = m.map(|m| m.permissions).unwrap_or(Tier::Full);
    let mut connect = String::from("ipc: http://ipc.localhost hatch:");
    let mut remote_media = String::new();
    if tier >= Tier::Desktop {
        remote_media.push_str(" https:");
        if let Some(m) = m {
            for h in &m.allowed_hosts {
                if h == "*" {
                    connect.push_str(" https: http:");
                } else if h.contains("://") {
                    connect.push(' ');
                    connect.push_str(h);
                } else {
                    connect.push_str(&format!(" https://{h}"));
                }
            }
        }
    }
    format!(
        "default-src 'self' hatch:; script-src 'self' 'unsafe-inline' 'unsafe-eval' hatch:; style-src 'self' 'unsafe-inline' hatch:; img-src 'self' data: blob: hatch:{rm}; font-src 'self' data: hatch:{rm}; media-src 'self' data: blob: hatch:{rm}; connect-src {connect}; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'",
        rm = remote_media,
        connect = connect
    )
}

fn serve(state: &AppCtx, request: tauri::http::Request<Vec<u8>>) -> tauri::http::Response<Cow<'static, [u8]>> {
    let path = request.uri().path().trim_start_matches('/').to_string();
    let path = percent_decode(&path);
    let csp = csp_for(state.manifest.as_ref());

    let (body, mime): (Vec<u8>, String) = match (&state.manifest, &state.app_dir) {
        (Some(m), Some(dir)) => {
            let rel = if path.is_empty() { m.entry.clone() } else { path.clone() };
            match read_in(dir, &rel) {
                Some(b) => (b, mime_for(&rel)),
                None => return not_found(&csp),
            }
        }
        _ => {
            if path.is_empty() || path == "index.html" {
                (GALLERY_HTML.as_bytes().to_vec(), "text/html".into())
            } else if path.starts_with("icon/") {
                // icon/<id> -> installed app icon
                let id = path.trim_start_matches("icon/");
                match Manifest::load(&store::app_dir(id)).ok().and_then(|m| read_in(&store::app_dir(id), &m.icon)) {
                    Some(b) => (b, "image/png".into()),
                    None => return not_found(&csp),
                }
            } else {
                return not_found(&csp);
            }
        }
    };

    tauri::http::Response::builder()
        .status(200)
        .header("content-type", mime)
        .header("Content-Security-Policy", csp)
        .header("Cache-Control", "no-cache")
        .header("Access-Control-Allow-Origin", "*")
        .body(Cow::Owned(body))
        .unwrap()
}

fn not_found(csp: &str) -> tauri::http::Response<Cow<'static, [u8]>> {
    tauri::http::Response::builder()
        .status(404)
        .header("content-type", "text/plain")
        .header("Content-Security-Policy", csp)
        .body(Cow::Borrowed(b"not found" as &[u8]))
        .unwrap()
}

/// Read `rel` inside `dir`, refusing anything that escapes it.
fn read_in(dir: &Path, rel: &str) -> Option<Vec<u8>> {
    let rel = rel.split('?').next().unwrap_or(rel);
    let rel = rel.split('#').next().unwrap_or(rel);
    if rel.contains("..") {
        return None;
    }
    let p = dir.join(rel);
    let p = if p.is_dir() { p.join("index.html") } else { p };
    let canon = p.canonicalize().ok()?;
    let base = dir.canonicalize().ok()?;
    if !canon.starts_with(&base) {
        return None;
    }
    std::fs::read(canon).ok()
}

fn mime_for(path: &str) -> String {
    mime_guess::from_path(path).first_or_octet_stream().essence_str().to_string()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}
