use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
pub const PROTOCOL_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest {
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub id: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: String,
    pub message: String,
}

impl RpcResponse {
    pub fn ok<T: Serialize>(id: impl Into<String>, result: T) -> Self {
        Self {
            id: id.into(),
            ok: true,
            result: Some(serde_json::to_value(result).expect("response result is serializable")),
            error: None,
        }
    }

    pub fn error(
        id: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            ok: false,
            result: None,
            error: Some(RpcError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandSpec {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
    #[serde(default = "default_raw_log")]
    pub raw_log: bool,
}

fn default_cols() -> u16 {
    80
}

fn default_rows() -> u16 {
    24
}

fn default_raw_log() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnParams {
    pub name: String,
    pub command: CommandSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NameParams {
    pub session: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloResult {
    pub protocol_version: u32,
    pub methods: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendParams {
    pub session: String,
    pub data: String,
    #[serde(default)]
    pub append_newline: bool,
    #[serde(default)]
    pub secret: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectParams {
    pub session: String,
    pub pattern: String,
    #[serde(default)]
    pub regex: bool,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub from_start: bool,
    #[serde(default)]
    pub include_history: bool,
    #[serde(default)]
    pub from_offset: Option<u64>,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_MS
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureParams {
    pub session: String,
    #[serde(default)]
    pub last_lines: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachParams {
    pub session: String,
    #[serde(default = "default_attach_replay_bytes")]
    pub replay_bytes: usize,
}

fn default_attach_replay_bytes() -> usize {
    4096
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResizeParams {
    pub session: String,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub name: String,
    pub command: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub pid: Option<u32>,
    pub cols: u16,
    pub rows: u16,
    pub started_at: String,
    pub state: SessionState,
    pub output_offset: u64,
    pub last_output_at: Option<String>,
    pub exit_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Running,
    Exited,
    Killed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendResult {
    pub bytes: usize,
    pub redacted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectResult {
    pub matched: String,
    pub before: String,
    pub after: String,
    pub recent: String,
    pub start_offset: u64,
    pub match_start_offset: u64,
    pub match_end_offset: u64,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureResult {
    pub text: String,
    pub offset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResizeResult {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveResult {
    pub removed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    pub ts: String,
    pub session: String,
    pub event: String,
    pub offset: Option<u64>,
    pub bytes: Option<usize>,
    pub preview: Option<String>,
}

pub fn default_socket_path() -> PathBuf {
    if let Ok(path) = std::env::var("BURNTTY_SOCKET") {
        return PathBuf::from(path);
    }

    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string());
    std::env::temp_dir().join(format!("burntty-{user}.sock"))
}
