# Security

deskhatch turns someone else's HTML into a desktop app. Every package, `.hatch` file, folder
and bare page is treated as untrusted code. This document states what the runtime enforces,
what it does not, and how to report a problem.

The supported version is the
[latest release](https://github.com/beskvladimir-create/deskhatch/releases/latest). Fixes go
into the next release from `main`; older versions are not patched. Versions 0.2.3 and earlier
are deprecated on npm.

## Threat model

The runtime is written for one attacker: an app you installed and should not have trusted.

In scope. The runtime is meant to stop an installed app from:

- reaching the network at the `web` tier, or any host outside `allowedHosts` at `desktop`
  and `full`, including this computer and the local network;
- reading credentials, tokens, browser profiles, mail, messenger data and shell histories,
  at any tier, in any letter case and through symlinks;
- writing the known places that run code at login or on a shell start: startup files,
  autostart and systemd entries, LaunchAgents, app menus and launchers;
- using your model key anywhere but the provider and endpoint you configured, and reading
  the key itself;
- escaping its own origin: navigating out of its package, opening windows or frames to
  other sites, using WebRTC, or reading another app's storage;
- harming the install itself: archive bombs, duplicate or escaping paths inside a package,
  overwriting the launcher of another app, taking common keyboard shortcuts, smuggling
  control characters into names and texts shown to you;
- being replaced by a swapped or corrupted runtime download, which the CLI checks by
  SHA-256 before it runs.

Out of scope. The runtime does not defend against:

- an app you granted `full` doing what `full` allows, which is reading and writing your
  home folder outside the deny list;
- a bug in the system webview (WebKit on macOS, WebKitGTK on Linux), on which the isolation
  of every app rests;
- anything already running on your machine with your rights, and anything running as root
  or admin;
- compromise of the publishing accounts. The checksums shipped in the npm package and the
  sums in a release are produced by the same accounts that publish the files, so they
  detect a corrupted or swapped download, not a malicious publisher;
- resource exhaustion by a running app: processor, memory, or filling the disk inside paths
  it is allowed to write;
- fingerprinting and side channels;
- an app at `full` writing a file that another program later executes, for example a
  project's `.git/hooks` or a build file.

## This is not an operating system sandbox

The tiers are enforced by deskhatch's own Rust code. Every call a page makes to the `hatch`
API is checked in-process, and the page's content-security-policy, derived from the tier,
plus a navigation lock keep the page inside its package. On Linux and on macOS 14 and later
the window's connections also go through a local proxy that opens tunnels only to the
allowed hosts.

There is no App Sandbox profile, no seatbelt, no Linux namespace, no seccomp filter. A bug
in deskhatch, or in the webview it uses, is enough to get past a rule, and nothing below
would stop it.

The deny list of sensitive folders has the same shape of limitation. It names the places
credentials and private data are known to live today. It cannot be complete: a tool released
tomorrow may put a token in a path nobody has listed yet, and a `full` app would read it.
The list is in `FORBIDDEN_IN_HOME` in `runtime/src/commands.rs`, and additions are welcome.

Read the tiers as protection against accidents and against ordinary hostile pages, not as a
boundary you would deliberately put untrusted code behind. When you need a real boundary,
run deskhatch in a virtual machine or under a separate user account.

## What `full` means

`full` means you trust the app with your home folder. It reads and writes anything there
except deskhatch's own data, the credential, browser, mail, messenger and history paths in
the deny list, and the startup locations no tier may write.

That is still enough to read your projects, your documents and your notes, and to change
files other programs run later. Grant `full` only to apps you wrote, or whose HTML you read.
The HTML of every installed app sits in `~/.deskhatch/apps/<id>/index.html` and is plain text.

`web` is the tier to prefer and the default for generated apps: no network, no files, no
notifications, no tray, no hotkeys. `desktop` adds notifications, tray and hotkeys, the hosts
in `allowedHosts`, and only the files and folders you pick yourself. The full table is in
[SPEC.md](SPEC.md#permission-tiers), and the gallery shows the tier of every installed app.

Installing a package that asks for `desktop` or `full`, global hotkeys or a model endpoint,
or that would replace an installed app from another source, shows what it asks for and waits
for your answer. Without a terminal and without the dialog, the answer is no.

## Verifying what you run

- Release assets are listed with their SHA-256 in `SHA256SUMS.txt` on the release page.
- The npm package ships `cli/checksums.json`, and `npx deskhatch` verifies the runtime it
  downloads against it, falling back to the release's `SHA256SUMS.txt`.
- `DESKHATCH_RUNTIME_URL` is refused unless `DESKHATCH_RUNTIME_SHA256` is set, before
  anything is downloaded.
- The macOS runtime in Releases is signed with a Developer ID certificate and notarised by
  Apple. `spctl -a -t exec Deskhatch.app` reports `Notarized Developer ID`, and
  `codesign -dv --verbose=4 Deskhatch.app` shows the team identifier.

## Reporting a vulnerability

Use "Report a vulnerability" on the
[Security tab](https://github.com/beskvladimir-create/deskhatch/security/advisories/new)
of this repository. That opens a private advisory visible only to the maintainer.

Please do not open a public issue for a vulnerability, and please do not post a working
exploit in a public thread.

Useful in a report: the deskhatch version, the operating system and its version, the tier of
the app, a minimal package or HTML that reproduces the problem, and what you expected the
runtime to prevent. A first reply comes within seven days. There is no bounty programme.

Reports that a `full` app read or wrote something inside the home folder, outside the deny
list, are expected behaviour rather than vulnerabilities. A path that belongs in the deny
list and is missing from it is worth reporting, and is handled as an ordinary issue.
