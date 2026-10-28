# hozd

A terminal HTTP client in the spirit of cURL, with a lazygit/lazydocker-style TUI.
Built on [reqwest](https://docs.rs/reqwest) and [ratatui](https://ratatui.rs).

```
+-----------+-----------------------------+-----------+
| Stored    |  [Method] [URL ............]  | Response  |
| History   |  Body (JSON)                 | Headers   |
|  requests |  { "name": "ada" }           | Body      |
+-----------+-----------------------------+-----------+
     3                    6                      3
```

- **Left**: `Stored` templates and `History`. Moving over an entry previews it: the center
  panel shows its request, the right panel its response (history only). Nothing is sent.
- **Center**: HTTP method, URL and a JSON body editor with syntax highlighting.
- **Right**: response status, headers and body.

## Install

```sh
curl --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/LeoBorai/hozd/main/install.sh | sh
```

Installs the latest release for Linux (glibc) or macOS, x86_64 or aarch64, into
`~/.local/bin`, after checking its SHA-256. Options, as environment variables:

- `HOZD_VERSION`: a release tag such as `v0.1.0` (default: the latest)
- `HOZD_INSTALL_DIR`: where to put the binary (default: `~/.local/bin`)

On Windows download the `.zip` from the releases page. Elsewhere (for example Alpine),
build from source: `cargo install --git https://github.com/LeoBorai/hozd hozd`.

## Data: `~/.hozd`

Override the location with `HOZD_HOME`.

```
~/.hozd/
  config.toml          optional settings
  db.sqlite3           request history (SQLite, migrated automatically on startup)
  projects/<name>/**/*.toml   request templates
```

A database written by a newer hozd is refused with a message to upgrade; hozd never
modifies it.

### Templates

One request per `.toml` file, anywhere under `projects/<name>/`. `body` must come
before any `[table]`:

```toml
name = "Create user"
method = "POST"
url = "https://api.example.com/users"
body = '''
{ "name": "ada" }
'''

[headers]
Content-Type = "application/json"
```

## Keys

| Key               | Action                                   |
| ----------------- | ---------------------------------------- |
| `Tab` / `S-Tab`   | Cycle focus: list, URL, body             |
| `Esc`             | Back to the list                         |
| `[` `]` / `h` `l` | Switch Stored / History tab (list)       |
| `j` `k` / arrows  | Move selection (list)                    |
| `Enter`           | Load the selected entry into the editor (list); send (URL field) |
| `Ctrl+S`          | Send the request                         |
| `Ctrl+N` / `Ctrl+P` | Next / previous HTTP method            |
| `PgUp` / `PgDn`   | Scroll the response body                 |
| `y` (list focus)  | Copy the response body to the clipboard  |
| `q` / `Ctrl+C`    | Quit                                     |

### Mouse

| Click                          | Action                                              |
| ------------------------------ | --------------------------------------------------- |
| `Stored` / `History` tab       | Switch tab                                          |
| List item                      | Select and preview; click again to load into editor |
| Method box                     | Next method (right-click: previous)                 |
| URL / body                     | Focus the editor and place the cursor               |
| Wheel over the list            | Move the selection                                  |
| Wheel over the response body   | Scroll                                              |
| Right-click the response body  | Copy it to the clipboard                            |

Clicking the editor while previewing an entry loads that entry first, so you edit what
you were looking at.

Headers of a loaded template or history entry are carried along and sent, but cannot be
edited in the TUI yet.

## Development

Needs Rust (edition 2024), [`cargo-nextest`](https://nexte.st), [`sqlx-cli`](https://crates.io/crates/sqlx-cli)
[`cargo-deny`](https://crates.io/crates/cargo-deny) and [`just`](https://just.systems).

```sh
just test              # cargo nextest run
just fmt               # clippy --fix + rustfmt
just run               # run hozd
just deny              # cargo-deny: advisories, licenses, bans, sources
just deps-update       # cargo update, then cargo deny
just db-new-migration add_something   # new numbered migration in migrations/
just db-prepare        # refresh the committed .sqlx/ query metadata
```

SQL queries are checked at compile time. Builds use the committed `.sqlx/` metadata
(`SQLX_OFFLINE=true`); after changing a query or a migration run `just db-prepare` and
commit `.sqlx/`. Never edit a released migration: add a new one.

### Copying

hozd captures the mouse, so the terminal's own text selection is off by default; hold
`Shift` (`Option` in iTerm2, `Fn` in some terminals) while dragging to select text natively.
To copy the whole response body use `y` or right-click it. The text (pretty-printed when
it is JSON) goes to the clipboard with the OSC 52 escape sequence, which works over SSH
and inside tmux (needs `set -g set-clipboard on`). Terminals without OSC 52 support
(for example macOS Terminal.app) ignore it; use Shift+drag there.
