//! One long-lived framed connection to herdr-core through `hc connect`.
//!
//! The connection starts in plain mode. `hello {framed: true}` switches it
//! after its reply; from then on channel 0 carries JSON-RPC and every other
//! channel is a stream. Calls are correlated by id; events and attach bytes
//! are pushed to their receivers. No `hc` is spawned per user action.

use super::discovery;
use super::events::{self, EventStream, Notification};
use super::frames::{Frame, FrameDecoder};
use super::rpc::{
    AttachResult, DocGetResult, DocSetResult, Hello, ReadResult, RpcError, Snapshot, Terminal,
};
use super::{Error, Result};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// How long a call waits for its reply before giving up.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the `hello` handshake may take before the connection is abandoned.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Shared label writes must not block the UI for the 10 s call default.
pub const LABEL_TIMEOUT: Duration = Duration::from_secs(2);

const READ_CHUNK: usize = 64 * 1024;
const MAX_HELLO_LINE: usize = 1 << 20;

/// What an attach stream delivers.
#[derive(Clone, Debug)]
pub enum StreamItem {
    /// Raw program output (first paint, then live bytes).
    Data(Vec<u8>),
    /// Core ended the stream; the JSON reason says why (for example an exit).
    Closed(Option<Value>),
    /// The connection dropped. The program may still be running.
    Lost,
}

/// An attached terminal: its channel, its record, and the byte stream.
pub struct Attached {
    pub channel: u32,
    pub terminal: Terminal,
    pub stream: Receiver<StreamItem>,
}

/// What `terminal.run` starts and how. The defaults carry the identity HerdR
/// Terminal starts every terminal with.
#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Program and arguments; empty starts core's own shell.
    pub argv: Vec<String>,
    /// A name unique among live terminals. It lets a client reclaim its own
    /// terminal when the `terminal.run` reply is lost with the connection.
    pub name: Option<String>,
    /// Set on top of core's environment.
    pub env: BTreeMap<String, String>,
    /// Labels stored with the terminal.
    pub labels: BTreeMap<String, String>,
    /// Remove the terminal once it exits.
    pub rm: bool,
    /// Replace a lost program with a fresh shell in place.
    pub keep: bool,
    /// Do not let an ambient terminal become its parent.
    pub no_parent: bool,
    /// The grid the terminal should start at, when the caller knows the size
    /// it will attach with. Older cores ignore the field; a core that applies
    /// it starts the program at this size before the first attach.
    pub size: Option<(u16, u16)>,
}

impl Default for RunOptions {
    fn default() -> RunOptions {
        RunOptions {
            argv: Vec::new(),
            name: None,
            env: client_env(),
            labels: client_labels(),
            rm: false,
            keep: false,
            no_parent: true,
            size: None,
        }
    }
}

/// The environment every HerdR Terminal terminal is started with.
pub fn client_env() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("COLORTERM".into(), "truecolor".into());
    env.insert("TERM_PROGRAM".into(), "herdr".into());
    env
}

/// Labels that let this client find its own terminals again.
pub fn client_labels() -> BTreeMap<String, String> {
    BTreeMap::from([("herdr.client".to_owned(), "herdr-terminal".to_owned())])
}

/// The `terminal.run` params. `size` is included only when the caller knows
/// the grid the terminal will attach at; cores that predate the field ignore
/// it, and a core that applies it starts the program at that size.
fn run_params(options: &RunOptions) -> Value {
    let mut params = json!({
        "argv": &options.argv,
        "env": &options.env,
        "labels": &options.labels,
        "rm": options.rm,
        "keep": options.keep,
        "no_parent": options.no_parent,
    });
    if let Some(name) = &options.name {
        params["name"] = json!(name);
    }
    if let Some((cols, rows)) = options.size {
        params["size"] = json!({"cols": cols, "rows": rows});
    }
    params
}

