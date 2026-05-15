use anyhow::{anyhow, bail, Context, Result};
use burntty_protocol::{
    default_socket_path, AttachParams, CaptureParams, CommandSpec, ExpectParams, NameParams,
    ResizeParams, RpcRequest, RpcResponse, SendParams, SpawnParams, PROTOCOL_VERSION,
};
use clap::{Args, Parser, Subcommand};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Parser)]
#[command(name = "burntty")]
#[command(about = "Persistent PTY sessions with an Expect-style automation API")]
struct Cli {
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Spawn(SpawnCommand),
    List,
    Status(NameCommand),
    Send(SendCommand),
    SendLine(SendLineCommand),
    SendSecret(SendSecretCommand),
    Expect(ExpectCommand),
    Capture(CaptureCommand),
    Attach(AttachCommand),
    Resize(ResizeCommand),
    Kill(NameCommand),
    Rm(NameCommand),
    SocketPath,
}

#[derive(Debug, Args)]
struct SpawnCommand {
    name: String,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long = "env", value_parser = parse_key_value)]
    env: Vec<(String, String)>,
    #[arg(long, default_value_t = 80)]
    cols: u16,
    #[arg(long, default_value_t = 24)]
    rows: u16,
    #[arg(long)]
    no_raw_log: bool,
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<OsString>,
}

#[derive(Debug, Args)]
struct NameCommand {
    session: String,
}

#[derive(Debug, Args)]
struct SendCommand {
    session: String,
    bytes: String,
}

#[derive(Debug, Args)]
struct SendLineCommand {
    session: String,
    text: String,
}

#[derive(Debug, Args)]
struct SendSecretCommand {
    session: String,
    secret: String,
}

#[derive(Debug, Args)]
struct ExpectCommand {
    session: String,
    pattern: String,
    #[arg(long)]
    regex: bool,
    #[arg(long, default_value = "30s", value_parser = parse_duration_ms)]
    timeout: u64,
    #[arg(long)]
    from_start: bool,
    #[arg(long)]
    include_history: bool,
    #[arg(long)]
    from_offset: Option<u64>,
}

#[derive(Debug, Args)]
struct CaptureCommand {
    session: String,
    #[arg(long)]
    last_lines: Option<usize>,
}

#[derive(Debug, Args)]
struct AttachCommand {
    session: String,
    #[arg(long, default_value_t = 4096)]
    replay_bytes: usize,
}

#[derive(Debug, Args)]
struct ResizeCommand {
    session: String,
    #[arg(long)]
    cols: u16,
    #[arg(long)]
    rows: u16,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let socket = cli.socket.unwrap_or_else(default_socket_path);

    if matches!(cli.command, Commands::SocketPath) {
        println!("{}", socket.display());
        return Ok(());
    }

    let require_current_protocol = matches!(cli.command, Commands::Attach(_));
    ensure_daemon(&socket, require_current_protocol)?;

    match cli.command {
        Commands::Spawn(command) => {
            let command = normalize_spawn_command(command)?;
            let result = request(
                &socket,
                "spawn",
                SpawnParams {
                    name: command.name,
                    command: command.command,
                },
            )?;
            print_json(result)
        }
        Commands::List => print_json(request(&socket, "list", serde_json::json!({}))?),
        Commands::Status(command) => {
            print_json(request(&socket, "status", name_params(command.session))?)
        }
        Commands::Send(command) => print_json(request(
            &socket,
            "send",
            SendParams {
                session: command.session,
                data: command.bytes,
                append_newline: false,
                secret: false,
            },
        )?),
        Commands::SendLine(command) => print_json(request(
            &socket,
            "send",
            SendParams {
                session: command.session,
                data: command.text,
                append_newline: true,
                secret: false,
            },
        )?),
        Commands::SendSecret(command) => print_json(request(
            &socket,
            "send",
            SendParams {
                session: command.session,
                data: command.secret,
                append_newline: true,
                secret: true,
            },
        )?),
        Commands::Expect(command) => print_json(request(
            &socket,
            "expect",
            ExpectParams {
                session: command.session,
                pattern: command.pattern,
                regex: command.regex,
                timeout_ms: command.timeout,
                from_start: command.from_start,
                include_history: command.include_history,
                from_offset: command.from_offset,
            },
        )?),
        Commands::Capture(command) => {
            let result = request(
                &socket,
                "capture",
                CaptureParams {
                    session: command.session,
                    last_lines: command.last_lines,
                },
            )?;
            if let Some(text) = result.get("text").and_then(Value::as_str) {
                print!("{text}");
                Ok(())
            } else {
                print_json(result)
            }
        }
        Commands::Attach(command) => attach(&socket, command),
        Commands::Resize(command) => print_json(request(
            &socket,
            "resize",
            ResizeParams {
                session: command.session,
                cols: command.cols,
                rows: command.rows,
            },
        )?),
        Commands::Kill(command) => {
            print_json(request(&socket, "kill", name_params(command.session))?)
        }
        Commands::Rm(command) => print_json(request(&socket, "rm", name_params(command.session))?),
        Commands::SocketPath => unreachable!(),
    }
}

