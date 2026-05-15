erminal Expect Handoff Plan
Goal
Build a terminal automation tool that can run any interactive terminal program in the background while exposing an Expect-like control surface for automation.

The tool should preserve the useful convention of terminal multiplexers: a long-running program can keep its PTY session alive, humans can attach for debugging, and the process continues after clients disconnect. The primary design goal is better automation, not pane layout. Think of it as:

persistent PTY session manager + Expect-style automation API
This is different from tmux because automation is the first-class interface, not a secondary behavior built on top of terminal pane scraping. It is different from pexpect because sessions are persistent, backgroundable, and attachable after the automation client exits.

Problem Statement
Legacy CLI and TUI programs often require shell-like interaction:

prompts must be matched before sending input
output may arrive asynchronously
commands can take variable time
secrets may be typed into non-echoing prompts
the process may need to stay alive between automation runs
debugging often requires attaching to the live terminal state
Existing tools solve parts of this:

pexpect is good for one script controlling one child process, but the session usually belongs to the script lifecycle.
tmux is good for persistent attachable sessions, but automation generally means sending keys and scraping pane text.
script, asciinema, and shell logging tools are good for recording, but not for structured interaction.
The desired tool owns the PTY directly and exposes structured automation operations.

High-Level Architecture
CLI / SDK clients
      |
      | local RPC over Unix socket
      v
Session daemon
      |
      | owns PTY master handles
      v
PTY-backed sessions
      |
      v
legacy CLI / shell / TUI program
The daemon is the durable owner of each PTY. Clients connect to the daemon to spawn programs, send input, wait for output, capture history, attach interactively, and terminate sessions.

Core Concepts
Session
A named persistent process running under a PTY.

Example:

texpect spawn legacy -- ./legacy-cli --profile prod
Expected session metadata:

session name
command and arguments
working directory
environment overrides
process id
PTY size
start time
current lifecycle state
last output timestamp
exit status if exited
Output Stream
Every byte read from the PTY should be recorded before it is consumed by any client. The daemon should support multiple consumers without allowing one reader to steal bytes from another.

The system should maintain:

raw byte log
decoded text log where possible
ring buffer for recent output
optional durable event log
timestamped read/write events
Expect Operation
An expect operation waits until session output matches a pattern or times out.

Example:

texpect expect legacy 'legacy>' --timeout 30s
Expected matching modes:

literal string
regular expression
prompt helper
idle/quiet period
process exit
Send Operation
A send operation writes bytes to the PTY.

Example:

texpect send legacy 'status\n'
There should be explicit variants for common cases:

texpect send-line legacy 'status'
texpect send-key legacy Enter
texpect send-secret legacy "$PASSWORD"
send-secret should avoid logging the secret payload by default.

Attach Operation
Attach connects the user's current terminal to a running PTY-backed session.

Example:

texpect attach legacy
Attach should be useful for debugging, but the system does not need full tmux-style pane/window management in the MVP.

Candidate CLI Shape
Use a short binary name only after the project name is chosen. The examples below use texpect as a placeholder.

texpect spawn <name> -- <command> [args...]
texpect list
texpect status <name>
texpect send <name> <bytes>
texpect send-line <name> <text>
texpect send-key <name> <key>
texpect send-secret <name> <secret>
texpect expect <name> <pattern> [--regex] [--timeout 30s]
texpect capture <name> [--last 200-lines]
texpect tail <name>
texpect attach <name>
texpect resize <name> --cols 120 --rows 40
texpect kill <name>
texpect rm <name>
Example flow:

texpect spawn legacy -- ./legacy-cli
texpect expect legacy 'login:'
texpect send-line legacy 'admin'
texpect expect legacy 'password:'
texpect send-secret legacy "$LEGACY_PASSWORD"
texpect expect legacy 'legacy>'
texpect send-line legacy 'status'
texpect expect legacy 'legacy>'
texpect capture legacy --last 100-lines
SDK Shape
The CLI should be backed by a stable local API so scripts can use the tool without shelling out for every operation.

Python-style target API:

from texpect import Client

client = Client()
session = client.spawn("legacy", ["./legacy-cli"])

session.expect("login:")
session.send_line("admin")
session.expect("password:")
session.send_secret(password)
session.expect("legacy>")

session.send_line("status")
result = session.expect("legacy>")
print(result.before)
Rust-style internal API:

let session = client.spawn("legacy", CommandSpec::new("./legacy-cli")).await?;

session.expect(Pattern::literal("login:"), timeout).await?;
session.send_line("admin").await?;
session.expect(Pattern::literal("password:"), timeout).await?;
session.send_secret(password).await?;
session.expect(Pattern::literal("legacy>"), timeout).await?;
Implementation Language Recommendation
Rust is a strong fit for this project because the hard parts are state management, async I/O, process lifecycle, and durable service boundaries.

Useful crates to evaluate:

