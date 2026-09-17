use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::event::*;

pub const RELIABLE_QUEUE_SIZE: usize = 16;
pub const MAX_CLIENTS: usize = 4;
const WRITE_TIMEOUT: Duration = Duration::from_millis(500);
const SOCKET_DIR_PERM: u32 = 0o700;
const SOCKET_FILE_PERM: u32 = 0o600;
const SOCKET_DIAL_TIMEOUT: Duration = Duration::from_millis(100);

pub type SnapshotFn = Arc<dyn Fn() -> StateEvent + Send + Sync>;

/// Where OSD events are published.
#[derive(Clone)]
pub enum Sink {
    Noop,
    Socket(Arc<SocketSink>),
}

impl Sink {
    pub fn publish(&self, event: impl Into<Event>) {
        if let Sink::Socket(sink) = self {
            sink.publish(event.into());
        }
    }

    pub async fn close(&self) -> Result<()> {
        match self {
            Sink::Noop => Ok(()),
            Sink::Socket(sink) => sink.close().await,
        }
    }
}

pub fn default_socket_path() -> PathBuf {
    match std::env::var("XDG_RUNTIME_DIR") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir).join("dictator").join("osd.sock"),
        _ => std::env::temp_dir()
            .join(format!("dictator-osd-{}", socket_user_id()))
            .join("osd.sock"),
    }
}

fn uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

fn socket_user_id() -> String {
    match std::env::var("USER") {
        Ok(user) if !user.is_empty() => sanitize_path_part(&user),
        _ => uid().to_string(),
    }
}

fn sanitize_path_part(value: &str) -> String {
    let out: String = value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if out.is_empty() {
        return uid().to_string();
    }
    out
}

struct Client {
    reliable: mpsc::Sender<Event>,
    latest_meter: Mutex<Option<MeterEvent>>,
    wake: Arc<Notify>,
    done: CancellationToken,
}

impl Client {
    fn publish_reliable(&self, event: Event) -> bool {
        if self.done.is_cancelled() {
            return false;
        }
        match self.reliable.try_send(event) {
            Ok(()) => {
                self.wake.notify_one();
                true
            }
            Err(_) => false,
        }
    }

    fn publish_meter(&self, event: MeterEvent) {
        *self.latest_meter.lock().unwrap() = Some(event);
        self.wake.notify_one();
    }

    fn clear_meter(&self) {
        *self.latest_meter.lock().unwrap() = None;
    }

    fn take_meter(&self) -> Option<MeterEvent> {
        self.latest_meter.lock().unwrap().take()
    }

    fn close(&self) {
        self.done.cancel();
    }
}

struct SinkState {
    clients: Vec<Arc<Client>>,
    tasks: JoinSet<()>,
}

