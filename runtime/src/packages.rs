//! Importing, exporting and installing app packages, and creating OS launchers.

use crate::manifest::{slugify, Manifest};
use crate::store;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum PkgError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Manifest(#[from] crate::manifest::ManifestError),
    #[error("{0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, PkgError>;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Installed {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub permissions: String,
    pub dir: PathBuf,
    pub icon: Option<PathBuf>,
    pub launcher: Option<PathBuf>,
}

/// Import a source into ~/.deskhatch/apps/<id>. Source may be:
/// * a directory containing app.json (+ index.html)
/// * a .hatch zip file
/// * a bare .html file (a manifest is synthesised; permissions = web)
pub fn import(source: &Path, name_override: Option<&str>) -> Result<Manifest> {
    store::ensure_dirs()?;
    let src = source
        .canonicalize()
        .map_err(|_| PkgError::Other(format!("not found: {}", source.display())))?;

    if src.is_dir() {
        let m = Manifest::load(&src)?;
        let dest = store::app_dir(&m.id);
        if dest.exists() && dest.canonicalize().ok().as_deref() != Some(&src) {
            fs::remove_dir_all(&dest)?;
        }
        if dest.canonicalize().ok().as_deref() != Some(&src) {
            copy_dir(&src, &dest)?;
        }
        ensure_icon(&dest, &m);
        return Ok(m);
    }

    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if ext == "hatch" || ext == "zip" {
        let tmp = store::root().join("tmp").join(format!("import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp)?;
        unzip_to(&src, &tmp)?;
        // Allow the archive to contain a single top-level folder.
        let mut root = tmp.clone();
        if !root.join("app.json").exists() {
            let entries: Vec<_> = fs::read_dir(&root)?.filter_map(|e| e.ok()).collect();
            if entries.len() == 1 && entries[0].path().is_dir() {
                root = entries[0].path();
            }
        }
        let m = Manifest::load(&root)?;
        let dest = store::app_dir(&m.id);
        if dest.exists() {
            fs::remove_dir_all(&dest)?;
        }
        copy_dir(&root, &dest)?;
        let _ = fs::remove_dir_all(&tmp);
        ensure_icon(&dest, &m);
        return Ok(m);
    }

    if ext == "html" || ext == "htm" {
        let stem = name_override
            .map(|s| s.to_string())
            .or_else(|| {
                src.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.replace(['-', '_'], " "))
            })
            .unwrap_or_else(|| "App".into());
        let title = html_title(&fs::read_to_string(&src)?).unwrap_or(stem);
        let mut m = Manifest::for_html(&title);
        if let Some(n) = name_override {
            m.name = n.to_string();
            m.id = slugify(n);
        }
        let dest = store::app_dir(&m.id);
        if dest.exists() {
            fs::remove_dir_all(&dest)?;
        }
        fs::create_dir_all(&dest)?;
        fs::copy(&src, dest.join("index.html"))?;
        fs::write(dest.join("app.json"), serde_json::to_vec_pretty(&m).unwrap())?;
        write_default_icon(&dest.join("icon.png"), &m.name)?;
        return Ok(m);
    }

    Err(PkgError::Other(format!(
        "unsupported source {} (expected a folder with app.json, a .hatch file or an .html file)",
        src.display()
    )))
}

/// A package may ship without the icon its manifest names. Draw a default one instead of
/// leaving the app with the system placeholder.
fn ensure_icon(dir: &Path, m: &Manifest) {
    let icon = dir.join(&m.icon);
    if icon.exists() {
        return;
    }
    if let Some(parent) = icon.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Err(e) = write_default_icon(&icon, &m.name) {
        eprintln!("deskhatch: could not draw a default icon: {e}");
    }
}

fn html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let s = lower.find("<title")?;
    let s = lower[s..].find('>')? + s + 1;
    let e = lower[s..].find("</title>")? + s;
    let t = html[s..e].trim();
    if t.is_empty() {
        None
    } else {
        Some(t.chars().take(60).collect())
    }
}

