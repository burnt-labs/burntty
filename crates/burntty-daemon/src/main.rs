use anyhow::{anyhow, bail, Context, Result};
use burntty_protocol::{
    AttachParams, CaptureParams, CaptureResult, CommandSpec, EventRecord, ExpectParams,
    ExpectResult, HelloResult, NameParams, RemoveResult, ResizeParams, ResizeResult, RpcRequest,
    RpcResponse, SendParams, SendResult, SessionInfo, SessionState, SpawnParams, PROTOCOL_VERSION,
};
use chrono::{SecondsFormat, Utc};
use clap::Parser;
use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};
use regex::Regex;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, Notify};
use tracing::{error, info};

#[derive(Debug, Parser)]
struct Args {
    #[arg(long)]
    socket: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "burntty_daemon=info".into()),
        )
        .init();

    let args = Args::parse();
    let socket = args
        .socket
        .unwrap_or_else(burntty_protocol::default_socket_path);

    if socket.exists() {
        fs::remove_file(&socket)
            .with_context(|| format!("failed to remove stale socket {}", socket.display()))?;
    }

    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("failed to bind {}", socket.display()))?;
    info!(socket = %socket.display(), "burntty daemon listening");

    let manager = Arc::new(SessionManager::default());
    loop {
        let (stream, _) = listener.accept().await?;
        let manager = Arc::clone(&manager);
        tokio::spawn(async move {
            if let Err(err) = handle_client(stream, manager).await {
                error!(error = %err, "client handler failed");
            }
        });
    }
}

async fn handle_client(stream: UnixStream, manager: Arc<SessionManager>) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }

        let request = match serde_json::from_str::<RpcRequest>(&line) {
            Ok(request) => request,
            Err(err) => {
                let response = RpcResponse::error("unknown", "bad_request", err.to_string());
                write_response(&mut writer, response).await?;
                continue;
            }
        };

        let id = request.id.clone();
        if request.method == "attach" {
            let response = match decode_params::<AttachParams>(request.params) {
                Ok(params) => match manager.get(&params.session) {
                    Ok(session) => {
                        write_response(&mut writer, RpcResponse::ok(id, serde_json::json!({})))
                            .await?;
                        return attach_client(
                            reader.into_inner(),
                            writer,
                            session,
                            params.replay_bytes,
                        )
                        .await;
                    }
                    Err(err) => RpcResponse::error(id, "request_failed", err.to_string()),
                },
                Err(err) => RpcResponse::error(id, "request_failed", err.to_string()),
            };
            write_response(&mut writer, response).await?;
            continue;
        }

        let response = match route_request(request, Arc::clone(&manager)).await {
            Ok(result) => RpcResponse::ok(id, result),
            Err(err) => RpcResponse::error(id, "request_failed", err.to_string()),
        };
        write_response(&mut writer, response).await?;
    }

    Ok(())
}