struct NormalizedSpawn {
    name: String,
    command: CommandSpec,
}

fn normalize_spawn_command(command: SpawnCommand) -> Result<NormalizedSpawn> {
    let mut argv = command
        .command
        .into_iter()
        .map(|value| {
            value
                .into_string()
                .map_err(|_| anyhow!("command arguments must be valid UTF-8"))
        })
        .collect::<Result<Vec<_>>>()?;

    if argv.first().map(String::as_str) == Some("--") {
        argv.remove(0);
    }
    if argv.is_empty() {
        bail!("spawn requires a command after --");
    }

    let program = argv.remove(0);
    let env = command.env.into_iter().collect::<BTreeMap<_, _>>();
    let cwd = match command.cwd {
        Some(cwd) => Some(cwd),
        None => Some(std::env::current_dir()?),
    };

    Ok(NormalizedSpawn {
        name: command.name,
        command: CommandSpec {
            program,
            args: argv,
            cwd,
            env,
            cols: command.cols,
            rows: command.rows,
            raw_log: !command.no_raw_log,
        },
    })
}

fn name_params(session: String) -> NameParams {
    NameParams { session }
}

fn request<T: Serialize>(socket: &Path, method: &str, params: T) -> Result<Value> {
    let mut stream = UnixStream::connect(socket)
        .with_context(|| format!("failed to connect to daemon socket {}", socket.display()))?;
    let request = RpcRequest {
        id: REQUEST_ID.fetch_add(1, Ordering::Relaxed).to_string(),
        method: method.to_string(),
        params: serde_json::to_value(params)?,
    };
    serde_json::to_writer(&mut stream, &request)?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    if line.trim().is_empty() {
        bail!("daemon closed connection without a response");
    }

    let response: RpcResponse = serde_json::from_str(&line)?;
    if response.ok {
        Ok(response.result.unwrap_or(Value::Null))
    } else {
        let error = response
            .error
            .map(|err| format!("{}: {}", err.code, err.message))
            .unwrap_or_else(|| "unknown daemon error".to_string());
        bail!("{error}")
    }
}

fn attach(socket: &Path, command: AttachCommand) -> Result<()> {
    let mut stream = UnixStream::connect(socket)
        .with_context(|| format!("failed to connect to daemon socket {}", socket.display()))?;
    let request = RpcRequest {
        id: REQUEST_ID.fetch_add(1, Ordering::Relaxed).to_string(),
        method: "attach".to_string(),
        params: serde_json::to_value(AttachParams {
            session: command.session.clone(),
            replay_bytes: command.replay_bytes,
        })?,
    };
    serde_json::to_writer(&mut stream, &request)?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    let response = read_response_line(&mut stream)?;
    if !response.ok {
        let error = response
            .error
            .map(|err| format!("{}: {}", err.code, err.message))
            .unwrap_or_else(|| "unknown daemon error".to_string());
        bail!("{error}");
    }

    eprintln!("attached to {}; press Ctrl-] to detach", command.session);
    let _raw_mode = RawModeGuard::enter()?;

    let mut output_stream = stream.try_clone()?;
    let output = std::thread::spawn(move || -> std::io::Result<()> {
        let mut stdout = std::io::stdout();
        let mut buffer = [0_u8; 8192];
        loop {
            let n = output_stream.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            stdout.write_all(&buffer[..n])?;
            stdout.flush()?;
        }
        Ok(())
    });

    let mut stdin = std::io::stdin();
    let mut buffer = [0_u8; 1024];
    loop {
        let n = stdin.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        if let Some(position) = buffer[..n].iter().position(|byte| *byte == 0x1d) {
            if position > 0 {
                stream.write_all(&buffer[..position])?;
                stream.flush()?;
            }
            break;
        }
        stream.write_all(&buffer[..n])?;
        stream.flush()?;
    }

    let _ = stream.shutdown(Shutdown::Both);
    match output.join() {
        Ok(Ok(())) => {}
        Ok(Err(err)) if err.kind() == std::io::ErrorKind::BrokenPipe => {}
        Ok(Err(err)) => return Err(err.into()),
        Err(_) => bail!("attach output thread panicked"),
    }
    Ok(())
}

