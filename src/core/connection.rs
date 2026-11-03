//! One long-lived framed connection to herdr-core through `hc connect`.
//!
//! The connection starts in plain mode. `hello {framed: true}` switches it
//! after its reply; from then on channel 0 carries JSON-RPC and every other
//! channel is a stream. Calls are correlated by id; events and attach bytes
//! are pushed to their receivers. No `hc` is spawned per user action.

use super::discovery;
use super::events::{self, EventStream, Notification};
use super::frames::{Frame, FrameDecoder, FrameError, parse_resize};
use super::rpc::{
    AttachResult, DocGetResult, DocSetResult, Hello, Machine, MachineAdd, MachineListResult,
    RESIZE_NOTIFICATIONS, ReadResult, RpcError, Snapshot, Terminal, machine_params,
    machine_rename_params,
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

/// The machine manager refreshes its inventory while open; a slow or wedged
/// core must not hold the UI for the full call default.
pub const MACHINE_LIST_TIMEOUT: Duration = Duration::from_secs(2);

const READ_CHUNK: usize = 64 * 1024;
const MAX_HELLO_LINE: usize = 1 << 20;

/// What an attach stream delivers.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamItem {
    /// Raw program output (first paint, then live bytes).
    Data(Vec<u8>),
    /// The terminal's grid, sent by core to a confirmed observer before the
    /// paint that uses it. It carries no terminal bytes and costs no credit.
    Resize { cols: u16, rows: u16 },
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
    /// True only for an observer whose attach reply confirmed that RESIZE
    /// precedes every paint. Normal viewers always see `false`.
    pub resize_notifications: bool,
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
    /// Start the program on another machine this core added. The terminal's
    /// address is then `machine/t7`; core relays attach, input and resize.
    pub on: Option<String>,
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
            on: None,
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
/// it, and a core that applies it starts the program at that size. `on` is
/// included only when the terminal runs on another machine; every local start
/// keeps exactly the params it sent before.
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
    if let Some(machine) = &options.on {
        params["on"] = json!(machine);
    }
    params
}

/// The `terminal.attach` params. A normal viewer sends no observer fields at
/// all; an observer asks for `observe` and, when the core advertises it,
/// `resize_notifications`.
fn attach_params(
    terminal: &str,
    cols: u16,
    rows: u16,
    history: usize,
    observe: bool,
    resize_notifications: bool,
) -> Value {
    let mut params = json!({
        "terminal": terminal,
        "cols": cols,
        "rows": rows,
        "history": history,
    });
    if observe {
        params["observe"] = json!(true);
        if resize_notifications {
            params["resize_notifications"] = json!(true);
        }
    }
    params
}

