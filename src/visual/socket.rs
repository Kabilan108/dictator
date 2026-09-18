use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::event::*;
use crate::ipc::protocol::default_socket_directory;
use crate::ipc::unix_socket::{SocketProbe, ensure_private_socket_parent, probe_socket};

pub const RELIABLE_QUEUE_SIZE: usize = 16;
pub const MAX_CLIENTS: usize = 4;
const WRITE_TIMEOUT: Duration = Duration::from_millis(500);
const SOCKET_FILE_PERM: u32 = 0o600;

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
    default_socket_directory().join("osd.sock")
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
}

#[derive(Clone, Copy)]
struct SocketIdentity {
    device: u64,
    inode: u64,
}

pub struct SocketSink {
    socket_path: PathBuf,
    socket_identity: SocketIdentity,
    snapshot: SnapshotFn,
    cancel: CancellationToken,
    state: Mutex<SinkState>,
    accept_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl SocketSink {
    /// Binds the OSD socket and starts accepting clients. `snapshot` is called
    /// for every new client so it receives the current state immediately.
    pub fn new(snapshot: Option<SnapshotFn>) -> Result<Arc<Self>> {
        Self::with_path(snapshot, default_socket_path())
    }

    /// Like [`SocketSink::new`] but listens on a custom socket path.
    pub fn with_path(snapshot: Option<SnapshotFn>, socket_path: PathBuf) -> Result<Arc<Self>> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| anyhow!("OSD socket requires a Tokio runtime"))?;
        ensure_private_socket_parent(&socket_path)?;
        prepare_socket_path(&socket_path)?;

        let listener = UnixListener::bind(&socket_path)
            .map_err(|e| anyhow!("failed to listen on OSD socket: {e}"))?;
        let socket_identity = match socket_identity(&socket_path) {
            Ok(identity) => identity,
            Err(err) => {
                drop(listener);
                return Err(err);
            }
        };

        if let Err(err) = std::fs::set_permissions(
            &socket_path,
            std::fs::Permissions::from_mode(SOCKET_FILE_PERM),
        ) {
            drop(listener);
            let _ = remove_socket_if_owned(&socket_path, socket_identity);
            bail!("failed to set OSD socket permissions: {err}");
        }

        let snapshot = snapshot.unwrap_or_else(|| {
            Arc::new(|| new_state_event(StateValue::Idle, None, "")) as SnapshotFn
        });
        let sink = Arc::new(Self {
            socket_path: socket_path.clone(),
            socket_identity,
            snapshot,
            cancel: CancellationToken::new(),
            state: Mutex::new(SinkState {
                clients: Vec::new(),
            }),
            accept_task: Mutex::new(None),
        });

        let handle = runtime.spawn(accept_connections(
            Arc::downgrade(&sink),
            listener,
            sink.cancel.clone(),
        ));
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

        {
            let mut state = self.state.lock().unwrap();
            for client in state.clients.drain(..) {
                client.close();
            }
        }

        let accept_task = self.accept_task.lock().unwrap().take();
        let task_result = match accept_task {
            Some(task) => task.await,
            None => Ok(()),
        };
        let socket_result = remove_socket_if_owned(&self.socket_path, self.socket_identity);

