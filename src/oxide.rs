use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};
use std::fmt;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_millis(3000);

/// The Epoch API contract major this build was written against. A daemon speaking a different
/// major refuses the call rather than answering with a shape epochctl may not understand.
const API_VERSION: u32 = 1;

#[derive(Debug)]
pub enum OxideError {
    NotRunning { socket: String, detail: String },
    Protocol(String),
    Backend(String),
}

impl fmt::Display for OxideError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning { socket, detail } => write!(
                f,
                "cannot reach EpochOxide at {socket} ({detail}).\n\
                 Start it with `systemctl --user start epochoxide.service`."
            ),
            Self::Protocol(message) => write!(f, "EpochOxide sent something unexpected: {message}"),
            Self::Backend(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for OxideError {}

impl OxideError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::NotRunning { .. } => 4,
            Self::Protocol(_) => 1,
            Self::Backend(_) => 1,
        }
    }
}

/// One item in a launcher result set. Only the fields epochctl renders or forwards are named;
/// the full item is kept so `--json` can pass EpochOxide's own shape through untouched.
#[derive(Debug, Clone, Deserialize)]
pub struct Item {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub identifier: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub subtext: String,
    #[serde(default)]
    pub score: i64,
    #[serde(default)]
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Provider {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub name_pretty: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub prefixes: Vec<String>,
    #[serde(default)]
    pub supports_query: bool,
}

/// A connection to EpochOxide's newline-delimited JSON socket.
///
/// This speaks the protocol directly rather than shelling out to the `epochoxide` binary: the
/// wire format is documented and stable, so a subprocess would only add startup cost and a
/// dependency on the binary being on PATH.
pub struct Client {
    socket: PathBuf,
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Client {
    pub fn connect(socket: &Path) -> Result<Self, OxideError> {
        let stream = UnixStream::connect(socket).map_err(|err| OxideError::NotRunning {
            socket: socket.display().to_string(),
            detail: err.to_string(),
        })?;
        let apply = |result: std::io::Result<()>| {
            result.map_err(|err| OxideError::Protocol(format!("configuring socket: {err}")))
        };
        apply(stream.set_read_timeout(Some(TIMEOUT)))?;
        apply(stream.set_write_timeout(Some(TIMEOUT)))?;
        let reader = BufReader::new(
            stream
                .try_clone()
                .map_err(|err| OxideError::Protocol(format!("cloning socket: {err}")))?,
        );
        Ok(Self {
            socket: socket.to_path_buf(),
            writer: stream,
            reader,
        })
    }

    fn send(&mut self, payload: &Value) -> Result<(), OxideError> {
        writeln!(self.writer, "{payload}").map_err(|err| OxideError::NotRunning {
            socket: self.socket.display().to_string(),
            detail: err.to_string(),
        })
    }

    /// Read one response frame. `Ok(None)` means the daemon closed the stream.
    fn recv(&mut self) -> Result<Option<Value>, OxideError> {
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => return Ok(None),
                Ok(_) => {
                    let line = line.trim();
                    if line.is_empty() {
                        return Ok(None);
                    }
                    let value: Value = serde_json::from_str(line)
                        .map_err(|err| OxideError::Protocol(format!("{err}: {line}")))?;
                    if value.get("ok").and_then(Value::as_bool) == Some(false) {
                        let message = value
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("request failed")
                            .to_string();
                        return Err(OxideError::Backend(message));
                    }
                    return Ok(Some(value.get("data").cloned().unwrap_or(value)));
                }
                Err(err) if err.kind() == ErrorKind::Interrupted => continue,
                Err(err)
                    if err.kind() == ErrorKind::WouldBlock || err.kind() == ErrorKind::TimedOut =>
                {
                    return Err(OxideError::NotRunning {
                        socket: self.socket.display().to_string(),
                        detail: "timed out waiting for a response".to_string(),
                    })
                }
                Err(err) => return Err(OxideError::Protocol(err.to_string())),
            }
        }
    }

    fn request(&mut self, payload: Value) -> Result<Value, OxideError> {
        self.send(&payload)?;
        self.recv()?
            .ok_or_else(|| OxideError::Protocol("connection closed without a response".to_string()))
    }

    /// Stop bounding how long a call may take.
    ///
    /// The default is right for a query the daemon answers from memory, but a capture waits on the
    /// user drawing a rectangle, and there is no sensible guess for how long that takes.
    pub fn set_read_timeout(&mut self, timeout: Option<Duration>) -> Result<(), OxideError> {
        self.writer
            .set_read_timeout(timeout)
            .map_err(|err| OxideError::Protocol(format!("configuring socket: {err}")))
    }

    /// Call a method on the Epoch API. `method` is `group.method`, as `api.describe` lists them.
    pub fn api(&mut self, method: &str, params: Value) -> Result<Value, OxideError> {
        self.request(json!({
            "type": "api",
            "method": method,
            "params": params,
            "version": API_VERSION,
        }))
    }

    pub fn providers(&mut self) -> Result<Vec<Provider>, OxideError> {
        let data = self.request(json!({ "type": "providers" }))?;
        serde_json::from_value(data).map_err(|err| OxideError::Protocol(err.to_string()))
    }

    pub fn query(
        &mut self,
        providers: &[String],
        query: &str,
        limit: usize,
        exact: bool,
    ) -> Result<Vec<Item>, OxideError> {
        let data = self.request(json!({
            "type": "query",
            "providers": providers,
            "query": query,
            "limit": limit,
            "exact": exact,
            "stream": false,
        }))?;
        serde_json::from_value(data).map_err(|err| OxideError::Protocol(err.to_string()))
    }

    pub fn activate(
        &mut self,
        provider: &str,
        identifier: &str,
        action: &str,
        arguments: &str,
    ) -> Result<Value, OxideError> {
        self.request(json!({
            "type": "activate",
            "provider": provider,
            "identifier": identifier,
            "action": action,
            "query": "",
            "arguments": arguments,
        }))
    }

    pub fn menu(&mut self, name: &str) -> Result<Vec<Item>, OxideError> {
        let data = self.request(json!({ "type": "menu", "menu": name }))?;
        serde_json::from_value(data).map_err(|err| OxideError::Protocol(err.to_string()))
    }
}
