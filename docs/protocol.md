# Protocol

The daemon listens on a local Unix socket. The default path is:

```text
$BURNTTY_SOCKET, or /tmp/burntty-$USER.sock
```

Each request is one newline-delimited JSON object:

```json
{"id":"1","method":"expect","params":{"session":"legacy","pattern":"legacy>","timeout_ms":30000}}
```

Each response is one newline-delimited JSON object:

```json
{"id":"1","ok":true,"result":{"matched":"legacy>","before":"status\nOK\n","after":"","recent":"...","start_offset":12,"match_start_offset":23,"match_end_offset":30,"elapsed_ms":431}}
```

Error responses use the same envelope:

```json
{"id":"1","ok":false,"error":{"code":"request_failed","message":"session legacy not found"}}
```

Methods currently implemented:

- `spawn`: `{ "name": "...", "command": { "program": "...", "args": [], "cwd": null, "env": {}, "cols": 80, "rows": 24, "raw_log": true } }`
- `list`: `{}`
- `status`: `{ "session": "..." }`
- `send`: `{ "session": "...", "data": "...", "append_newline": false, "secret": false }`
- `expect`: `{ "session": "...", "pattern": "...", "regex": false, "timeout_ms": 30000, "include_history": false, "from_start": false, "from_offset": null }`
- `capture`: `{ "session": "...", "last_lines": 100 }`
- `attach`: `{ "session": "...", "replay_bytes": 4096 }`
- `resize`: `{ "session": "...", "cols": 120, "rows": 40 }`
- `kill`: `{ "session": "..." }`
- `rm`: `{ "session": "..." }`

`expect` defaults to matching from the current output end. Use `include_history`, `from_start`, or `from_offset` when matching output that may already be buffered.

`attach` is a mode switch, not a normal request/response operation. After the daemon sends the initial `ok` response line, the socket becomes a raw byte stream:

- client bytes are written directly to the session PTY
- session PTY output bytes are written directly to the client
- the CLI uses `Ctrl-]` as a local detach key
