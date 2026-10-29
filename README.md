# hozd

A terminal HTTP and gRPC client in the spirit of cURL and grpcurl, with a
lazygit/lazydocker-style TUI. Built on [reqwest](https://docs.rs/reqwest),
[tonic](https://docs.rs/tonic) and [ratatui](https://ratatui.rs).

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
- **Center**: HTTP method, URL, and a box with two tabs: **Body** (JSON editor with syntax
  highlighting) and **Headers** (name and value inputs, one row per header).
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

A template without `kind` is an HTTP template. Add `kind = "grpc"` for a gRPC one:

```toml
name = "Get user"
kind = "grpc"
address = "http://localhost:50051"
method = "user.v1.UserService/GetUser"
reflection = true
body = '''
{ "id": 7 }
'''

[metadata]
authorization = "Bearer token"
```

| Key          | Meaning                                                                      |
| ------------ | ---------------------------------------------------------------------------- |
| `address`    | Server address with a scheme: `http://` is plaintext, `https://` is TLS      |
| `method`     | `package.Service/Method`                                                     |
| `reflection` | `true` to ask the server for its schema (`v1`, falling back to `v1alpha`)    |
| `schema`     | Alternative to `reflection`: path of a `.proto` file or a compiled descriptor set (`.bin`, `.pb`, `.desc`), relative to the template file |
| `insecure`   | `true` to skip TLS certificate verification (self-signed development servers) |
| `[metadata]` | gRPC request metadata, like HTTP headers                                     |
| `body`       | A JSON object; for client and bidirectional streaming a JSON array, one element per message |

Set exactly one of `reflection` and `schema`. A `.proto` file is compiled by hozd itself, no
`protoc` needed; a descriptor set is what `protoc --include_imports --descriptor_set_out=`
writes.

### `config.toml`

Optional. Every setting has a default.

```toml
# Extra directories searched for `.proto` imports, after the directory of the loaded file.
# `~` is your home directory; relative paths are relative to ~/.hozd.
proto_include = ["~/work/protos"]
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
| `Ctrl+T`          | Switch the request box between Body and Headers (Metadata in gRPC) |
| `Ctrl+G`          | Switch the request between REST and gRPC |
| Drag (reading panel) | Select text; it is copied to the clipboard on release (`y` copies the selection, or everything if none) |
| `Ctrl+E` / click the Logs box | Open the log in the reading panel (newest last) |
| `Enter` (response focus) | Open the response body in the reading panel |
| `PgUp` / `PgDn`   | Scroll the response body                 |
| `y` (list focus)  | Copy the response body to the clipboard  |
| `q` / `Ctrl+C`    | Quit                                     |

### gRPC

`Ctrl+G` (or a click on `REST | gRPC` on the address box) switches the request to gRPC. The
URL becomes the server address and the headers become metadata; body and headers carry over.
Three more boxes appear: the schema source, the method, and the TLS state.

1. Type the server address with a scheme: `http://` is plaintext, `https://` is TLS verified
   against the system trust store.
2. Choose where the schema comes from and load it with `Ctrl+L` (or `Enter` in the Schema
   box): **reflection** (the server describes itself, `v1` then `v1alpha`) or **a file**
   (`Ctrl+O` toggles): a `.proto` source, compiled by hozd, or a compiled descriptor set
   (`.bin`, `.pb`, `.desc`). Import roots beyond the file's own directory go in
   `proto_include` of `config.toml`. Sending without loading loads it first. The loaded
   schema is kept for that address and source.
3. Pick the method with `Ctrl+N` / `Ctrl+P`, `Enter` in the Method box, or a click. Type to
   filter. An empty body is filled with a skeleton of the input message.
4. `Ctrl+S` sends. Unary and server streaming take a JSON object; client and bidirectional
   streaming take a JSON array, one element per message, sent in one go. Responses stream
   in as they arrive, numbered; `Ctrl+X` cancels the call (status `CANCELLED`).

`Ctrl+K` (or a click on the TLS box) turns on **insecure** mode, which skips certificate
verification for self-signed development servers; the address box then says `NOT VERIFIED`.
The copy action and the reading panel give the single message of a unary or client streaming
call, and a JSON array of all messages for server and bidirectional streaming.

| Key (gRPC mode)     | Action                                                |
| ------------------- | ----------------------------------------------------- |
| `Ctrl+L`            | Load the schema for the current address and source    |
| `Ctrl+O`            | Schema source: reflection / file                      |
| `Tab` / `Down` `Up` / `Enter` (file input) | Accept / move through / accept the path suggestions |
| `Ctrl+N` / `Ctrl+P` | Open the method picker                                |
| `Ctrl+K`            | Toggle insecure TLS                                   |
| `Ctrl+X`            | Cancel the call in flight                             |
| picker: `Up` `Down` `Enter` `Esc` | Move, pick, close; typing filters        |

`Ctrl+X` also shadows the text editors' cut shortcut.

### Mouse

| Click                          | Action                                              |
| ------------------------------ | --------------------------------------------------- |
| `Stored` / `History` tab       | Switch tab                                          |
| List item                      | Select and preview; click again to load into editor |
| Method box                     | Next method (right-click: previous); TLS box in gRPC toggles insecure |
| `REST` / `gRPC` label          | Switch the request kind                             |
| Schema / Method box (gRPC)     | Focus the schema box / open the method picker       |
| URL / body / headers           | Focus the editor and place the cursor               |
| `Body` / `Headers` title       | Switch the request tab                              |
| Wheel over the list            | Move the selection                                  |
| Wheel over the response body   | Scroll                                              |
| Right-click the response body  | Copy it to the clipboard                            |
| Click / double-click the response body | Focus it / open the reading panel           |

Clicking the editor while previewing an entry loads that entry first, so you edit what
you were looking at.

### Headers

The Headers tab has one row per header: the name and the value are inputs side by side.

| Key                   | Action                                                        |
| --------------------- | ------------------------------------------------------------- |
| `Enter` (in a name)   | Move to the value                                             |
| `Enter` (in a value)  | Move to the next row; on the last row, append a new header    |
| `Up` / `Down`         | Previous / next row                                           |
| `Right` / `Left`      | At the end of a name / start of a value, cross to the other input |
| `Ctrl+D`              | Remove the row                                                |

Clicking an input puts the cursor there; clicking the `+ add header` line appends a row.
Rows left empty are ignored. A name with characters a header cannot have shows in red, and
sending is refused with the row number until you fix it. Headers of loaded templates and
history entries appear here and are saved with every history entry.

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

### Reading a response

`Tab` to the response body (or click it) and press `Enter`, or double-click it, to open
the body in a large panel. It opens as received; wrapped lines keep their colors.

| Key               | Action                                           |
| ----------------- | ------------------------------------------------ |
| `F`               | Format JSON or HTML; press again for the original |
| `j` `k` / arrows  | Scroll a line                                    |
| `PgUp` `PgDn` / `b` `Space` | Scroll a page                          |
| `g` `G` / `Home` `End` | Top / bottom                                |
| `y`               | Copy the text as shown (formatted if formatted)  |
| `Esc` / `q`       | Close (or click outside the panel)               |

JSON is re-indented. HTML gets one tag or text node per line, indented by nesting;
`<script>` and `<style>` keep their relative indentation and `<pre>`/`<textarea>` are left
untouched. Other content types are shown as is, and `F` says there is nothing to format.

### Copying

hozd captures the mouse, so the terminal's own text selection is off by default; hold
`Shift` (`Option` in iTerm2, `Fn` in some terminals) while dragging to select text natively.
To copy the whole response body use `y` or right-click it. The text (pretty-printed when
it is JSON) goes to the clipboard with the OSC 52 escape sequence, which works over SSH
and inside tmux (needs `set -g set-clipboard on`). Terminals without OSC 52 support
(for example macOS Terminal.app) ignore it; use Shift+drag there.
