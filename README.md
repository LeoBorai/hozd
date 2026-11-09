# hozd

A terminal HTTP, gRPC, WebSocket and GraphQL client in the spirit of cURL and grpcurl, with a
lazygit/lazydocker-style TUI. Built on [reqwest](https://docs.rs/reqwest),
[tonic](https://docs.rs/tonic) and [ratatui](https://ratatui.rs).

```
+-----------+-----------------------------+-----------+
| History   |  [Method] [URL ............]  | Response  |
|  requests |  Body (JSON)                 | Headers   |
|           |  { "name": "ada" }           | Body      |
+-----------+-----------------------------+-----------+
     3                    6                      3
```

- **Left**: `History`. Moving over an entry previews it: the center
  panel shows its request, the right panel its response. Nothing is sent.
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

### Give it a try

#### gRPC

You can leverage this site https://grpcb.in for testing the gRPC client.

#### REST

You can use the good ol' https://httpbin.org for testing the REST client.

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

A WebSocket template has `kind = "ws"` and a `ws://` or `wss://` `url`. `body`, if set, is
sent as a text message once the socket opens:

```toml
name = "Echo"
kind = "ws"
url = "wss://echo.websocket.org"
body = "hello"

[headers]
Authorization = "Bearer token"
```

A GraphQL template has `kind = "graphql"`, an `http://` or `https://` `url`, the `query`
document and, optionally, `variables` as a JSON object. It is sent as a JSON `POST`:

```toml
name = "Get user"
kind = "graphql"
url = "https://api.example.com/graphql"
query = '''
query User($id: ID!) { user(id: $id) { name } }
'''
variables = '''
{ "id": "7" }
'''

[headers]
Authorization = "Bearer token"
```

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
| `?` / `F1`        | Open the keybindings menu: `Up`/`Down` pick a row, `Enter` runs it, `Esc` closes (`?` types a character in text fields, use `F1` there) |
| `j` `k` / arrows  | Move selection (list)                    |
| `Enter`           | Load the selected entry into the editor (list); send (URL field) |
| `Ctrl+S` / `Ctrl+Enter` / `Cmd+Enter` | Send the request (`Ctrl+Enter` needs a kitty-protocol terminal) |
| `Ctrl+1`..`Ctrl+9` | Jump to the panel with that number (shown in its title), also from text fields; needs a kitty-protocol terminal |
| `1`..`9`          | Same, but only outside text fields |
| `Ctrl+N`          | New request: clear the form (refused while a request runs; `Ctrl+X` first) |
| `Ctrl+P`          | Previous HTTP method (REST); open the method / operation picker (gRPC, GraphQL) |
| `Ctrl+T`          | Switch the request box between Body and Headers (Metadata in gRPC); REST also has URL Params and Auth |
| `Ctrl+R`          | Auth tab: switch scheme (None, Basic, JWT, Free text)              |
| `Ctrl+G`          | Cycle the request between REST, GraphQL, gRPC and WebSocket |
| `Ctrl+Y`          | REST: next body type (also a click on the type shown on the Body box's border) |
| `Ctrl+V`          | Switch the lower left panel between Logs and Variables |
| `Ctrl+B`          | REST: use `$variables` in the body (also the `[x] $vars` box under the Body box) |
| Drag (reading panel) | Select text; it is copied to the clipboard on release (`y` copies the selection, or everything if none) |
| `Ctrl+E` / click the Logs box | Open the log in the reading panel (newest last) |
| `Enter` (response focus) | Open the response body in the reading panel |
| `PgUp` / `PgDn`   | Scroll the response body                 |
| `y` (list focus)  | Copy the response body to the clipboard  |
| `q` / `Ctrl+C`    | Quit                                     |

### Project variables

Capture a value from one response and send it in the next request, like Hurl's captures.

1. `Ctrl+V` switches the lower left panel from **Logs** to **Variables**. Each row there is a
   capture: a variable name and a path into the JSON response body, such as `auth_token` and
   `.data.token`. Paths are a jq subset: `.a.b`, `.items[0].id`, `.items[-1]`, `.["odd key"]`.
2. Send the request. Every capture runs on the response; the panel lists the values captured
   (`$auth_token = ...`) and the log says which succeeded or failed.
3. In a later REST request write `$auth_token` in the body: a JSON string, a raw JSON value, text,
   or a form or multipart value. It is replaced when the request is sent.

A `$name` that no variable matches is sent as written. `Ctrl+B` (or a click on `[x] $vars`)
turns the substitution off for the body, to send a literal `$auth_token`. History keeps the
request as written, so a replay uses the current value.

`Ctrl+D` on a row deletes the capture and its variable. `Ctrl+N` clears the capture rows but
keeps the captured values. Variables live until hozd quits.

### REST body types

`Ctrl+Y` cycles the body type; the current one is shown on the right of the Body box's
border. Every type sets its `Content-Type` unless you set that header yourself.

| Type | Editor | `Content-Type` |
| ---- | ------ | -------------- |
| Text | free text | `text/plain` |
| JSON | one row per field: key, value and type (string, number, boolean, null, object, array) | `application/json` |
| JSON (RAW) | JSON typed by hand, with highlighting; the default | `application/json` |
| Form URL Encoded | name and value rows, percent-encoded | `application/x-www-form-urlencoded` |
| Multipart | name and value rows; a value starting with `@` is a file | `multipart/form-data` |
| File | a single `@path` | `application/octet-stream` |

- **`@` picks a file.** Typing `@` in a Multipart value or in the File input opens a menu of
  the files and directories that continue the path (`~/` works): `Up`/`Down` pick, `Tab` (or
  `Enter` on a picked row) accepts, a directory opens its content. A file is read when the
  request is sent; a missing one is reported before anything goes out. To send a text that
  starts with `@`, write `@@`.
- **JSON rows.** `Enter` moves from the key to the value, then to the next row, and adds a
  row after the last one (on an empty object or array it adds the first child). `Ctrl+R`
  changes the row's type, `Space` flips a boolean, `Alt+Right` / `Alt+Left` nest a row into
  the container above it or move it out, `Ctrl+D` removes a row with its children, and
  `Alt+T` switches the document between an object and an array. Fields keep the order you
  added them in.
- Switching from JSON to JSON (RAW) or Text prints the document as indented text; switching
  from JSON (RAW) or Text to JSON loads an object or array into rows.
- Template files (`body = ...`) are JSON (RAW). History keeps the body type of each request.

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
3. Pick the method with `Ctrl+P`, `Enter` in the Method box, or a click. Type to
   filter. An empty body is filled with a skeleton of the input message.
4. `Ctrl+S` sends. Unary and server streaming take a JSON object; client and bidirectional
   streaming take a JSON array, one element per message, sent in one go. Responses stream
   in as they arrive, numbered; `Ctrl+X` cancels the call (status `CANCELLED`).

### GraphQL

`Ctrl+G` (or a click on `GQL`) switches the request to GraphQL. The URL is the endpoint;
the request box has three tabs, **Query**, **Variables** (a JSON object) and **Headers**
(`Ctrl+T` cycles them). `Ctrl+S` sends a JSON `POST` with `query` and `variables`; the
response is shown as received, errors included.

1. `Ctrl+L` introspects the endpoint (a standard introspection query, sent with your
   headers). The result is kept for that URL.
2. `Ctrl+P` (or `Enter` in the Operation box) opens the picker over the queries and
   mutations; type to filter. With an empty query, picking one fills in a document with
   every argument as a variable and a shallow selection set, plus a variables skeleton.
   Subscriptions are not supported.

### WebSocket

`Ctrl+G` (or a click on `WS`) switches the request to WebSocket. The URL is a `ws://` or
`wss://` address, the headers are sent with the upgrade request, and the body is the message.

1. `Ctrl+S` opens the socket and sends the body, if there is one. The Socket box reads `OPEN`.
2. While it is open, `Ctrl+S` sends the body again: edit it and send as often as you like.
   The response body is the conversation, `->` for what you sent and `<-` for what arrived,
   live as it happens.
3. `Ctrl+X` closes the socket (a normal close frame). A close from the server ends the session
   too. The session is saved to history with its whole transcript.

Text frames only are sent; binary frames received are shown as lossy UTF-8.

`Ctrl+K` (or a click on the TLS box) turns on **insecure** mode, which skips certificate
verification for self-signed development servers; the address box then says `NOT VERIFIED`.
The copy action and the reading panel give the single message of a unary or client streaming
call, and a JSON array of all messages for server and bidirectional streaming.

| Key (gRPC mode)     | Action                                                |
| ------------------- | ----------------------------------------------------- |
| `Ctrl+L`            | Load the schema for the current address and source    |
| `Ctrl+O`            | Schema source: reflection / file                      |
| `Tab` / `Down` `Up` / `Enter` (file input) | Accept / move through / accept the path suggestions |
| `Ctrl+P`            | Open the method picker                                |
| `Ctrl+K`            | Toggle insecure TLS                                   |
| `Ctrl+X`            | Cancel the call in flight                             |
| picker: `Up` `Down` `Enter` `Esc` | Move, pick, close; typing filters        |

`Ctrl+X` also shadows the text editors' cut shortcut.

### Mouse

| Click                          | Action                                              |
| ------------------------------ | --------------------------------------------------- |
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