pub struct SocketSink {
    socket_path: PathBuf,
    snapshot: SnapshotFn,
    cancel: CancellationToken,
    state: Mutex<SinkState>,
    accept_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl SocketSink {
    /// Binds the OSD socket and starts accepting clients. `snapshot` is called
    /// for every new client so it receives the current state immediately.
    pub fn new(snapshot: Option<SnapshotFn>) -> Result<Arc<Self>> {
        let socket_path = default_socket_path();
        let dir = socket_path
            .parent()
            .ok_or_else(|| anyhow!("invalid OSD socket path"))?;

        ensure_socket_dir(dir)?;
        prepare_socket_path(&socket_path)?;

        let listener = UnixListener::bind(&socket_path)
            .map_err(|e| anyhow!("failed to listen on OSD socket: {e}"))?;

        if let Err(err) = std::fs::set_permissions(
            &socket_path,
            std::fs::Permissions::from_mode(SOCKET_FILE_PERM),
        ) {
            drop(listener);
            bail!("failed to set OSD socket permissions: {err}");
        }

        let snapshot = snapshot.unwrap_or_else(|| {
            Arc::new(|| new_state_event(StateValue::Idle, None, "")) as SnapshotFn
        });

        let sink = Arc::new(Self {
            socket_path: socket_path.clone(),
            snapshot,
            cancel: CancellationToken::new(),
            state: Mutex::new(SinkState {
                clients: Vec::new(),
                tasks: JoinSet::new(),
            }),
            accept_task: Mutex::new(None),
        });

        let accept_sink = Arc::clone(&sink);
        let handle = tokio::spawn(async move { accept_sink.accept_connections(listener).await });
        *sink.accept_task.lock().unwrap() = Some(handle);

        info!(path = %socket_path.display(), "OSD event socket started");
        Ok(sink)
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn publish(&self, event: Event) {
        let mut state = self.state.lock().unwrap();

        let mut dropped: Vec<usize> = Vec::new();
        for (idx, client) in state.clients.iter().enumerate() {
            match &event {
                Event::State(state_event) => {
                    if state_event.value != StateValue::Recording {
                        client.clear_meter();
                    }
                    if !client.publish_reliable(event.clone()) {
                        debug!(event = %state_event.value, "dropping slow OSD client");
                        dropped.push(idx);
                    }
                }
                Event::Meter(meter) => client.publish_meter(meter.clone()),
            }
        }

        for idx in dropped.into_iter().rev() {
            let client = state.clients.remove(idx);
            client.close();
            debug!("OSD client disconnected");
        }
    }

    pub async fn close(&self) -> Result<()> {
        self.cancel.cancel();

        let accept_task = self.accept_task.lock().unwrap().take();
        if let Some(task) = accept_task {
            let _ = task.await;
        }

        let mut tasks = {
            let mut state = self.state.lock().unwrap();
            for client in state.clients.drain(..) {
                client.close();
            }
            std::mem::take(&mut state.tasks)
        };
        while tasks.join_next().await.is_some() {}

        match std::fs::remove_file(&self.socket_path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    async fn accept_connections(self: Arc<Self>, listener: UnixListener) {
        loop {
            tokio::select! {
                _ = self.cancel.cancelled() => return,
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => self.add_client(stream),
                    Err(err) => warn!(err = %err, "failed to accept OSD client"),
                }
            }
        }
    }

    fn add_client(self: &Arc<Self>, stream: UnixStream) {
        let (tx, rx) = mpsc::channel::<Event>(RELIABLE_QUEUE_SIZE);
        let client = Arc::new(Client {
            reliable: tx,
            latest_meter: Mutex::new(None),
            wake: Arc::new(Notify::new()),
            done: CancellationToken::new(),
        });

        let mut state = self.state.lock().unwrap();

        if self.cancel.is_cancelled() {
            client.close();
            return;
        }

        if state.clients.len() >= MAX_CLIENTS {
            warn!(
                limit = MAX_CLIENTS,
                "rejecting OSD client because client limit is reached"
            );
            client.close();
            drop(stream);
            return;
        }

        let snapshot: Event = (self.snapshot)().into();
        if !client.publish_reliable(snapshot) {
            client.close();
            return;
        }

        state.clients.push(Arc::clone(&client));
        let sink = Arc::downgrade(self);
        state.tasks.spawn(async move {
            run_client(Arc::clone(&client), stream, rx).await;
            if let Some(sink) = sink.upgrade() {
                sink.remove_client(&client);
            }
        });
        debug!("OSD client connected");
    }

    fn remove_client(&self, client: &Arc<Client>) {
        let mut state = self.state.lock().unwrap();
        if let Some(idx) = state.clients.iter().position(|c| Arc::ptr_eq(c, client)) {
            state.clients.remove(idx);
            client.close();
            debug!("OSD client disconnected");
        }
    }
}

async fn run_client(
    client: Arc<Client>,
    mut stream: UnixStream,
    mut reliable: mpsc::Receiver<Event>,
) {
    loop {
        if client.done.is_cancelled() {
            break;
        }

        // reliable events first
        match reliable.try_recv() {
            Ok(event) => {
                if !write_event(&mut stream, &event).await {
                    break;
                }
                continue;
            }
            Err(mpsc::error::TryRecvError::Disconnected) => break,
            Err(mpsc::error::TryRecvError::Empty) => {}
        }

        if let Some(meter) = client.take_meter() {
            if !write_event(&mut stream, &Event::Meter(meter)).await {
                break;
            }
            continue;
        }

        tokio::select! {
            _ = client.done.cancelled() => break,
            event = reliable.recv() => match event {
                Some(event) => {
                    if !write_event(&mut stream, &event).await {
                        break;
                    }
                }
                None => break,
            },
            _ = client.wake.notified() => {}
        }
    }

    client.close();
    let _ = stream.shutdown().await;
}

async fn write_event(stream: &mut UnixStream, event: &Event) -> bool {
    let mut payload = match serde_json::to_vec(event) {
        Ok(payload) => payload,
        Err(err) => {
            warn!(err = %err, "failed to write OSD event");
            return false;
        }
    };
    payload.push(b'\n');

    match tokio::time::timeout(WRITE_TIMEOUT, stream.write_all(&payload)).await {
        Ok(Ok(())) => true,
        Ok(Err(err)) => {
            warn!(err = %err, "failed to write OSD event");
            false
        }
        Err(_) => {
            warn!("failed to write OSD event: write timed out");
            false
        }
    }
}

fn ensure_socket_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| anyhow!("failed to create OSD socket directory: {e}"))?;

    let meta = std::fs::metadata(dir)
        .map_err(|e| anyhow!("failed to inspect OSD socket directory: {e}"))?;
    if !meta.is_dir() {
        bail!(
            "OSD socket directory path is not a directory: {}",
            dir.display()
        );
    }
    if meta.uid() != uid() {
        bail!(
            "OSD socket directory is owned by uid {}, want {}: {}",
            meta.uid(),
            uid(),
            dir.display()
        );
    }
    if meta.permissions().mode() & 0o777 != SOCKET_DIR_PERM {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(SOCKET_DIR_PERM))
            .map_err(|e| anyhow!("failed to secure OSD socket directory: {e}"))?;
    }
    Ok(())
}

fn prepare_socket_path(socket_path: &Path) -> Result<()> {
    let meta = match std::fs::symlink_metadata(socket_path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => bail!("failed to inspect OSD socket path: {err}"),
    };
    if !meta.file_type().is_socket() {
        bail!(
            "OSD socket path exists and is not a socket: {}",
            socket_path.display()
        );
    }

    // probe: if something is listening, refuse to clobber it
    let probe = std::os::unix::net::UnixStream::connect(socket_path);
    if let Ok(conn) = probe {
        let _ = conn.set_read_timeout(Some(SOCKET_DIAL_TIMEOUT));
        drop(conn);
        bail!("OSD socket already in use: {}", socket_path.display());
    }

    match std::fs::remove_file(socket_path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => bail!("failed to remove stale OSD socket: {err}"),
    }
}