/// One framed connection to core.
pub struct CoreClient {
    hello: Hello,
    writer: Arc<Mutex<ChildStdin>>,
    routes: Arc<Mutex<Routes>>,
    next_id: AtomicU64,
    child: Mutex<Option<Child>>,
}

impl CoreClient {
    /// Discovers `hc` (unless one is given), connects, and performs the framed
    /// `hello` handshake. Fails clearly when core cannot serve this client.
    pub fn connect(hc: Option<&Path>) -> Result<CoreClient> {
        Self::connect_timeout(hc, HANDSHAKE_TIMEOUT)
    }

    /// [`CoreClient::connect`] with a deadline for the `hello` exchange, so a
    /// core that accepts a connection and never answers cannot stall a
    /// reconnect loop forever.
    pub fn connect_timeout(hc: Option<&Path>, timeout: Duration) -> Result<CoreClient> {
        let bin = match hc {
            Some(path) => {
                if discovery::version(path).is_none() {
                    return Err(Error::Protocol(format!(
                        "{} is not herdr-core: `--version` did not print `hc ...`",
                        path.display()
                    )));
                }
                path.to_path_buf()
            }
            None => discovery::find(None).ok_or(Error::NotFound)?,
        };
        let mut child = Command::new(&bin)
            .arg("connect")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|source| Error::Spawn {
                path: bin.clone(),
                source,
            })?;
        let (stdin, stdout) = match (child.stdin.take(), child.stdout.take()) {
            (Some(stdin), Some(stdout)) => (stdin, stdout),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Protocol(
                    "hc connect did not provide stdin/stdout".into(),
                ));
            }
        };
        // The handshake owns both pipes, then hands them back. A separate
        // thread lets the deadline interrupt a core that answers nothing.
        let (tx, rx) = mpsc::channel();
        if let Err(source) = std::thread::Builder::new()
            .name("ht-core-hello".into())
            .spawn(move || {
                let mut stdin = stdin;
                let mut stdout = stdout;
                let result = handshake(&mut stdin, &mut stdout).map(|hello| (hello, stdin, stdout));
                let _ = tx.send(result);
            })
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Io(source));
        }
        let (hello, stdin, stdout) = match rx.recv_timeout(timeout) {
            Ok(Ok(parts)) => parts,
            Ok(Err(error)) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Timeout {
                    method: "hello".into(),
                    timeout,
                });
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Closed);
            }
        };

        let writer = Arc::new(Mutex::new(stdin));
        let routes = Arc::new(Mutex::new(Routes::default()));
        let reader_routes = routes.clone();
        let reader_writer = writer.clone();
        if let Err(source) = std::thread::Builder::new()
            .name("ht-core-reader".into())
            .spawn(move || read_loop(stdout, reader_routes, reader_writer))
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Io(source));
        }
        Ok(CoreClient {
            hello,
            writer,
            routes,
            next_id: AtomicU64::new(1),
            child: Mutex::new(Some(child)),
        })
    }

    /// What core said at the handshake (generation, version, methods, features).
    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    /// One JSON-RPC call, waiting up to [`DEFAULT_TIMEOUT`].
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_timeout(method, params, DEFAULT_TIMEOUT)
    }

    pub fn call_timeout(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply_tx, reply_rx) = mpsc::channel();
        {
            let mut routes = lock(&self.routes);
            if routes.closed {
                return Err(Error::Closed);
            }
            routes.replies.insert(id, reply_tx);
        }
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if let Err(error) = self.write(Frame::data(0, request.to_string().into_bytes())) {
            lock(&self.routes).replies.remove(&id);
            return Err(error);
        }
        match reply_rx.recv_timeout(timeout) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(Error::Rpc {
                method: method.to_owned(),
                error,
            }),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                lock(&self.routes).replies.remove(&id);
                Err(Error::Timeout {
                    method: method.to_owned(),
                    timeout,
                })
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(Error::Closed),
        }
    }

    /// Subscribes to events: the snapshot plus notifications on their channel.
    ///
    /// M0 keeps one subscription per connection: calling this again replaces
    /// the first route, and the earlier [`EventStream`] stops receiving.
    pub fn subscribe(&self) -> Result<EventStream> {
        let value = self.call("events.subscribe", json!({}))?;
        let snapshot: Snapshot = serde_json::from_value(value)?;
        let channel = snapshot.channel.ok_or_else(|| {
            Error::Protocol(
                "events.subscribe returned no channel; a framed connection is required".into(),
            )
        })?;
        let (tx, rx) = mpsc::channel();
        {
            let mut routes = lock(&self.routes);
            if routes.closed {
                return Err(Error::Closed);
            }
            let early = routes.early.remove(&channel).unwrap_or_default();
            let ended = early
                .iter()
                .any(|item| !matches!(item, StreamItem::Data(_)));
            let mut credits = 0usize;
            if ended {
                // Core closed the channel before we registered it. Deliver
                // what arrived, then let the receiver report the end.
                let mut lines = events::LineBuffer::default();
                for item in early {
                    if let StreamItem::Data(bytes) = item {
                        credits += bytes.len();
                        for value in lines.push(&bytes) {
                            if let Some(notification) = events::parse_notification(value) {
                                let _ = tx.send(notification);
                            }
                        }
                    }
                }
            } else {
                routes.events = Some(EventsRoute {
                    channel,
                    tx,
                    lines: events::LineBuffer::default(),
                });
                let route = routes.events.as_mut().expect("just installed");
                for item in early {
                    if let StreamItem::Data(bytes) = item {
                        credits += bytes.len();
                        for value in route.lines.push(&bytes) {
                            if let Some(notification) = events::parse_notification(value) {
                                let _ = route.tx.send(notification);
                            }
                        }
                    }
                }
            }
            drop(routes);
            if credits > 0 {
                let _ = self.credit(channel, credits);
            }
        }
        Ok(EventStream::new(snapshot, rx))
    }

    /// Starts a program in a new core terminal.
    pub fn run(&self, options: &RunOptions) -> Result<Terminal> {
        let value = self.call("terminal.run", run_params(options))?;
        let terminal: Terminal = serde_json::from_value(value.clone())?;
        if terminal.address.is_empty() {
            return Err(Error::Protocol(format!(
                "terminal.run returned no address: {value}"
            )));
        }
        Ok(terminal)
    }

    /// Sets or removes (with an empty value) labels on a terminal, merging
    /// them with the labels it already carries. Returns the updated record
    /// with a bounded timeout: shared attention labels never stall the UI.
    pub fn label(&self, terminal: &str, labels: &BTreeMap<String, String>) -> Result<Terminal> {
        let value = self.call_timeout(
            "terminal.label",
            json!({"terminal": terminal, "labels": labels}),
            LABEL_TIMEOUT,
        )?;
        let record: Terminal = serde_json::from_value(value.clone())?;
        if record.address.is_empty() {
            return Err(Error::Protocol(format!(
                "terminal.label returned no address: {value}"
            )));
        }
        Ok(record)
    }

    /// Opens a raw stream to a live terminal at the given size, asking for up
    /// to `history` lines above the screen so local scrollback also covers
    /// output the program produced before this client attached.
    pub fn attach(&self, terminal: &str, cols: u16, rows: u16, history: usize) -> Result<Attached> {
        let value = self.call(
            "terminal.attach",
            json!({
                "terminal": terminal,
                "cols": cols,
                "rows": rows,
                "history": history,
            }),
        )?;
        let result: AttachResult = serde_json::from_value(value)?;
        let (tx, rx) = mpsc::channel();
        {
            let mut routes = lock(&self.routes);
            if routes.closed {
                return Err(Error::Closed);
            }
            // Frames for this channel may already have arrived between the
            // reply and this registration; the 256 KiB start window bounds
            // them. Deliver them in order under the same lock the reader uses.
            let early = routes.early.remove(&result.channel).unwrap_or_default();
            let ended = early
                .iter()
                .any(|item| !matches!(item, StreamItem::Data(_)));
            if !ended {
                routes.streams.insert(result.channel, tx.clone());
            }
            for item in early {
                let _ = tx.send(item);
            }
        }
        Ok(Attached {
            channel: result.channel,
            terminal: result.terminal,
            stream: rx,
        })
    }

    /// Reads a versioned opaque document. An absent document is
    /// `value: null, version: 0`.
    pub fn doc_get(&self, name: &str, timeout: Duration) -> Result<DocGetResult> {
        let value = self.call_timeout("doc.get", json!({"name": name}), timeout)?;
        Ok(serde_json::from_value(value)?)
    }

    /// Writes a versioned opaque document, or deletes it with a null value.
    /// A stale `version` fails with error kind `conflict`.
    pub fn doc_set(
        &self,
        name: &str,
        value: &Value,
        version: u64,
        timeout: Duration,
    ) -> Result<DocSetResult> {
        let value = self.call_timeout(
            "doc.set",
            json!({"name": name, "value": value, "version": version}),
            timeout,
        )?;
        Ok(serde_json::from_value(value)?)
    }

    /// The terminal's record, live or finished.
    pub fn get(&self, terminal: &str) -> Result<Terminal> {
        let value = self.call("terminal.get", json!({"terminal": terminal}))?;
        let terminal: Terminal = serde_json::from_value(value.clone())?;
        if terminal.address.is_empty() {
            return Err(Error::Protocol(format!(
                "terminal.get returned no address: {value}"
            )));
        }
        Ok(terminal)
    }

    /// Reads a terminal's screen text (with `lines` of history above it).    /// Works for a finished terminal too: core keeps its final screen.
    pub fn read(&self, terminal: &str, lines: usize) -> Result<ReadResult> {
        let value = self.call(
            "terminal.read",
            json!({"terminal": terminal, "lines": lines}),
        )?;
        Ok(serde_json::from_value(value)?)
    }

    /// Forgets a finished terminal and its record.
    pub fn rm(&self, terminal: &str) -> Result<()> {
        self.call("terminal.rm", json!({"terminal": terminal}))?;
        Ok(())
    }

    /// Ends a terminal's program.
    pub fn kill(&self, terminal: &str) -> Result<()> {
        self.call("terminal.kill", json!({"terminal": terminal}))?;
        Ok(())
    }

    /// Sends key (or paste) bytes as DATA on an attach channel.
    pub fn keys(&self, channel: u32, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.write(Frame::data(channel, bytes.to_vec()))
    }

    /// Tells core how many stream bytes the viewer has consumed.
    pub fn credit(&self, channel: u32, bytes: usize) -> Result<()> {
        if bytes == 0 {
            return Ok(());
        }
        self.write(Frame::credit(channel, bytes))
    }

    /// Tells core the viewer's size changed.
    pub fn resize(&self, channel: u32, cols: u16, rows: u16) -> Result<()> {
        self.write(Frame::resize(channel, cols, rows))
    }

    /// Tells core the viewer gained or lost focus.
    pub fn focus(&self, channel: u32, focused: bool) -> Result<()> {
        self.write(Frame::focus(channel, focused))
    }

    /// Tells core the viewer's colors or cell size changed.
    pub fn facts(&self, channel: u32, display: &super::rpc::Display) -> Result<()> {
        let facts = serde_json::to_value(display)?;
        self.write(Frame::facts(channel, &facts))
    }

    /// Detaches from an attach channel (CLOSE). The program keeps running.
    pub fn detach(&self, channel: u32) -> Result<()> {
        {
            let mut routes = lock(&self.routes);
            routes.streams.remove(&channel);
            routes.retired.insert(channel);
        }
        self.write(Frame::close(channel, None))
    }

    fn write(&self, frame: Frame) -> Result<()> {
        write_frame(&self.writer, frame)
    }
}