pub fn list() -> Vec<Installed> {
    let mut out = vec![];
    if let Ok(rd) = fs::read_dir(store::apps_dir()) {
        for e in rd.flatten() {
            let dir = e.path();
            if let Ok(m) = Manifest::load(&dir) {
                let icon = dir.join(&m.icon);
                out.push(Installed {
                    launcher: launcher_path(&m).filter(|p| p.exists()),
                    id: m.id,
                    name: m.name,
                    version: m.version,
                    description: m.description,
                    permissions: m.permissions.label().into(),
                    icon: icon.exists().then_some(icon),
                    dir,
                });
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

pub fn uninstall(id: &str) -> Result<()> {
    let dir = store::app_dir(id);
    if let Ok(m) = Manifest::load(&dir) {
        if let Some(l) = launcher_path(&m) {
            if l.is_dir() {
                let _ = fs::remove_dir_all(&l);
            } else {
                let _ = fs::remove_file(&l);
            }
        }
        #[cfg(target_os = "linux")]
        {
            let _ = fs::remove_file(linux_icon_path(&m.id));
        }
    }
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    let _ = fs::remove_file(store::grants_file(id));
    let _ = fs::remove_dir_all(store::data_dir(id));
    Ok(())
}

/// Zip an installed app into `<out_dir>/<id>.hatch`. Returns the archive path.
pub fn export(id: &str, out_dir: &Path) -> Result<PathBuf> {
    let dir = store::app_dir(id);
    let m = Manifest::load(&dir)?;
    fs::create_dir_all(out_dir)?;
    let out = out_dir.join(format!("{}.hatch", m.id));
    let file = fs::File::create(&out)?;
    let mut zw = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for entry in walk(&dir)? {
        let rel = entry.strip_prefix(&dir).unwrap();
        let rel_s = rel.to_string_lossy().replace('\\', "/");
        if entry.is_dir() {
            zw.add_directory(format!("{rel_s}/"), opts)?;
        } else {
            zw.start_file(rel_s, opts)?;
            let mut f = fs::File::open(&entry)?;
            let mut buf = vec![];
            f.read_to_end(&mut buf)?;
            zw.write_all(&buf)?;
        }
    }
    zw.finish()?;
    Ok(out)
}

// ---------- launchers ----------

/// Path of the OS launcher that would be created for this app.
pub fn launcher_path(m: &Manifest) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let apps = dirs::home_dir()?.join("Applications");
        Some(apps.join(format!("{}.app", safe_name(&m.name))))
    }
    #[cfg(target_os = "linux")]
    {
        Some(
            dirs::data_local_dir()?
                .join("applications")
                .join(format!("deskhatch-{}.desktop", m.id)),
        )
    }
    #[cfg(target_os = "windows")]
    {
        let _ = m;
        None
    }
}

#[allow(dead_code)]
fn safe_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if "/\\:*?\"<>|".contains(c) { ' ' } else { c })
        .collect();
    s.trim().to_string()
}

/// Info.plist for a macOS launcher bundle. `executable` is the file in Contents/MacOS that
/// macOS starts: the thin shell script, or the copied runtime binary in a standalone bundle.
#[cfg(target_os = "macos")]
fn mac_plist(m: &Manifest, executable: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>{name}</string>
  <key>CFBundleDisplayName</key><string>{name}</string>
  <key>CFBundleIdentifier</key><string>tech.besk.deskhatch.app.{id}</string>
  <key>CFBundleVersion</key><string>{version}</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>{executable}</string>
  <key>CFBundleIconFile</key><string>app.icns</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSUIElement</key><false/>
</dict>
</plist>
"#,
        name = xml_escape(&m.name),
        id = xml_escape(&m.id),
        version = xml_escape(&m.version),
        executable = executable,
    )
}

/// The .app bundle this binary runs from, when it runs from one at all.
#[cfg(target_os = "macos")]
fn runtime_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let looks_right = macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension().map(|e| e == "app").unwrap_or(false)
        && contents.join("Info.plist").exists();
    looks_right.then(|| bundle.to_path_buf())
}