fn read_response_line(stream: &mut UnixStream) -> Result<RpcResponse> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        let n = stream.read(&mut byte)?;
        if n == 0 {
            bail!("daemon closed connection without a response");
        }
        line.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }
    serde_json::from_slice(&line).map_err(Into::into)
}

struct RawModeGuard;

impl RawModeGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

fn print_json(value: Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn ensure_daemon(socket: &Path, require_current_protocol: bool) -> Result<()> {
    if UnixStream::connect(socket).is_ok() {
        if require_current_protocol {
            verify_daemon(socket)?;
        }
        return Ok(());
    }

    let daemon = daemon_path()?;
    Command::new(&daemon)
        .arg("--socket")
        .arg(socket)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to start daemon {}", daemon.display()))?;

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if UnixStream::connect(socket).is_ok() {
            if require_current_protocol {
                verify_daemon(socket)?;
            }
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(25));
    }

    bail!("daemon did not become ready at {}", socket.display())
}

fn verify_daemon(socket: &Path) -> Result<()> {
    let result = request(socket, "hello", serde_json::json!({})).with_context(|| {
        format!(
            "running burntty-daemon at {} does not support the current protocol; stop the old daemon or remove the socket, then rerun the command",
            socket.display()
        )
    })?;

    let version = result
        .get("protocol_version")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if version < PROTOCOL_VERSION as u64 {
        bail!(
            "running burntty-daemon is too old for this CLI at {}. Stop the old daemon or remove the socket, then rerun the command.",
            socket.display()
        );
    }

    Ok(())
}

fn daemon_path() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("BURNTTY_DAEMON") {
        return Ok(PathBuf::from(path));
    }

    let current = std::env::current_exe()?;
    let dir = current
        .parent()
        .ok_or_else(|| anyhow!("could not determine current executable directory"))?;
    let candidates = [
        dir.join("burntty-daemon"),
        dir.join("../burntty-daemon"),
        PathBuf::from("target/debug/burntty-daemon"),
    ];

    candidates
        .into_iter()
        .find(|path| path.exists())
        .ok_or_else(|| anyhow!("could not find burntty-daemon; set BURNTTY_DAEMON"))
}

fn parse_key_value(value: &str) -> Result<(String, String), String> {
    let Some((key, value)) = value.split_once('=') else {
        return Err("expected KEY=VALUE".to_string());
    };
    if key.is_empty() {
        return Err("environment key cannot be empty".to_string());
    }
    Ok((key.to_string(), value.to_string()))
}

fn parse_duration_ms(value: &str) -> Result<u64, String> {
    let value = value.trim();
    if let Some(ms) = value.strip_suffix("ms") {
        ms.parse::<u64>().map_err(|err| err.to_string())
    } else if let Some(seconds) = value.strip_suffix('s') {
        seconds
            .parse::<u64>()
            .map(|seconds| seconds * 1_000)
            .map_err(|err| err.to_string())
    } else if let Some(minutes) = value.strip_suffix('m') {
        minutes
            .parse::<u64>()
            .map(|minutes| minutes * 60_000)
            .map_err(|err| err.to_string())
    } else {
        value.parse::<u64>().map_err(|err| err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_parser_accepts_common_suffixes() {
        assert_eq!(parse_duration_ms("250ms").unwrap(), 250);
        assert_eq!(parse_duration_ms("2s").unwrap(), 2_000);
        assert_eq!(parse_duration_ms("1m").unwrap(), 60_000);
        assert_eq!(parse_duration_ms("42").unwrap(), 42);
    }

    #[test]
    fn key_value_parser_requires_equals() {
        assert_eq!(
            parse_key_value("A=B").unwrap(),
            ("A".to_string(), "B".to_string())
        );
        assert!(parse_key_value("AB").is_err());
    }
}