tokio: async runtime and Unix socket handling
portable-pty: cross-platform PTY abstraction
rustix or nix: lower-level Unix PTY, signal, and process control
regex: output matching
serde: request/response serialization
tracing: structured daemon logs
clap: CLI parsing
vte: terminal escape parsing if screen-state capture becomes necessary
crossterm or ratatui: optional interactive attach UI
Avoid starting with a full terminal multiplexer. The first milestone should be a durable automation daemon for a single PTY-backed session.

MVP Scope
The MVP should prove that the daemon can own a PTY and provide useful automation primitives.

Required:

spawn one named session
keep the session alive after the client exits
send bytes or lines to the session
read and buffer PTY output
wait for literal or regex output matches
return captured output around a match
list sessions
kill a session
timestamp PTY reads and writes
avoid logging secret sends
Nice to have:

attach to a session
resize PTY
tail live output
durable logs on disk
idle detection
Out of scope for MVP:

split panes
windows/tabs
remote access
full terminal rendering correctness
plugin system
multi-user permissions
terminal scrollback UI
MVP Milestones
Milestone 1: One Process, One PTY
Build a local command that spawns a child process inside a PTY and forwards output to stdout.

Acceptance criteria:

can run /bin/sh
can send input to the shell
can read shell output
handles process exit
handles basic terminal resize
Milestone 2: Daemon Ownership
Move PTY ownership into a background daemon.

Acceptance criteria:

spawn creates a named session
client can exit while child process continues
list shows active sessions
kill terminates a session
daemon cleans up exited children
Milestone 3: Expect API
Add output buffering and pattern matching.

Acceptance criteria:

expect <literal> waits until matching output appears
expect --regex supports regular expressions
timeout returns a structured error
result includes before, match, and recent output context
multiple clients can call capture without stealing output
Milestone 4: Logging and Replay
Add structured PTY event logging.

Acceptance criteria:

PTY reads are timestamped
PTY writes are timestamped
secret writes are redacted
recent output can be retrieved as text
raw logs can be retained for debugging
Milestone 5: Attach
Allow a human to attach to a running session.

Acceptance criteria:

current terminal enters raw mode
local input is forwarded to PTY
PTY output is rendered to local terminal
detach key returns to local shell without killing the session
resize events propagate to the child PTY
RPC Design
Start with a Unix domain socket and newline-delimited JSON. This keeps the protocol inspectable and easy to drive from multiple languages.

Example request:

{"id":"1","method":"expect","params":{"session":"legacy","pattern":"legacy>","mode":"literal","timeout_ms":30000}}
Example response:

{"id":"1","ok":true,"result":{"matched":"legacy>","before":"status\nOK\n","after":"","elapsed_ms":431}}
Example event:

{"event":"session_output","session":"legacy","ts":"2026-05-15T12:00:00.000Z","bytes":42}
The protocol can later move to JSON-RPC, gRPC, Cap'n Proto, or another structured protocol if needed. Do not optimize this too early.

Matching Semantics
Expect matching should operate against the session output stream, not just visible terminal text.

Important details:

Each session should track a monotonically increasing output offset.
Each expect call should specify whether it starts from the current end or an earlier offset.
Default behavior should likely be "from now" to avoid matching stale prompts accidentally.
Advanced users should be able to expect against recent history.
Suggested options:

texpect expect legacy 'ready>'              # from now
texpect expect legacy 'ready>' --from-start
texpect expect legacy 'ready>' --from-offset 12345
texpect expect legacy 'ready>' --include-history
For scripted command execution, provide a helper that avoids stale prompt confusion:

texpect run legacy 'status' --prompt 'legacy>'
Internally this should:

record current output offset
send the command
wait for prompt from that offset
return only command output
Screen State vs Stream State
There are two kinds of output state:

stream state: raw bytes/text emitted by the PTY
screen state: what the terminal display currently looks like after ANSI control sequences
MVP should focus on stream state. That is enough for many shell-style programs.

Screen state becomes important for:

curses apps
full-screen TUIs
progress bars
output that rewrites previous lines
menus navigated by arrow keys
When needed, add a vte-backed screen model:

PTY bytes -> vte parser -> virtual screen buffer -> capture-screen API
Do not make full screen emulation part of the first milestone unless the target legacy program requires it.

Logging Strategy
PTY logging is critical because automation failures are otherwise hard to debug.

Recommended event types:

session created
process started
PTY read
PTY write
PTY secret write
expect started
expect matched
expect timed out
process exited
session killed
client attached
client detached
resize
Log examples:

{"ts":"...","session":"legacy","event":"pty_read","offset":1024,"bytes":80}
{"ts":"...","session":"legacy","event":"pty_write","bytes":7,"preview":"status\n"}
{"ts":"...","session":"legacy","event":"pty_secret_write","bytes":18,"preview":"[REDACTED]"}
Keep raw PTY logs separate from daemon diagnostic logs.

Secret Handling
PTY-level automation can easily capture passwords and tokens. Treat this as a core design concern.

Requirements:

send-secret should redact payloads from structured logs
avoid echoing secret values in CLI errors
document that the child program may still echo secrets itself
support disabling raw logs for sensitive sessions
consider per-session log retention settings
Example:

texpect spawn prod --no-raw-log -- ./prod-admin-cli
Failure Modes to Design Around
Stale Prompt Match
The automation matches an old prompt in the buffer and sends input too early.

Mitigation:

default expect to match from current output offset
expose command helper that records offset before sending
Output Race
Output arrives while a client disconnects or reconnects.

Mitigation:

daemon owns all reads
clients consume buffered events
no direct competing PTY readers
Dead Session
The child process exits but the session metadata remains.

Mitigation:

daemon tracks process lifecycle
status shows exit code and recent output
rm cleans exited sessions
TUI Escape Noise
ANSI control sequences make captured output hard to match.

Mitigation:

support raw matching and stripped-text matching
later add virtual screen capture
Secret Leakage
Secrets are stored in logs.

Mitigation:

explicit secret-send path
redacted logging
optional raw log disablement
Attach Interference
Human attach sends keys while automation is running.

Mitigation:

session lock modes
visible session status
optional exclusive attach
automation lease for critical flows
Session Locking
Add locking after the basic API works.

Useful modes:

shared observe: multiple clients can capture/tail output
automation lock: one client can send/expect command sequences
attach lock: a human controls the session
force unlock: admin recovery path
Example:

texpect lock legacy --mode automation --ttl 60s
texpect unlock legacy
Configuration
Support a simple project-local config after the MVP.

Example:

[[sessions]]
name = "legacy"
command = "./legacy-cli"
cwd = "/srv/legacy"
env = { LEGACY_ENV = "prod" }
prompt = "legacy>"
raw_log = false
Then:

texpect up
texpect run legacy 'status'
Relationship to Existing Tools
tmux
tmux is a terminal multiplexer first. It can be automated with send-keys, capture-pane, and pipe-pane, but that means automation is layered over terminal display behavior.

This project should own the PTY and provide structured automation directly.

pexpect
pexpect provides excellent Expect-style primitives for scripts, but the child session is normally tied to the script process.

This project should keep sessions alive independently and expose the same style of primitives over a daemon API.

Zellij
Zellij is a mature Rust terminal workspace. It is useful as a reference for Rust terminal architecture, but it is still primarily a terminal workspace/multiplexer.

zmux
If zmux already has useful minimal PTY/session behavior, use it as a reference or prototype baseline. Avoid a direct rewrite until the desired automation API is clear.

Suggested Repository Structure
.
├── crates/
│   ├── texpect-cli/
│   ├── texpect-daemon/
│   ├── texpect-core/
│   └── texpect-protocol/
├── docs/
│   ├── architecture.md
│   ├── protocol.md
│   └── examples.md
├── examples/
│   ├── shell-login.sh
│   └── legacy-status.py
└── tests/
    └── fixtures/
Keep PTY/process/session logic in texpect-core, protocol types in texpect-protocol, daemon service code in texpect-daemon, and command-line UX in texpect-cli.

Test Strategy
Use fake interactive programs for deterministic tests.

Example fixture behavior:

prints login:
reads username
prints password:
reads password
prints ready>
responds to status
exits on quit
Test cases:

spawn session
expect prompt
send line
expect next prompt
timeout when pattern does not appear
command output does not include stale history
secret send is redacted in logs
process exit is detected
session survives client disconnect
Integration tests should run against real shells too:

/bin/sh
/bin/bash
python -i
node
Open Design Questions
Should the daemon start automatically on first CLI use?
Should session metadata be stored in memory only or persisted?
Should raw PTY logs be enabled by default?
What is the default expect starting offset: current end or recent history?
What is the detach key for interactive attach?
Is Windows support a goal, or is this Unix/macOS/Linux first?
Should the first API be a CLI only, a Rust SDK, or both?
Should screen-state capture be part of v1 or deferred?
Recommended First Implementation Path
Build a Rust prototype that spawns one PTY-backed child and records output.
Add a daemon process that owns named sessions.
Add a simple Unix socket protocol with JSON request/response.
Implement spawn, list, send-line, expect, capture, and kill.
Add timestamped PTY event logs and secret redaction.
Build a fake legacy CLI fixture and integration tests.
Add attach only after non-interactive automation is solid.
Add screen-state parsing only if stream matching is insufficient for real targets.
Success Criteria
The first useful version is successful if it can reliably automate this flow:

texpect spawn legacy -- ./legacy-cli
texpect expect legacy 'login:'
texpect send-line legacy 'admin'
texpect expect legacy 'password:'
texpect send-secret legacy "$PASSWORD"
texpect expect legacy 'legacy>'
texpect run legacy 'status' --prompt 'legacy>'
texpect capture legacy --last 100-lines
texpect attach legacy
And it should provide enough logs to debug failures without needing to reproduce them manually.

Non-Goals
replacing tmux for general pane/window workflow
perfect terminal emulator behavior in v1
distributed or remote session management
multi-user access control
browser-based terminal UI
plugin system
Summary
Build an automation-first PTY daemon. Keep terminal attachability, but make structured expect, send, capture, and logging the core product. Start narrow with stream-based matching and one PTY session, then add persistence, attach, locking, and optional screen-state parsing as real use cases demand them.
