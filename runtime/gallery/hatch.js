// window.hatch — injected into every deskhatch window before the page runs.
(() => {
  if (window.hatch) return;
  const T = () => window.__TAURI__;
  const inv = (cmd, args) => T().core.invoke(cmd, args || {});

  const hatch = {
    version: '0.1',
    app: null,
    /** resolves to app info: {id, name, version, permissions, platform, llm, hotkeys, dev} */
    ready: null,

    llm: {
      /** chat(messagesOrPrompt, {model, temperature, maxTokens, system, json}) -> string */
      chat: (messages, options) => inv('llm_chat', { messages, options: normOpts(options) }).then((r) => r.text),
      /** same as chat() but returns {text, model, provider, usage} */
      chatFull: (messages, options) => inv('llm_chat', { messages, options: normOpts(options) }),
      models: () => inv('llm_models'),
    },

    store: {
      get: (key) => inv('store_get', { key }),
      set: (key, value) => inv('store_set', { key, value: value === undefined ? null : value }),
      remove: (key) => inv('store_set', { key, value: null }),
      all: () => inv('store_all'),
    },

    notify: (title, body) => inv('notify', { title, body: body == null ? null : String(body) }),
    openUrl: (url) => inv('open_url', { url }),
    about: () => inv('about'),
    quit: () => inv('quit'),

    fs: {
      pickFolder: (title) => inv('fs_pick_folder', { title: title || null }),
      pickFile: (title) => inv('fs_pick_file', { title: title || null }),
      grants: () => inv('fs_grants'),
      list: (path) => inv('fs_list', { path }),
      readText: (path) => inv('fs_read_text', { path }),
      writeText: (path, contents) => inv('fs_write_text', { path, contents: String(contents) }),
      exists: (path) => inv('fs_exists', { path }),
      mkdir: (path) => inv('fs_mkdir', { path }),
      remove: (path) => inv('fs_remove', { path }),
    },

    window: {
      toggle: () => inv('window_toggle'),
      setTitle: (t) => T().window.getCurrentWindow().setTitle(String(t)),
      minimize: () => T().window.getCurrentWindow().minimize(),
      hide: () => T().window.getCurrentWindow().hide(),
      show: () => T().window.getCurrentWindow().show(),
      setAlwaysOnTop: (v) => T().window.getCurrentWindow().setAlwaysOnTop(!!v),
    },

    /** on('hotkey', name => ...), on('refresh', () => ...) */
    on: (event, fn) => T().event.listen('hatch:' + event, (e) => fn(e.payload)),

    // gallery-only (rejected in app windows)
    _gallery: {
      list: () => inv('gallery_list'),
      open: (id) => inv('gallery_open', { id }),
      uninstall: (id) => inv('gallery_uninstall', { id }),
      import: () => inv('gallery_import'),
      share: (id) => inv('gallery_share', { id }),
      launcher: (id) => inv('gallery_launcher', { id }),
      reveal: (id) => inv('gallery_reveal', { id }),
      create: (prompt, name, tier) => inv('gallery_new', { prompt, name: name || null, tier: tier || 'web' }),
      configGet: () => inv('config_get'),
      configSet: (config) => inv('config_set', { config }),
      keySet: (provider, key) => inv('key_set', { provider, key }),
      keyHas: (provider) => inv('key_has', { provider }),
    },
  };

  function normOpts(o) {
    if (!o) return null;
    return {
      model: o.model ?? null,
      temperature: o.temperature ?? null,
      max_tokens: o.maxTokens ?? o.max_tokens ?? null,
      system: o.system ?? null,
      json: !!o.json,
    };
  }

  hatch.ready = inv('app_info').then((info) => {
    hatch.app = info;
    document.documentElement.dataset.hatch = info.permissions;
    return info;
  }).catch(() => null);

  window.hatch = hatch;
  window.llm = hatch.llm;
})();