impl Drop for CoreClient {
    fn drop(&mut self) {
        if let Some(mut child) = lock(&self.child).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[derive(Default)]
struct Routes {
    /// The connection is over; new calls fail at once.
    closed: bool,
    /// Calls waiting for a reply, by id.
    replies: HashMap<u64, Sender<std::result::Result<Value, RpcError>>>,
    /// Attach streams, by channel.
    streams: HashMap<u32, Sender<StreamItem>>,
    /// The one event subscription, when framed.
    events: Option<EventsRoute>,
    /// Frames for a channel whose call reply has not been handled yet.
    early: HashMap<u32, Vec<StreamItem>>,
    /// Channels that are over; late frames for them are dropped.
    retired: HashSet<u32>,
}

struct EventsRoute {
    channel: u32,
    tx: Sender<Notification>,
    lines: events::LineBuffer,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Plain line mode before `hello`, then framed for the life of the connection.
/// The caller runs this with a deadline, so a silent core is abandoned rather
/// than stalling the client.
fn handshake(stdin: &mut ChildStdin, stdout: &mut ChildStdout) -> Result<Hello> {
    let request = json!({"jsonrpc": "2.0", "id": 0, "method": "hello", "params": {"framed": true}});
    let mut line = request.to_string().into_bytes();
    line.push(b'\n');
    stdin.write_all(&line)?;
    stdin.flush()?;
    let reply = read_line(stdout)?;
    let reply: Value = serde_json::from_slice(&reply)?;
    if let Some(error) = reply.get("error") {
        return Err(Error::Rpc {
            method: "hello".into(),
            error: RpcError::from_json(error),
        });
    }
    let result = reply.get("result").cloned().unwrap_or(Value::Null);
    let hello: Hello = serde_json::from_value(result)?;
    hello.check()?;
    Ok(hello)
}

/// Reads one plain-mode line byte by byte, so no framed byte is consumed
/// before the reader thread starts.
fn read_line(reader: &mut impl Read) -> Result<Vec<u8>> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => {
                return Err(Error::Protocol(
                    "hc connect closed before the hello reply".into(),
                ));
            }
            Ok(_) if byte[0] == b'\n' => return Ok(line),
            Ok(_) => {
                line.push(byte[0]);
                if line.len() > MAX_HELLO_LINE {
                    return Err(Error::Protocol("hello reply is too long".into()));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(Error::Io(error)),
        }
    }
}

fn read_loop(mut stdout: ChildStdout, routes: Arc<Mutex<Routes>>, writer: Arc<Mutex<ChildStdin>>) {
    let mut decoder = FrameDecoder::new();
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        match stdout.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                decoder.push(&buf[..n]);
                loop {
                    match decoder.next_frame() {
                        Ok(Some(frame)) => dispatch(&routes, &writer, frame),
                        Ok(None) => break,
                        Err(_) => {
                            // A stream that cannot be framed is over.
                            routes_closed(routes);
                            return;
                        }
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    routes_closed(routes);
}

fn dispatch(routes: &Arc<Mutex<Routes>>, writer: &Arc<Mutex<ChildStdin>>, frame: Frame) {
    if frame.channel == 0 {
        if frame.kind == super::frames::DATA
            && let Ok(message) = serde_json::from_slice::<Value>(&frame.payload)
        {
            dispatch_message(routes, message);
        }
        return;
    }

    let mut credit = 0usize;
    let mut notifications = None;
    {
        let mut routes = lock(routes);
        if routes.closed || routes.retired.contains(&frame.channel) {
            return;
        }
        let events_channel = routes.events.as_ref().map(|route| route.channel);
        if events_channel == Some(frame.channel) {
            match frame.kind {
                super::frames::DATA => {
                    let route = routes.events.as_mut().expect("checked above");
                    credit = frame.payload.len();
                    notifications = Some((route.tx.clone(), route.lines.push(&frame.payload)));
                }
                super::frames::CLOSE => {
                    routes.events = None;
                    routes.retired.insert(frame.channel);
                }
                _ => {}
            }
        } else if let Some(tx) = routes.streams.get(&frame.channel).cloned() {
            match frame.kind {
                super::frames::DATA => {
                    let _ = tx.send(StreamItem::Data(frame.payload));
                }
                super::frames::CLOSE => {
                    routes.streams.remove(&frame.channel);
                    routes.retired.insert(frame.channel);
                    let _ = tx.send(StreamItem::Closed(close_reason(&frame.payload)));
                }
                _ => {}
            }
        } else {
            // The reply for this channel has not been registered yet.
            match frame.kind {
                super::frames::DATA => routes
                    .early
                    .entry(frame.channel)
                    .or_default()
                    .push(StreamItem::Data(frame.payload)),
                super::frames::CLOSE => {
                    routes.retired.insert(frame.channel);
                    routes
                        .early
                        .entry(frame.channel)
                        .or_default()
                        .push(StreamItem::Closed(close_reason(&frame.payload)));
                }
                _ => {}
            }
        }
    }
    if let Some((tx, values)) = notifications {
        for value in values {
            if let Some(notification) = events::parse_notification(value) {
                let _ = tx.send(notification);
            }
        }
    }
    if credit > 0 {
        // The reader has consumed these bytes; give core room for more.
        let _ = write_frame(writer, Frame::credit(frame.channel, credit));
    }
}

fn dispatch_message(routes: &Arc<Mutex<Routes>>, message: Value) {
    if let Some(id) = message.get("id").and_then(Value::as_u64) {
        let reply = match message.get("error") {
            Some(error) => Err(RpcError::from_json(error)),
            None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
        };
        let sender = lock(routes).replies.remove(&id);
        if let Some(sender) = sender {
            let _ = sender.send(reply);
        }
        return;
    }
    if message.get("method").is_some() {
        let sender = lock(routes).events.as_ref().map(|route| route.tx.clone());
        if let Some(sender) = sender
            && let Some(notification) = events::parse_notification(message)
        {
            let _ = sender.send(notification);
        }
    }
}

fn routes_closed(routes: Arc<Mutex<Routes>>) {
    let mut routes = lock(&routes);
    if routes.closed {
        return;
    }
    routes.closed = true;
    for (_, sender) in routes.replies.drain() {
        let _ = sender.send(Err(RpcError::lost()));
    }
    for (_, sender) in routes.streams.drain() {
        let _ = sender.send(StreamItem::Lost);
    }
    routes.events = None;
    routes.early.clear();
}

fn write_frame(writer: &Mutex<ChildStdin>, frame: Frame) -> Result<()> {
    let bytes = frame.encode()?;
    let mut writer = lock(writer);
    writer
        .write_all(&bytes)
        .and_then(|()| writer.flush())
        .map_err(|_| Error::Closed)
}

fn close_reason(payload: &[u8]) -> Option<Value> {
    if payload.is_empty() {
        None
    } else {
        serde_json::from_slice(payload).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_close_reason_is_json_or_absent() {
        assert_eq!(
            close_reason(b""),
            None,
            "a detach acknowledgement carries no reason"
        );
        let reason = json!({"reason": "exited", "exit_code": 0, "signal": null});
        assert_eq!(close_reason(reason.to_string().as_bytes()), Some(reason));
        assert_eq!(
            close_reason(b"not json"),
            None,
            "a broken reason does not end the world"
        );
    }

    #[test]
    fn run_params_carry_the_initial_size_only_when_it_is_known() {
        let mut options = RunOptions::default();
        assert!(run_params(&options).get("size").is_none());
        options.size = Some((78, 20));
        assert_eq!(
            run_params(&options)["size"],
            json!({"cols": 78, "rows": 20})
        );
    }
}