/// True when this app's launcher is a standalone copy of the runtime rather than a thin script.
pub fn launcher_is_standalone(m: &Manifest) -> bool {
    match launcher_path(m) {
        Some(p) => p.join("Contents").join("Resources").join(APP_ID_MARKER).exists(),
        None => false,
    }
}

/// Name of the file inside a standalone bundle that tells the runtime which app to host.
pub const APP_ID_MARKER: &str = "deskhatch-app";

/// Create a launcher, either the thin shared one or a standalone bundle with its own identity.
pub fn create_launcher_for(m: &Manifest, standalone: bool) -> Result<PathBuf> {
    if standalone {
        standalone_launcher(m)
    } else {
        create_launcher(m)
    }
}

/// A full copy of the runtime bundle carrying the app's own name, identifier and icon, so that
/// the menu bar, the dock, the app switcher and the notification settings all show the app
/// instead of "Deskhatch". Costs one runtime copy (~10 MB) per app.
#[cfg(target_os = "macos")]
fn standalone_launcher(m: &Manifest) -> Result<PathBuf> {
    let bundle = runtime_bundle().ok_or_else(|| {
        PkgError::Other(
            "a standalone app needs the bundled runtime (Deskhatch.app); this binary does not run from one".into(),
        )
    })?;
    let dir = store::app_dir(&m.id);
    let icon_png = dir.join(&m.icon);
    let target = launcher_path(m)
        .ok_or_else(|| PkgError::Other("launchers are not supported on this OS yet".into()))?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    // Build beside the target and swap at the end. Rebuilding in place would leave the bundle
    // missing for a while, and the app may well be running from it right now.
    let staging = target.with_extension("app.new");
    let _ = fs::remove_dir_all(&staging);
    copy_dir(&bundle, &staging)?;
    let contents = staging.join("Contents");
    let resources = contents.join("Resources");
    fs::create_dir_all(&resources)?;
    // The copy must not inherit the runtime's identity, its .hatch file association or its icon.
    let _ = fs::remove_dir_all(contents.join("_CodeSignature"));
    let _ = fs::remove_file(resources.join("icon.icns"));
    fs::write(contents.join("Info.plist"), mac_plist(m, "deskhatch"))?;
    fs::write(resources.join(APP_ID_MARKER), format!("{}\n", m.id))?;
    if let Err(e) = write_icns(&icon_png, &resources.join("app.icns")) {
        eprintln!("deskhatch: icon skipped: {e}");
    }
    // Info.plist and the resources changed, so the inherited signature no longer matches.
    // Sign the copy ad-hoc; it is created locally, so Gatekeeper does not evaluate it.
    match std::process::Command::new("codesign")
        .args(["--force", "--deep", "--sign", "-"])
        .arg(&staging)
        .output()
    {
        Ok(o) if !o.status.success() => eprintln!(
            "deskhatch: could not ad-hoc sign {}: {}",
            staging.display(),
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => eprintln!("deskhatch: codesign is not available: {e}"),
        _ => {}
    }
    let previous = target.with_extension("app.old");
    let _ = fs::remove_dir_all(&previous);
    if target.exists() {
        fs::rename(&target, &previous)?;
    }
    fs::rename(&staging, &target)?;
    let _ = fs::remove_dir_all(&previous);
    // Nudge LaunchServices so Finder/Dock picks up the new bundle.
    let _ = std::process::Command::new("touch").arg(&target).status();
    Ok(target)
}

#[cfg(not(target_os = "macos"))]
fn standalone_launcher(m: &Manifest) -> Result<PathBuf> {
    eprintln!("deskhatch: standalone apps are macOS-only; creating the usual launcher");
    create_launcher(m)
}

/// Create a launcher (dock/menu icon) that starts this runtime with `--app <id>`.
pub fn create_launcher(m: &Manifest) -> Result<PathBuf> {
    let exe = crate::self_exe()?;
    let dir = store::app_dir(&m.id);
    let icon_png = dir.join(&m.icon);
    let target = launcher_path(m)
        .ok_or_else(|| PkgError::Other("launchers are not supported on this OS yet".into()))?;

    #[cfg(target_os = "macos")]
    {
        if target.exists() {
            fs::remove_dir_all(&target)?;
        }
        let contents = target.join("Contents");
        fs::create_dir_all(contents.join("MacOS"))?;
        fs::create_dir_all(contents.join("Resources"))?;
        fs::write(contents.join("Info.plist"), mac_plist(m, "launch"))?;
        let script = format!(
            "#!/bin/sh\nexec {} --app {} \"$@\"\n",
            shell_quote(&exe.to_string_lossy()),
            shell_quote(&m.id)
        );
        let launch = contents.join("MacOS").join("launch");
        fs::write(&launch, script)?;
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&launch, fs::Permissions::from_mode(0o755))?;
        }
        let icns = contents.join("Resources").join("app.icns");
        match write_icns(&icon_png, &icns) {
            Ok(()) => {}
            Err(e) => eprintln!("deskhatch: icon skipped: {e}"),
        }
        // Nudge LaunchServices so Finder/Dock picks up the new bundle.
        let _ = std::process::Command::new("touch").arg(&target).status();
        return Ok(target);
    }

    #[cfg(target_os = "linux")]
    {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let icon_dst = linux_icon_path(&m.id);
        if let Some(p) = icon_dst.parent() {
            fs::create_dir_all(p)?;
        }
        let icon_field = if icon_png.exists() {
            fs::copy(&icon_png, &icon_dst)?;
            icon_dst.to_string_lossy().to_string()
        } else {
            "application-x-executable".into()
        };
        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName={name}\nComment={comment}\nExec={exe} --app {id}\nIcon={icon}\nTerminal=false\nCategories=Utility;\nStartupWMClass=deskhatch-{id}\n",
            name = m.name,
            comment = m.description.clone().unwrap_or_default(),
            exe = shell_quote(&exe.to_string_lossy()),
            id = m.id,
            icon = icon_field,
        );
        fs::write(&target, desktop)?;
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;
        }
        let _ = std::process::Command::new("update-desktop-database")
            .arg(target.parent().unwrap())
            .status();
        return Ok(target);
    }

    #[allow(unreachable_code)]
    Err(PkgError::Other("launchers are not supported on this OS yet".into()))
}