        if let Err(err) = task_result {
            return Err(anyhow!("OSD server task failed during shutdown: {err}"));
        }
        socket_result
    }

    fn add_client(&self) -> Option<(Arc<Client>, mpsc::Receiver<Event>)> {
        let (tx, rx) = mpsc::channel::<Event>(RELIABLE_QUEUE_SIZE);
        let client = Arc::new(Client {
            reliable: tx,
            latest_meter: Mutex::new(None),
            wake: Arc::new(Notify::new()),
            done: CancellationToken::new(),
        });

        if self.cancel.is_cancelled() {
            client.close();
            return None;
        }

        let snapshot: Event = (self.snapshot)().into();
        let mut state = self.state.lock().unwrap();

        if self.cancel.is_cancelled() {
            client.close();
            return None;
        }

        if state.clients.len() >= MAX_CLIENTS {
            warn!(
                limit = MAX_CLIENTS,
                "rejecting OSD client because client limit is reached"
            );
            client.close();
            return None;
        }

        if !client.publish_reliable(snapshot) {
            client.close();
            return None;
        }

        state.clients.push(Arc::clone(&client));
        debug!("OSD client connected");
        Some((client, rx))
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

impl Drop for SocketSink {
    fn drop(&mut self) {
        self.cancel.cancel();
        for client in self.state.lock().unwrap().clients.drain(..) {
            client.close();
        }
        if let Err(err) = remove_socket_if_owned(&self.socket_path, self.socket_identity) {
            warn!(err = %err, "failed to remove OSD socket while dropping sink");
        }
    }
}

async fn accept_connections(
    sink: std::sync::Weak<SocketSink>,
    listener: UnixListener,
    cancel: CancellationToken,
) {
    let mut clients = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            result = clients.join_next(), if !clients.is_empty() => {
                if let Some(Err(err)) = result {
                    warn!(err = %err, "OSD client task panicked");
                }
            }
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let Some(sink_ref) = sink.upgrade() else { break };
                    let Some((client, rx)) = sink_ref.add_client() else {
                        drop(stream);
                        continue;
                    };
                    let task_sink = sink.clone();
                    clients.spawn(async move {
                        run_client(Arc::clone(&client), stream, rx).await;
                        if let Some(sink) = task_sink.upgrade() {
                            sink.remove_client(&client);
                        }
                    });
                }
                Err(err) => {
                    warn!(err = %err, "failed to accept OSD client");
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            }
        }
    }

    while let Some(result) = clients.join_next().await {
        if let Err(err) = result {
            warn!(err = %err, "OSD client task panicked during shutdown");
        }
    }
}

async fn run_client(client: Arc<Client>, stream: UnixStream, mut reliable: mpsc::Receiver<Event>) {
    let (mut reader, mut writer) = stream.into_split();
    let mut peer_input = [0_u8; 1];
    loop {
        if client.done.is_cancelled() {
            break;
        }

        // reliable events first
        match reliable.try_recv() {
            Ok(event) => {
                if !write_event(&mut writer, &event).await {
                    break;
                }
                continue;
            }
            Err(mpsc::error::TryRecvError::Disconnected) => break,
            Err(mpsc::error::TryRecvError::Empty) => {}
        }

        if let Some(meter) = client.take_meter() {
            if !write_event(&mut writer, &Event::Meter(meter)).await {
                break;
            }
            continue;
        }

        tokio::select! {
            _ = client.done.cancelled() => break,
            event = reliable.recv() => match event {
                Some(event) => {
                    if !write_event(&mut writer, &event).await {
                        break;
                    }
                }
                None => break,
            },
            _ = client.wake.notified() => {}
            result = reader.read(&mut peer_input) => {
                match result {
                    Ok(0) => break,
                    Ok(_) => {
                        warn!("disconnecting OSD client that sent unexpected data");
                        break;
                    }
                    Err(err) => {
                        debug!(err = %err, "OSD client read side closed");
                        break;
                    }
                }
            }
        }
    }

    client.close();
    let _ = writer.shutdown().await;
}

async fn write_event<W: AsyncWrite + Unpin>(stream: &mut W, event: &Event) -> bool {
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
    let identity = SocketIdentity {
        device: meta.dev(),
        inode: meta.ino(),
    };

    match probe_socket(socket_path)? {
        SocketProbe::Active => {
            bail!("OSD socket already in use: {}", socket_path.display())
        }
        SocketProbe::Stale => {}
    }

    remove_socket_if_owned(socket_path, identity)
        .map_err(|err| anyhow!("failed to remove stale OSD socket: {err}"))
}

fn socket_identity(socket_path: &Path) -> Result<SocketIdentity> {
    let metadata = std::fs::symlink_metadata(socket_path)?;
    Ok(SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn remove_socket_if_owned(socket_path: &Path, expected: SocketIdentity) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(socket_path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    if !metadata.file_type().is_socket()
        || metadata.dev() != expected.device
        || metadata.ino() != expected.inode
    {
        bail!("refusing to remove an OSD socket path no longer owned by this sink");
    }
    std::fs::remove_file(socket_path)?;
    Ok(())
}