/// Whether already-arrived frames contain the end of the stream. A RESIZE is
/// stream content, not an ending: treating it as one would drop the live
/// route and lose everything after it.
fn early_ended(early: &[StreamItem]) -> bool {
    early
        .iter()
        .any(|item| matches!(item, StreamItem::Closed(_)))
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
            let ended = early_ended(&early);
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
    /// output the program produced before this client attached. A normal
    /// viewer: it may type and its size is the one the program uses.
    pub fn attach(&self, terminal: &str, cols: u16, rows: u16, history: usize) -> Result<Attached> {
        self.attach_with(terminal, cols, rows, history, false)
    }

    /// Attaches as an observer: it never types or resizes, so it cannot steal
    /// the size from the viewer the person is using.
    ///
    /// When core advertises `resize_notifications`, the attach asks for them
    /// and requires the reply to confirm; a confirmed observer applies each
    /// RESIZE before the DATA that follows it. Without the feature there is no
    /// safe observer path, so this fails instead of attaching as a normal
    /// viewer.
    pub fn attach_observer(
        &self,
        terminal: &str,
        cols: u16,
        rows: u16,
        history: usize,
    ) -> Result<Attached> {
        if !self.hello.supports(RESIZE_NOTIFICATIONS) {
            return Err(Error::Protocol(format!(
                "this core does not advertise {RESIZE_NOTIFICATIONS}; observer attach is unavailable"
            )));
        }
        let attached = self.attach_with(terminal, cols, rows, history, true)?;
        if !attached.resize_notifications {
            // Core accepted the stream but cannot promise the ordering the
            // observer relies on. Leave no route, early frames or open channel
            // behind, then fail clearly.
            self.abandon(attached.channel);
            return Err(Error::Protocol(
                "core did not confirm resize notifications for the observer attach".into(),
            ));
        }
        Ok(attached)
    }

    fn attach_with(
        &self,
        terminal: &str,
        cols: u16,
        rows: u16,
        history: usize,
        observe: bool,
    ) -> Result<Attached> {
        let value = self.call(
            "terminal.attach",
            attach_params(terminal, cols, rows, history, observe, observe),
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
            let ended = early_ended(&early);
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
            resize_notifications: observe && result.resize_notifications,
        })
    }

    /// Forgets a channel this client is not going to use, so late frames are
    /// dropped and core sees the stream closed.
    fn abandon(&self, channel: u32) {
        {
            let mut routes = lock(&self.routes);
            routes.streams.remove(&channel);
            routes.early.remove(&channel);
            routes.retired.insert(channel);
        }
        let _ = self.write(Frame::close(channel, None));
    }

    /// The machines this core added, with the state of each line. A bounded
    /// timeout: the machine manager polls this while open, and a slow core
    /// must not stall the UI for the full call default.
    pub fn machine_list(&self) -> Result<Vec<Machine>> {
        let value = self.call_timeout("machine.list", json!({}), MACHINE_LIST_TIMEOUT)?;
        let result: MachineListResult = serde_json::from_value(value)?;
        Ok(result.machines)
    }

    /// Adds another machine. `params.target` is sent unchanged; core dials it,
    /// checks that hc answers there and derives the name. A failure
    /// (`invalid_params`, `name_taken`, `already_added`, `unreachable`,
    /// `hc_missing`, `incompatible`) is reported to the caller.
    pub fn machine_add(&self, params: &MachineAdd) -> Result<Machine> {
        let value = self.call("machine.add", serde_json::to_value(params)?)?;
        Ok(serde_json::from_value(value)?)
    }

    /// Dials a machine now instead of at its next retry and reports its line.
    pub fn machine_connect(&self, machine: &str) -> Result<Machine> {
        let value = self.call("machine.connect", machine_params(machine))?;
        Ok(serde_json::from_value(value)?)
    }

    /// Gives a machine a new name here. Only this core's name changes.
    pub fn machine_rename(&self, machine: &str, new_name: &str) -> Result<Machine> {
        let value = self.call("machine.rename", machine_rename_params(machine, new_name))?;
        Ok(serde_json::from_value(value)?)
    }

    /// Forgets a machine here. Its terminals keep running there; core emits
    /// `removed` for the mirrored records.
    pub fn machine_rm(&self, machine: &str) -> Result<()> {
        self.call("machine.rm", machine_params(machine))?;
        Ok(())
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
                        Ok(Some(frame)) => {
                            if dispatch(&routes, &writer, frame).is_err() {
                                // A malformed stream frame corrupts the
                                // framing itself; the connection is over.
                                routes_closed(routes);
                                return;
                            }
                        }
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

fn dispatch(
    routes: &Arc<Mutex<Routes>>,
    writer: &Arc<Mutex<ChildStdin>>,
    frame: Frame,
) -> std::result::Result<(), FrameError> {
    if frame.channel == 0 {
        if frame.kind == super::frames::DATA
            && let Ok(message) = serde_json::from_slice::<Value>(&frame.payload)
        {
            dispatch_message(routes, message);
        }
        return Ok(());
    }

    let mut credit = 0usize;
    let mut notifications = None;
    {
        let mut routes = lock(routes);
        if routes.closed || routes.retired.contains(&frame.channel) {
            return Ok(());
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
        } else {
            route_attach_frame(&mut routes, &frame)?;
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
    Ok(())
}

/// Routes one attach-channel frame to its live stream, or to the early queue
/// while the attach reply is still being handled. RESIZE is content, not an
/// ending, and carries no credit.
fn route_attach_frame(routes: &mut Routes, frame: &Frame) -> std::result::Result<(), FrameError> {
    if let Some(tx) = routes.streams.get(&frame.channel).cloned() {
        match frame.kind {
            super::frames::DATA => {
                let _ = tx.send(StreamItem::Data(frame.payload.clone()));
            }
            super::frames::RESIZE => {
                let (cols, rows) = parse_resize(&frame.payload)?;
                let _ = tx.send(StreamItem::Resize { cols, rows });
            }
            super::frames::CLOSE => {
                routes.streams.remove(&frame.channel);
                routes.retired.insert(frame.channel);
                let _ = tx.send(StreamItem::Closed(close_reason(&frame.payload)));
            }
            _ => {}
        }
    } else {
        match frame.kind {
            super::frames::DATA => routes
                .early
                .entry(frame.channel)
                .or_default()
                .push(StreamItem::Data(frame.payload.clone())),
            super::frames::RESIZE => {
                let (cols, rows) = parse_resize(&frame.payload)?;
                routes
                    .early
                    .entry(frame.channel)
                    .or_default()
                    .push(StreamItem::Resize { cols, rows });
            }
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
    Ok(())
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

    #[test]
    fn run_params_send_on_only_for_a_machine_start() {
        let mut options = RunOptions::default();
        let local = run_params(&options);
        assert!(
            local.get("on").is_none(),
            "a local start must not grow an on field: {local}"
        );
        options.on = Some("rohan".into());
        assert_eq!(run_params(&options)["on"], json!("rohan"));
        assert_eq!(
            run_params(&options)["labels"],
            json!(options.labels),
            "labels are unchanged by a machine start"
        );
    }

    #[test]
    fn a_normal_attach_sends_no_observer_fields() {
        let normal = attach_params("t7", 80, 24, 200, false, false);
        assert_eq!(
            normal,
            json!({"terminal": "t7", "cols": 80, "rows": 24, "history": 200}),
            "normal attach params are unchanged, byte for byte"
        );

        let observer = attach_params("t7", 80, 24, 0, true, true);
        assert_eq!(observer["observe"], json!(true));
        assert_eq!(observer["resize_notifications"], json!(true));
    }

    #[test]
    fn a_resize_is_stream_content_that_never_looks_like_an_ending() {
        let mut routes = Routes::default();
        let (tx, rx) = mpsc::channel();
        routes.streams.insert(7, tx);

        route_attach_frame(&mut routes, &Frame::resize(7, 120, 40)).expect("valid resize");
        assert_eq!(
            rx.try_recv().expect("resize item"),
            StreamItem::Resize {
                cols: 120,
                rows: 40
            }
        );
        assert!(!routes.retired.contains(&7), "the stream is still live");
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));

        // The same frame before registration is queued, not treated as CLOSE.
        let mut early_routes = Routes::default();
        route_attach_frame(&mut early_routes, &Frame::resize(9, 20, 5)).expect("valid resize");
        let early = early_routes.early.get(&9).expect("early queue");
        assert!(matches!(
            early.as_slice(),
            [StreamItem::Resize { cols: 20, rows: 5 }]
        ));
        assert!(!early_ended(early));
        assert!(!early_routes.retired.contains(&9));
    }

    #[test]
    fn a_malformed_resize_is_a_frame_error_and_does_not_panic() {
        let mut routes = Routes::default();
        let (tx, _rx) = mpsc::channel();
        routes.streams.insert(7, tx);
        for payload in [Vec::new(), vec![1], vec![1, 2, 3], vec![0; 5]] {
            let frame = Frame {
                channel: 7,
                kind: super::super::frames::RESIZE,
                payload: payload.clone(),
            };
            assert_eq!(
                route_attach_frame(&mut routes, &frame),
                Err(FrameError::BadResize { len: payload.len() })
            );
        }

        // An early malformed frame is refused the same way.
        let mut early_routes = Routes::default();
        let frame = Frame {
            channel: 9,
            kind: super::super::frames::RESIZE,
            payload: vec![1, 2],
        };
        assert_eq!(
            route_attach_frame(&mut early_routes, &frame),
            Err(FrameError::BadResize { len: 2 })
        );
        assert!(early_routes.early.get(&9).is_none_or(Vec::is_empty));
    }

    #[test]
    fn only_a_close_ends_the_early_frames() {
        assert!(!early_ended(&[]));
        assert!(!early_ended(&[StreamItem::Data(b"paint".to_vec())]));
        assert!(!early_ended(&[
            StreamItem::Resize { cols: 80, rows: 24 },
            StreamItem::Data(b"paint".to_vec()),
        ]));
        assert!(early_ended(&[
            StreamItem::Resize { cols: 80, rows: 24 },
            StreamItem::Closed(None),
        ]));
    }
}