#[cfg(target_os = "linux")]
pub fn linux_icon_path(id: &str) -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("icons/hicolor/512x512/apps")
        .join(format!("deskhatch-{id}.png"))
}

#[allow(dead_code)]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Write an .icns file with PNG-encoded entries (128, 256, 512, 1024 px) from any PNG.
#[allow(dead_code)]
pub fn write_icns(png: &Path, out: &Path) -> Result<()> {
    let img = image::open(png).map_err(|e| PkgError::Other(e.to_string()))?;
    let sizes: [(u32, &[u8; 4]); 4] = [
        (128, b"ic07"),
        (256, b"ic08"),
        (512, b"ic09"),
        (1024, b"ic10"),
    ];
    let mut entries: Vec<u8> = vec![];
    for (px, tag) in sizes {
        let resized = img.resize_exact(px, px, image::imageops::FilterType::Lanczos3);
        let mut buf = std::io::Cursor::new(Vec::new());
        resized
            .write_to(&mut buf, image::ImageFormat::Png)
            .map_err(|e| PkgError::Other(e.to_string()))?;
        let data = buf.into_inner();
        entries.extend_from_slice(tag);
        entries.extend_from_slice(&((data.len() + 8) as u32).to_be_bytes());
        entries.extend_from_slice(&data);
    }
    let mut file = vec![];
    file.extend_from_slice(b"icns");
    file.extend_from_slice(&((entries.len() + 8) as u32).to_be_bytes());
    file.extend_from_slice(&entries);
    fs::write(out, file)?;
    Ok(())
}