async fn attach_client(
    mut reader: OwnedReadHalf,
    mut writer: OwnedWriteHalf,
    session: Arc<Session>,
    replay_bytes: usize,
) -> Result<()> {
    session.log_event("client_attached", None, None, None);

    let mut output = session.subscribe();
    let replay = session.replay_bytes(replay_bytes);
    if !replay.is_empty() {
        writer.write_all(&replay).await?;
        writer.flush().await?;
    }

    let mut input = [0_u8; 8192];
    loop {
        tokio::select! {
            read = reader.read(&mut input) => {
                let n = read?;
                if n == 0 {
                    break;
                }
                session.write_attach_input(&input[..n])?;
            }
            received = output.recv() => {
                match received {
                    Ok(bytes) => {
                        writer.write_all(&bytes).await?;
                        writer.flush().await?;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let replay = session.replay_bytes(replay_bytes.max(4096));
                        writer.write_all(&replay).await?;
                        writer.flush().await?;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    session.log_event("client_detached", None, None, None);
    Ok(())
}

async fn write_response(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    response: RpcResponse,
) -> Result<()> {
    let mut encoded = serde_json::to_vec(&response)?;
    encoded.push(b'\n');
    writer.write_all(&encoded).await?;
    writer.flush().await?;
    Ok(())
}

async fn route_request(request: RpcRequest, manager: Arc<SessionManager>) -> Result<Value> {
    match request.method.as_str() {
        "hello" => serde_json::to_value(HelloResult {
            protocol_version: PROTOCOL_VERSION,
            methods: vec![
                "hello".to_string(),
                "spawn".to_string(),
                "list".to_string(),
                "status".to_string(),
                "send".to_string(),
                "expect".to_string(),
                "capture".to_string(),
                "attach".to_string(),
                "resize".to_string(),
                "kill".to_string(),
                "rm".to_string(),
            ],
        })
        .map_err(Into::into),
        "spawn" => {
            let params: SpawnParams = decode_params(request.params)?;
            serde_json::to_value(manager.spawn(params)?).map_err(Into::into)
        }
        "list" => serde_json::to_value(manager.list()?).map_err(Into::into),
        "status" => {
            let params: NameParams = decode_params(request.params)?;
            serde_json::to_value(manager.status(&params.session)?).map_err(Into::into)
        }
        "send" => {
            let params: SendParams = decode_params(request.params)?;
            serde_json::to_value(manager.send(params)?).map_err(Into::into)
        }
        "expect" => {
            let params: ExpectParams = decode_params(request.params)?;
            serde_json::to_value(manager.expect(params).await?).map_err(Into::into)
        }
        "capture" => {
            let params: CaptureParams = decode_params(request.params)?;
            serde_json::to_value(manager.capture(params)?).map_err(Into::into)
        }
        "resize" => {
            let params: ResizeParams = decode_params(request.params)?;
            serde_json::to_value(manager.resize(params)?).map_err(Into::into)
        }
        "kill" => {
            let params: NameParams = decode_params(request.params)?;
            serde_json::to_value(manager.kill(&params.session)?).map_err(Into::into)
        }
        "rm" => {
            let params: NameParams = decode_params(request.params)?;
            serde_json::to_value(manager.remove(&params.session)?).map_err(Into::into)
        }
        other => bail!("unknown method {other}"),
    }
}

fn decode_params<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(Into::into)
}

#[derive(Default)]
struct SessionManager {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

impl SessionManager {
    fn spawn(&self, params: SpawnParams) -> Result<SessionInfo> {
        if params.name.trim().is_empty() {
            bail!("session name cannot be empty");
        }

        let mut sessions = self.sessions.lock().expect("sessions lock poisoned");
        if sessions.contains_key(&params.name) {
            bail!("session {} already exists", params.name);
        }

        let session = Session::spawn(params.name, params.command)?;
        let info = session.info();
        sessions.insert(info.name.clone(), session);
        Ok(info)
    }

    fn list(&self) -> Result<Vec<SessionInfo>> {
        Ok(self
            .sessions
            .lock()
            .expect("sessions lock poisoned")
            .values()
            .map(|session| session.info())
            .collect())
    }

    fn status(&self, name: &str) -> Result<SessionInfo> {
        Ok(self.get(name)?.info())
    }

    fn send(&self, params: SendParams) -> Result<SendResult> {
        self.get(&params.session)?.send(params)
    }

    async fn expect(&self, params: ExpectParams) -> Result<ExpectResult> {
        self.get(&params.session)?.expect(params).await
    }

    fn capture(&self, params: CaptureParams) -> Result<CaptureResult> {
        self.get(&params.session)?.capture(params)
    }

    fn resize(&self, params: ResizeParams) -> Result<ResizeResult> {
        self.get(&params.session)?.resize(params)
    }

    fn kill(&self, name: &str) -> Result<SessionInfo> {
        let session = self.get(name)?;
        session.kill()?;
        Ok(session.info())
    }

    fn remove(&self, name: &str) -> Result<RemoveResult> {
        let mut sessions = self.sessions.lock().expect("sessions lock poisoned");
        let session = sessions
            .get(name)
            .ok_or_else(|| anyhow!("session {name} not found"))?;
        if session.is_running() {
            bail!("session {name} is still running; kill it before rm");
        }
        sessions.remove(name);
        Ok(RemoveResult { removed: true })
    }

    fn get(&self, name: &str) -> Result<Arc<Session>> {
        self.sessions
            .lock()
            .expect("sessions lock poisoned")
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow!("session {name} not found"))
    }
}

struct Session {
    name: String,
    command: Vec<String>,
    cwd: Option<PathBuf>,
    pid: Option<u32>,
    started_at: String,
    raw_log: bool,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    inner: Mutex<SessionInner>,
    notify: Notify,
    output_tx: broadcast::Sender<Vec<u8>>,
}

struct SessionInner {
    cols: u16,
    rows: u16,
    state: SessionState,
    text: String,
    raw: Vec<u8>,
    events: Vec<EventRecord>,
    last_output_at: Option<String>,
    exit_status: Option<String>,
}

impl Session {
    fn spawn(name: String, command: CommandSpec) -> Result<Arc<Self>> {
        if command.program.trim().is_empty() {
            bail!("command program cannot be empty");
        }

        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: command.rows,
            cols: command.cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut builder = CommandBuilder::new(&command.program);
        for arg in &command.args {
            builder.arg(arg);
        }
        if let Some(cwd) = &command.cwd {
            builder.cwd(cwd);
        }
        for (key, value) in &command.env {
            builder.env(key, value);
        }

        let child = pair.slave.spawn_command(builder)?;
        let pid = child.process_id();
        let killer = child.clone_killer();
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let command_vec = std::iter::once(command.program.clone())
            .chain(command.args.clone())
            .collect();

        let (output_tx, _) = broadcast::channel(1024);
        let session = Arc::new(Self {
            name,
            command: command_vec,
            cwd: command.cwd,
            pid,
            started_at: now(),
            raw_log: command.raw_log,
            writer: Mutex::new(writer),
            killer: Mutex::new(killer),
            master: Mutex::new(pair.master),
            inner: Mutex::new(SessionInner {
                cols: command.cols,
                rows: command.rows,
                state: SessionState::Running,
                text: String::new(),
                raw: Vec::new(),
                events: vec![],
                last_output_at: None,
                exit_status: None,
            }),
            notify: Notify::new(),
            output_tx,
        });

        session.log_event("session_created", None, None, None);
        session.log_event("process_started", None, None, None);
        spawn_reader_thread(Arc::clone(&session), &mut reader);
        spawn_wait_thread(Arc::clone(&session), child);
        Ok(session)
    }

    fn info(&self) -> SessionInfo {
        let inner = self.inner.lock().expect("session lock poisoned");
        SessionInfo {
            name: self.name.clone(),
            command: self.command.clone(),
            cwd: self.cwd.clone(),
            pid: self.pid,
            cols: inner.cols,
            rows: inner.rows,
            started_at: self.started_at.clone(),
            state: inner.state.clone(),
            output_offset: inner.text.len() as u64,
            last_output_at: inner.last_output_at.clone(),
            exit_status: inner.exit_status.clone(),
        }
    }

    fn is_running(&self) -> bool {
        matches!(
            self.inner.lock().expect("session lock poisoned").state,
            SessionState::Running
        )
    }

    fn send(&self, params: SendParams) -> Result<SendResult> {
        if !self.is_running() {
            bail!("session {} is not running", self.name);
        }

        let mut data = params.data.into_bytes();
        if params.append_newline {
            data.push(b'\n');
        }

        {
            let mut writer = self.writer.lock().expect("writer lock poisoned");
            writer.write_all(&data)?;
            writer.flush()?;
        }

        let preview = if params.secret {
            Some("[REDACTED]".to_string())
        } else {
            Some(String::from_utf8_lossy(&data).chars().take(120).collect())
        };
        self.log_event(
            if params.secret {
                "pty_secret_write"
            } else {
                "pty_write"
            },
            None,
            Some(data.len()),
            preview,
        );

        Ok(SendResult {
            bytes: data.len(),
            redacted: params.secret,
        })
    }

    fn write_attach_input(&self, data: &[u8]) -> Result<()> {
        if !self.is_running() {
            bail!("session {} is not running", self.name);
        }

        {
            let mut writer = self.writer.lock().expect("writer lock poisoned");
            writer.write_all(data)?;
            writer.flush()?;
        }

        self.log_event("attach_input", None, Some(data.len()), None);
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.output_tx.subscribe()
    }

    fn replay_bytes(&self, replay_bytes: usize) -> Vec<u8> {
        if replay_bytes == 0 {
            return vec![];
        }

        let inner = self.inner.lock().expect("session lock poisoned");
        if inner.raw.is_empty() {
            let bytes = inner.text.as_bytes();
            let start = bytes.len().saturating_sub(replay_bytes);
            bytes[start..].to_vec()
        } else {
            let start = inner.raw.len().saturating_sub(replay_bytes);
            inner.raw[start..].to_vec()
        }
    }

    async fn expect(&self, params: ExpectParams) -> Result<ExpectResult> {
        let compiled = if params.regex {
            Some(Regex::new(&params.pattern)?)
        } else {
            None
        };
        let timeout = Duration::from_millis(params.timeout_ms);
        let started = Instant::now();
        let deadline = started + timeout;
        let start_offset = self.expect_start_offset(&params);

        self.log_event("expect_started", Some(start_offset), None, None);

        loop {
            if let Some(result) =
                self.find_match(&params.pattern, compiled.as_ref(), start_offset, started)?
            {
                self.log_event(
                    "expect_matched",
                    Some(result.match_start_offset),
                    Some(result.matched.len()),
                    Some(result.matched.clone()),
                );
                return Ok(result);
            }

            if Instant::now() >= deadline {
                self.log_event("expect_timed_out", Some(start_offset), None, None);
                bail!(
                    "timed out after {}ms waiting for {}",
                    params.timeout_ms,
                    params.pattern
                );
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if tokio::time::timeout(remaining, self.notify.notified())
                .await
                .is_err()
            {
                self.log_event("expect_timed_out", Some(start_offset), None, None);
                bail!(
                    "timed out after {}ms waiting for {}",
                    params.timeout_ms,
                    params.pattern
                );
            }
        }
    }

    fn expect_start_offset(&self, params: &ExpectParams) -> u64 {
        if let Some(offset) = params.from_offset {
            offset
        } else if params.from_start || params.include_history {
            0
        } else {
            self.inner.lock().expect("session lock poisoned").text.len() as u64
        }
    }

    fn find_match(
        &self,
        pattern: &str,
        regex: Option<&Regex>,
        start_offset: u64,
        started: Instant,
    ) -> Result<Option<ExpectResult>> {
        let inner = self.inner.lock().expect("session lock poisoned");
        let start = clamp_char_boundary(&inner.text, start_offset as usize);
        let haystack = &inner.text[start..];
        let found = if let Some(regex) = regex {
            regex
                .find(haystack)
                .map(|match_| (match_.start(), match_.end()))
        } else {
            haystack
                .find(pattern)
                .map(|index| (index, index + pattern.len()))
        };

        let Some((relative_start, relative_end)) = found else {
            return Ok(None);
        };

        let absolute_start = start + relative_start;
        let absolute_end = start + relative_end;
        let before = inner.text[start..absolute_start].to_string();
        let matched = inner.text[absolute_start..absolute_end].to_string();
        let after = inner.text[absolute_end..].to_string();
        let recent_start = inner.text.len().saturating_sub(4096);

        Ok(Some(ExpectResult {
            matched,
            before,
            after,
            recent: inner.text[clamp_char_boundary(&inner.text, recent_start)..].to_string(),
            start_offset: start as u64,
            match_start_offset: absolute_start as u64,
            match_end_offset: absolute_end as u64,
            elapsed_ms: started.elapsed().as_millis(),
        }))
    }

    fn capture(&self, params: CaptureParams) -> Result<CaptureResult> {
        let inner = self.inner.lock().expect("session lock poisoned");
        let text = if let Some(last_lines) = params.last_lines {
            last_n_lines(&inner.text, last_lines)
        } else {
            inner.text.clone()
        };

        Ok(CaptureResult {
            text,
            offset: inner.text.len() as u64,
        })
    }

    fn resize(&self, params: ResizeParams) -> Result<ResizeResult> {
        self.master
            .lock()
            .expect("pty lock poisoned")
            .resize(PtySize {
                rows: params.rows,
                cols: params.cols,
                pixel_width: 0,
                pixel_height: 0,
            })?;

        {
            let mut inner = self.inner.lock().expect("session lock poisoned");
            inner.cols = params.cols;
            inner.rows = params.rows;
        }

        self.log_event(
            "resize",
            None,
            None,
            Some(format!("{}x{}", params.cols, params.rows)),
        );
        Ok(ResizeResult {
            cols: params.cols,
            rows: params.rows,
        })
    }

    fn kill(&self) -> Result<()> {
        {
            let mut killer = self.killer.lock().expect("killer lock poisoned");
            killer.kill()?;
        }
        {
            let mut inner = self.inner.lock().expect("session lock poisoned");
            inner.state = SessionState::Killed;
            inner.exit_status = Some("killed".to_string());
        }
        self.log_event("session_killed", None, None, None);
        self.notify.notify_waiters();
        Ok(())
    }

    fn mark_exited(&self, status: String) {
        let changed = {
            let mut inner = self.inner.lock().expect("session lock poisoned");
            if matches!(inner.state, SessionState::Running) {
                inner.state = SessionState::Exited;
                inner.exit_status = Some(status);
                true
            } else {
                false
            }
        };
        if changed {
            self.log_event("process_exited", None, None, None);
            self.notify.notify_waiters();
        }
    }

    fn log_event(
        &self,
        event: impl Into<String>,
        offset: Option<u64>,
        bytes: Option<usize>,
        preview: Option<String>,
    ) {
        let mut inner = self.inner.lock().expect("session lock poisoned");
        inner.events.push(EventRecord {
            ts: now(),
            session: self.name.clone(),
            event: event.into(),
            offset,
            bytes,
            preview,
        });
    }
}

fn spawn_reader_thread(session: Arc<Session>, reader: &mut Box<dyn Read + Send>) {
    let mut reader = std::mem::replace(reader, Box::new(std::io::empty()));
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    session.log_event("pty_eof", None, None, None);
                    session.notify.notify_waiters();
                    return;
                }
                Ok(n) => {
                    let ts = now();
                    let chunk = &buffer[..n];
                    let preview = String::from_utf8_lossy(chunk).chars().take(120).collect();
                    let offset = {
                        let mut inner = session.inner.lock().expect("session lock poisoned");
                        if session.raw_log {
                            inner.raw.extend_from_slice(chunk);
                        }
                        inner.text.push_str(&String::from_utf8_lossy(chunk));
                        inner.last_output_at = Some(ts.clone());
                        let offset = inner.text.len() as u64;
                        inner.events.push(EventRecord {
                            ts,
                            session: session.name.clone(),
                            event: "pty_read".to_string(),
                            offset: Some(offset),
                            bytes: Some(n),
                            preview: Some(preview),
                        });
                        offset
                    };
                    let _ = session.output_tx.send(chunk.to_vec());
                    tracing::debug!(session = %session.name, offset, bytes = n, "pty read");
                    session.notify.notify_waiters();
                }
                Err(err) => {
                    session.log_event("pty_read_error", None, None, Some(err.to_string()));
                    session.notify.notify_waiters();
                    return;
                }
            }
        }
    });
}

fn spawn_wait_thread(session: Arc<Session>, mut child: Box<dyn Child + Send>) {
    std::thread::spawn(move || match child.wait() {
        Ok(status) => session.mark_exited(status.to_string()),
        Err(err) => session.mark_exited(format!("wait_error: {err}")),
    });
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn clamp_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn last_n_lines(text: &str, n: usize) -> String {
    if n == 0 {
        return String::new();
    }

    let mut lines = text.lines().rev().take(n).collect::<Vec<_>>();
    lines.reverse();
    let mut output = lines.join("\n");
    if text.ends_with('\n') && !output.is_empty() {
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_lines_limits_from_end() {
        assert_eq!(last_n_lines("a\nb\nc\n", 2), "b\nc\n");
    }

    #[test]
    fn clamp_offset_to_valid_utf8_boundary() {
        let text = "aéz";
        assert_eq!(clamp_char_boundary(text, 2), 1);
        assert_eq!(clamp_char_boundary(text, 3), 3);
    }
}
