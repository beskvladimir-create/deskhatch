# The `.hatch` app package — format specification v1

A deskhatch app is a folder (or a zip of that folder with the `.hatch` extension) containing:

```
my-app/
  app.json      manifest (required)
  index.html    entry page (required; name configurable via "entry")
  icon.png      512×512 or 1024×1024 PNG (optional; a placeholder is generated if missing)
  assets/…      anything the page references with relative URLs (optional)
```

Everything is served to the page from an origin of its own, `hatch://app-<id>/` (the gallery uses `hatch://gallery/`), so apps never share localStorage, IndexedDB or caches. On Linux each app also gets its own web-data folder. Relative URLs work; `..` and links that point outside the package are refused.

## app.json

```jsonc
{
  "schema": 1,
  "id": "pomodoro",               // [a-z0-9-], 1–64 chars; unique on the machine
  "name": "Pomodoro",             // window title, launcher name
  "version": "1.0.0",
  "description": "25/5 focus timer",
  "icon": "icon.png",             // default "icon.png"
  "entry": "index.html",          // default "index.html"
  "window": {                     // all optional
    "width": 420, "height": 560,
    "minWidth": 340, "minHeight": 460,
    "resizable": true, "alwaysOnTop": false, "decorations": true
  },
  "permissions": "desktop",       // "web" (default) | "desktop" | "full"
  "tray": { "enabled": true, "tooltip": "Pomodoro", "hideOnClose": true },
  "hotkeys": { "toggle": "CmdOrCtrl+Shift+P", "startStop": "CmdOrCtrl+Shift+O" },
  "llm": {                        // defaults for hatch.llm.chat(); all optional
    "provider": "ollama",         // ollama | openai | openrouter | groq | mistral | deepseek | lmstudio | llamacpp | <custom>
    "model": "qwen2.5-coder:7b",
    "endpoint": "http://localhost:11434/v1",
    "system": "You are…"
  },
  "allowedHosts": ["api.example.com"],   // desktop/full only: hosts the page may fetch() directly; "*" = any https host
  "author": "…", "homepage": "https://…"
}
```

Precedence for model settings: `app.json` → user's `deskhatch config` for that provider → built-in defaults. Exception: `llm.endpoint` from `app.json` is used only when it equals an endpoint the user configured (or the provider's built-in one); otherwise it is ignored. `llm.provider` is honoured only for providers the user set up. The API key is sent only to an endpoint the user configured or to the provider's built-in endpoint.
API keys are never in the package; they live in the OS keychain (`deskhatch key <provider>`) or in `DESKHATCH_API_KEY[_<PROVIDER>]`.

## Permission tiers

| | `web` | `desktop` | `full` |
|---|---|---|---|
| Page network (fetch/XHR/websocket) | none | only `allowedHosts` | only `allowedHosts` |
| `hatch.llm.*` (model via runtime) | ✓ | ✓ | ✓ |
| `hatch.store.*` (per-app JSON store) | ✓ | ✓ | ✓ |
| `hatch.notify`, tray, hotkeys | – | ✓ | ✓ |
| `hatch.openUrl` | – | asks the user unless the host is in `allowedHosts` | same |
| Remote images, fonts, media | none | only `allowedHosts` | only `allowedHosts` |
| `hatch.fs.*` | – | folders picked via `pickFolder()`, single files picked via `pickFile()` | anything under the home folder, plus what the user picked |
| Navigation and `window.open` | own package only | own package only | own package only |
| `~/.deskhatch`, credentials, browser profiles, mail and messenger data (see `FORBIDDEN_IN_HOME` in runtime/src/commands.rs) | never | never | never |

The tier is enforced in the runtime (Rust), not in the page: a page cannot grant itself anything. The content-security-policy of the page is derived from the tier, so a `web` app cannot load remote scripts, images or fonts and cannot open sockets. The content-security-policy does not cover navigation, so the runtime also refuses any navigation outside the package and every `window.open`; external links go through `hatch.openUrl`. The gallery shows the tier of every installed app.

## The `hatch` API (injected into every page)

```js
await hatch.ready                                   // {id, name, version, permissions, platform, llm:{provider,model,endpoint,has_key}, hotkeys, dev}
hatch.app                                           // same object, after ready

await hatch.llm.chat("prompt")                      // → string
await hatch.llm.chat([{role,content},…], {model, temperature, maxTokens, system, json})
await hatch.llm.chatFull(…)                         // → {text, model, provider, usage}
await hatch.llm.models()                            // → [ids]

await hatch.store.get(key) / set(key, value) / remove(key) / all()   // JSON values, 20 MB per app

await hatch.notify(title, body?)                    // desktop+
await hatch.openUrl("https://…")                    // desktop+
await hatch.fs.pickFolder(title?) / pickFile(title?)   // desktop+, grants that folder to this app, returns path or null
await hatch.fs.grants()                             // folders this app may use
await hatch.fs.list(path) / readText(path) / writeText(path, text) / exists(path) / mkdir(path) / remove(path)

hatch.window.setTitle(t) / minimize() / hide() / show() / toggle() / setAlwaysOnTop(bool)
hatch.on("hotkey", name => …)                       // names from app.json "hotkeys"
hatch.about() / hatch.quit()
```

`window.llm` is an alias of `hatch.llm`. Errors are thrown as strings.

## Where things live

```
~/.deskhatch/
  apps/<id>/          installed packages (what the page sees)
  data/<id>/store.json
  grants/<id>.json    folders granted to the app
  config.json         default provider / endpoints / models
  runtime/<version>/  the runtime downloaded by `npx deskhatch`
```

Launchers: `~/Applications/<Name>.app` on macOS, `~/.local/share/applications/deskhatch-<id>.desktop` on Linux. They start the shared runtime with `--app <id>`; nothing is compiled per app.

`install --standalone` (macOS) builds the launcher differently: the whole runtime bundle is copied
to `~/Applications/<Name>.app`, its `Info.plist` gets the app's name, version, icon and the
identifier `tech.besk.deskhatch.app.<id>`, the app id is written to
`Contents/Resources/deskhatch-app`, and the copy is signed ad-hoc. Started without arguments, the
runtime reads that file and hosts the app named in it. Such a bundle has its own identity for the
menu bar, the dock, the app switcher, notification settings and the permission prompts, and it
costs one runtime copy per app. `deskhatch relink` rebuilds every launcher, keeping each app in the
mode it already had, which is what `npx deskhatch update` calls after fetching a new runtime.

Only one process hosts a given app: the pid is kept in `~/.deskhatch/data/<id>/run.pid`, and a
second start exits with a message instead of opening a second window onto the same store.

## Sharing

`deskhatch export <id>` (or **Share .hatch** in the gallery) zips the app folder into `<id>.hatch`. The recipient installs it with `npx deskhatch install file.hatch`, by double-clicking it (file association), or via **Install from file…** in the gallery. The package carries no keys and no user data.