/// A simple generated icon: coloured rounded square with the first letter.
pub fn write_default_icon(out: &Path, name: &str) -> Result<()> {
    use image::{Rgba, RgbaImage};
    let size = 512u32;
    let letter = name.chars().next().unwrap_or('A').to_ascii_uppercase();
    // deterministic hue from the name
    let h = name.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32)) % 360;
    let (r, g, b) = hsl_to_rgb(h as f32, 0.55, 0.45);
    let mut img = RgbaImage::new(size, size);
    let radius = 110.0f32;
    for y in 0..size {
        for x in 0..size {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let inside = rounded_rect(fx, fy, size as f32, radius);
            if inside {
                img.put_pixel(x, y, Rgba([r, g, b, 255]));
            } else {
                img.put_pixel(x, y, Rgba([0, 0, 0, 0]));
            }
        }
    }
    draw_letter(&mut img, letter);
    img.save(out).map_err(|e| PkgError::Other(e.to_string()))?;
    Ok(())
}

fn rounded_rect(x: f32, y: f32, s: f32, r: f32) -> bool {
    let pad = 24.0;
    let (x0, y0, x1, y1) = (pad, pad, s - pad, s - pad);
    if x < x0 || x > x1 || y < y0 || y > y1 {
        return false;
    }
    let cx = x.clamp(x0 + r, x1 - r);
    let cy = y.clamp(y0 + r, y1 - r);
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (
        ((r1 + m) * 255.0) as u8,
        ((g1 + m) * 255.0) as u8,
        ((b1 + m) * 255.0) as u8,
    )
}

/// Tiny 5x7 bitmap font for A-Z, 0-9; enough for a placeholder icon.
fn draw_letter(img: &mut image::RgbaImage, ch: char) {
    let glyph = font5x7(ch);
    let cell = 44u32; // pixel size of one font cell
    let w = 5 * cell;
    let h = 7 * cell;
    let ox = (img.width() - w) / 2;
    let oy = (img.height() - h) / 2;
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..5 {
            if bits & (0b10000 >> col) != 0 {
                for dy in 0..cell {
                    for dx in 0..cell {
                        img.put_pixel(
                            ox + col * cell + dx,
                            oy + row as u32 * cell + dy,
                            image::Rgba([255, 255, 255, 255]),
                        );
                    }
                }
            }
        }
    }
}

fn font5x7(ch: char) -> [u8; 7] {
    match ch {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        _ => [0x00, 0x00, 0x0A, 0x00, 0x11, 0x0E, 0x00],
    }
}

// ---------- fs helpers ----------

fn walk(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d)? {
            let p = e?.path();
            if p.file_name().and_then(|n| n.to_str()) == Some(".DS_Store") {
                continue;
            }
            if p.is_dir() {
                stack.push(p.clone());
            }
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for e in fs::read_dir(src)? {
        let e = e?;
        let p = e.path();
        let t = dst.join(e.file_name());
        if p.is_dir() {
            copy_dir(&p, &t)?;
        } else {
            fs::copy(&p, &t)?;
        }
    }
    Ok(())
}

fn unzip_to(archive: &Path, dest: &Path) -> Result<()> {
    let f = fs::File::open(archive)?;
    let mut z = zip::ZipArchive::new(f)?;
    let total: u64 = (0..z.len()).map(|i| z.by_index(i).map(|e| e.size()).unwrap_or(0)).sum();
    if total > 200 * 1024 * 1024 {
        return Err(PkgError::Other("package larger than 200 MB".into()));
    }
    for i in 0..z.len() {
        let mut entry = z.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            continue; // zip-slip protection
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out)?;
        } else {
            if let Some(p) = out.parent() {
                fs::create_dir_all(p)?;
            }
            let mut w = fs::File::create(&out)?;
            std::io::copy(&mut entry, &mut w)?;
        }
    }
    Ok(())
}
