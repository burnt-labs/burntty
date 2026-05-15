# Architecture

`burntty` is split into three crates:

- `burntty-protocol`: JSON request/response types shared by clients and the daemon.
- `burntty-daemon`: Unix socket server that owns PTY sessions and process handles.
- `burntty`: command-line client that starts the daemon on demand and sends one RPC per command.

The daemon is the only process that reads from PTY masters. Every PTY read is appended to the session buffer before any client can capture or expect against it, so clients do not steal bytes from each other.

Implemented session operations:

- `spawn`: create one named PTY-backed process.
- `list` and `status`: inspect active or exited sessions.
- `send`, `send-line`, and `send-secret`: write to the PTY, with secret sends redacted in structured events.
- `expect`: wait for literal or regex matches over the stream buffer.
- `capture`: return recent or complete buffered output.
- `attach`: replay recent output and bridge the user's terminal to the PTY for debugging.
- `resize`: update PTY dimensions.
- `kill` and `rm`: terminate and remove sessions.

The current implementation keeps session metadata and buffers in daemon memory. Restarting the daemon drops sessions. Durable logs, screen-state capture, session locking, and richer attach behavior are planned follow-ups from `docs/plan.md`.
