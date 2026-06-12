//! Linux audio backend built on **native PipeWire** (the `pipewire` crate).
//!
//! PipeWire's client library is single-threaded: the main loop and every proxy must
//! live on, and be used from, one thread. The rest of HandoffGG calls the
//! [`AudioBackend`] trait synchronously from several threads (Tauri commands, the
//! presence monitor), so this module runs a dedicated **worker thread** that owns the
//! loop, the registry, and all proxies — mirroring the pattern the HID listeners use.
//!
//! - A live **mirror** of the graph (sinks, sources, output streams + their volumes and
//!   the current default sink/source) is kept in an `Arc<Mutex<Mirror>>`. Read-only trait
//!   methods ([`endpoints`](AudioBackend::endpoints),
//!   [`render_sessions`](AudioBackend::render_sessions)) read it directly — no round-trip.
//! - Mutating methods ([`set_default`](AudioBackend::set_default),
//!   [`set_session_volume`](AudioBackend::set_session_volume)) must touch proxies, so they
//!   send a [`Command`] over a [`pipewire::channel`] into the loop and block on a reply.
//!
//! Mapping to PipeWire concepts:
//! - Output endpoint  = node with `media.class = "Audio/Sink"`.
//! - Input endpoint   = node with `media.class = "Audio/Source"`.
//! - "Audio session"  = node with `media.class = "Stream/Output/Audio"` (an app playing audio).
//! - Default device   = the `default.audio.sink` / `default.audio.source` keys on the
//!   `default` metadata object; we *read* those, and *write*
//!   `default.configured.audio.{sink,source}` to actually change the default (the same
//!   mechanism `wpctl set-default` uses).
//! - Per-stream volume = the `channelVolumes` array in a node's `Props` param.

use crate::audio::AudioBackend;
use crate::chatmix::{app_id_for_session, route_for_app};
use crate::models::{AudioEndpoint, AudioSession, ChatMixConfig, EndpointFlow, EndpointState};
use anyhow::{anyhow, Context};
use parking_lot::Mutex;
use pipewire as pw;
use pw::proxy::ProxyT;
use pw::spa;
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Cursor;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// How a mirrored node participates in the audio graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeKind {
    Sink,
    Source,
    Stream,
}

fn classify(media_class: &str) -> Option<NodeKind> {
    match media_class {
        "Audio/Sink" => Some(NodeKind::Sink),
        "Audio/Source" => Some(NodeKind::Source),
        "Stream/Output/Audio" => Some(NodeKind::Stream),
        _ => None,
    }
}

/// One node tracked in the live mirror. Identity comes from the registry props (available
/// immediately); `channel_volumes`/`muted` are filled asynchronously from `Props` param
/// events on the bound proxy.
#[derive(Debug, Clone)]
struct MirrorNode {
    kind: NodeKind,
    /// `node.name` — the stable identifier used as the endpoint id and for defaults.
    node_name: Option<String>,
    description: Option<String>,
    app_name: Option<String>,
    app_binary: Option<String>,
    process_id: u32,
    /// `client.id` of the app that owns this stream. The registry node global is a thin
    /// subset (often just node.name/media.class/client.id); the richer identity
    /// (application.name, binary, real pid) lives on the matching [`ClientInfo`].
    client_id: Option<u32>,
    channel_volumes: Vec<f32>,
    muted: bool,
}

/// Identity of a PipeWire client (the app behind one or more streams), assembled from the
/// client's registry global props (`application.name`, `pipewire.sec.pid` — available
/// immediately) and, once bound, its `info` props (`application.process.binary`,
/// `application.process.id`).
#[derive(Debug, Clone, Default)]
struct ClientInfo {
    app_name: Option<String>,
    app_binary: Option<String>,
    /// `application.process.id` from the bound info, when present and plausible.
    info_pid: Option<u32>,
    /// `pipewire.sec.pid` — the real host pid PipeWire authenticated. More reliable than
    /// `application.process.id` for sandboxed apps (e.g. Spotify reports its namespaced pid 4).
    sec_pid: Option<u32>,
}

impl ClientInfo {
    /// Best available process id: the authenticated host pid first, then the self-reported one.
    fn process_id(&self) -> u32 {
        self.sec_pid.or(self.info_pid).unwrap_or(0)
    }
}

#[derive(Debug, Default)]
struct Mirror {
    /// Keyed by the PipeWire global id.
    nodes: HashMap<u32, MirrorNode>,
    /// Keyed by client global id; the identity source for stream nodes.
    clients: HashMap<u32, ClientInfo>,
    /// Current active defaults (from `default.audio.sink` / `default.audio.source`), by node name.
    default_sink: Option<String>,
    default_source: Option<String>,
}

/// A request that must run on the PipeWire loop thread (it touches proxies).
enum Command {
    SetDefault {
        node_name: String,
        flow: EndpointFlow,
        reply: mpsc::Sender<anyhow::Result<()>>,
    },
    SetSessionVolume {
        node_id: u32,
        volume: f32,
        reply: mpsc::Sender<anyhow::Result<()>>,
    },
}

pub struct PipewireAudioBackend {
    /// Shared with the worker thread: each reconnected session recreates the command
    /// channel (it is bound to one main loop) and republishes its sender here, so trait
    /// calls always reach the *current* session.
    cmd_tx: Arc<Mutex<pw::channel::Sender<Command>>>,
    mirror: Arc<Mutex<Mirror>>,
    /// Handed once to the Linux audio-device monitor (see `monitor.rs`).
    change_rx: Mutex<Option<mpsc::Receiver<()>>>,
    _thread: thread::JoinHandle<()>,
}

impl PipewireAudioBackend {
    pub fn new() -> anyhow::Result<Self> {
        let (cmd_tx, cmd_rx) = pw::channel::channel::<Command>();
        let cmd_tx = Arc::new(Mutex::new(cmd_tx));
        let mirror = Arc::new(Mutex::new(Mirror::default()));
        let (change_tx, change_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<anyhow::Result<()>>();

        let cmd_tx_for_thread = cmd_tx.clone();
        let mirror_for_thread = mirror.clone();
        let thread = thread::Builder::new()
            .name("handoffgg-pipewire".into())
            .spawn(move || {
                pw_thread(
                    cmd_rx,
                    cmd_tx_for_thread,
                    mirror_for_thread,
                    change_tx,
                    ready_tx,
                )
            })
            .context("failed to spawn PipeWire thread")?;

        // Block until the worker has connected and the first registry roundtrip completed,
        // so the very first endpoints()/decision sees a populated graph.
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error),
            Err(_) => anyhow::bail!("PipeWire did not become ready within 5s"),
        }

        Ok(Self {
            cmd_tx,
            mirror,
            change_rx: Mutex::new(Some(change_rx)),
            _thread: thread,
        })
    }

    fn send_command(&self, command: Command) -> anyhow::Result<()> {
        self.cmd_tx
            .lock()
            .send(command)
            .map_err(|_| anyhow!("PipeWire worker is reconnecting or gone"))
    }
}

impl AudioBackend for PipewireAudioBackend {
    fn endpoints(&self) -> anyhow::Result<Vec<AudioEndpoint>> {
        let mirror = self.mirror.lock();
        let mut endpoints = Vec::new();
        for node in mirror.nodes.values() {
            let (flow, is_default) = match node.kind {
                NodeKind::Sink => (
                    EndpointFlow::Render,
                    node.node_name.as_deref() == mirror.default_sink.as_deref(),
                ),
                NodeKind::Source => (
                    EndpointFlow::Capture,
                    node.node_name.as_deref() == mirror.default_source.as_deref(),
                ),
                NodeKind::Stream => continue,
            };
            let Some(id) = node.node_name.clone() else {
                continue;
            };
            let name = node.description.clone().unwrap_or_else(|| id.clone());

            endpoints.push(AudioEndpoint {
                is_presence_tracked: is_presence_tracked(&name),
                is_default_console: is_default,
                is_default_multimedia: is_default,
                is_default_communications: is_default,
                id,
                name,
                flow,
                // PipeWire only exposes nodes that are present/usable, so anything we can
                // see is treated as Active. Headset-off availability is driven by HID
                // presence (`is_presence_tracked`), exactly as on Windows.
                state: EndpointState::Active,
            });
        }

        endpoints.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        Ok(endpoints)
    }

    fn set_default(&self, endpoint_id: &str, flow: EndpointFlow) -> anyhow::Result<()> {
        let (reply, reply_rx) = mpsc::channel();
        self.send_command(Command::SetDefault {
            node_name: endpoint_id.to_string(),
            flow,
            reply,
        })?;
        reply_rx
            .recv()
            .map_err(|_| anyhow!("PipeWire worker dropped set_default reply"))?
    }

    fn render_sessions(&self, chatmix: &ChatMixConfig) -> anyhow::Result<Vec<AudioSession>> {
        let mirror = self.mirror.lock();
        let mut result = Vec::new();
        for (id, node) in mirror.nodes.iter() {
            if node.kind != NodeKind::Stream {
                continue;
            }

            // The node global rarely carries the app's identity; fall back to its client.
            let client = node.client_id.and_then(|cid| mirror.clients.get(&cid));

            let display_name = node
                .app_name
                .clone()
                .or_else(|| client.and_then(|c| c.app_name.clone()))
                .or_else(|| node.description.clone())
                .or_else(|| node.node_name.clone())
                .unwrap_or_else(|| format!("Stream {id}"));
            let executable_path = node
                .app_binary
                .clone()
                .or_else(|| client.and_then(|c| c.app_binary.clone()));
            let process_id = if node.process_id != 0 {
                node.process_id
            } else {
                client.map(ClientInfo::process_id).unwrap_or(0)
            };
            let app_id = app_id_for_session(executable_path.as_deref(), &display_name, process_id);
            let (route, route_source) =
                route_for_app(&app_id, &display_name, executable_path.as_deref(), chatmix);
            let volume = average_volume(&node.channel_volumes);

            result.push(AudioSession {
                id: id.to_string(),
                app_id,
                display_name,
                executable_path,
                process_id,
                route,
                route_source,
                volume,
                muted: node.muted,
            });
        }

        result.sort_by(|a, b| {
            a.display_name
                .cmp(&b.display_name)
                .then(a.app_id.cmp(&b.app_id))
                .then(a.id.cmp(&b.id))
        });
        Ok(result)
    }

    fn set_session_volume(&self, session_id: &str, volume: f32) -> anyhow::Result<()> {
        let node_id: u32 = session_id
            .parse()
            .with_context(|| format!("invalid PipeWire session id: {session_id}"))?;
        let (reply, reply_rx) = mpsc::channel();
        self.send_command(Command::SetSessionVolume {
            node_id,
            volume: volume.clamp(0.0, 1.0),
            reply,
        })?;
        reply_rx
            .recv()
            .map_err(|_| anyhow!("PipeWire worker dropped set_session_volume reply"))?
    }

    /// Hand the device-change notification channel (fired on sink/source add/remove and
    /// default changes) to the Linux audio-device monitor. Returns `None` after the first call.
    fn take_change_receiver(&self) -> Option<mpsc::Receiver<()>> {
        self.change_rx.lock().take()
    }
}

/// SteelSeries/Arctis endpoints stay present while the wireless headset is off, so their
/// real availability is driven by HID presence rather than the audio graph. Mirrors
/// `windows_audio::is_presence_tracked`.
fn is_presence_tracked(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("arctis") || lower.contains("steelseries")
}

fn average_volume(channel_volumes: &[f32]) -> f32 {
    if channel_volumes.is_empty() {
        return 1.0;
    }
    let sum: f32 = channel_volumes.iter().copied().sum();
    (sum / channel_volumes.len() as f32).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------------------
// Worker thread
// ---------------------------------------------------------------------------------------

/// Proxies that must stay alive for the lifetime of the loop. Held in an `Rc<RefCell<…>>`
/// shared between the registry callbacks (which insert) and the command handler (which
/// looks up stream nodes to set their volume, and the metadata object to set defaults).
#[derive(Default)]
struct LoopState {
    /// Bound stream nodes, keyed by global id — kept for `set_param` (volume).
    stream_nodes: HashMap<u32, pw::node::Node>,
    /// Per-stream listeners; kept alive alongside the proxy.
    stream_listeners: HashMap<u32, (pw::node::NodeListener, pw::proxy::ProxyListener)>,
    /// Bound client proxies, keyed by client global id — bound to read their `info` props
    /// (binary, pid) which the registry global omits.
    client_proxies: HashMap<u32, pw::client::Client>,
    /// Per-client info listeners; kept alive alongside the proxy.
    client_listeners: HashMap<u32, pw::client::ClientListener>,
    /// The `default` metadata object — used to set the default sink/source.
    metadata: Option<pw::metadata::Metadata>,
    _metadata_listeners: Vec<Box<dyn pw::proxy::Listener>>,
}

/// Supervises the PipeWire connection for the lifetime of the app.
///
/// The first session must succeed (so [`PipewireAudioBackend::new`] can fail fast when the
/// daemon is down). After that, if the daemon restarts or the connection drops, the loop
/// clears the now-stale mirror, notifies the monitor, and reconnects with capped backoff —
/// each session recreates the command channel (it is bound to one main loop) and
/// republishes its sender so trait calls keep reaching the live session.
fn pw_thread(
    initial_cmd_rx: pw::channel::Receiver<Command>,
    cmd_tx: Arc<Mutex<pw::channel::Sender<Command>>>,
    mirror: Arc<Mutex<Mirror>>,
    change_tx: mpsc::Sender<()>,
    ready_tx: mpsc::Sender<anyhow::Result<()>>,
) {
    pw::init();

    // First connection: on failure the error flows back through `ready_tx` to `new()`.
    if let Err(error) = run_pw_session(initial_cmd_rx, &mirror, &change_tx, Some(ready_tx)) {
        eprintln!("PipeWire initial connection failed: {error}");
        return;
    }

    // We were connected and then lost it — keep trying to re-establish for the app's life.
    let mut backoff = Duration::from_millis(200);
    loop {
        clear_mirror(&mirror);
        let _ = change_tx.send(());
        thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_secs(5));

        let (session_tx, session_rx) = pw::channel::channel::<Command>();
        *cmd_tx.lock() = session_tx;
        match run_pw_session(session_rx, &mirror, &change_tx, None) {
            // Reconnected and ran for a while before dropping again — reset the backoff.
            Ok(()) => backoff = Duration::from_millis(200),
            Err(error) => eprintln!("PipeWire reconnect attempt failed: {error}"),
        }
    }
}

/// Drop every mirrored object so a stale graph isn't reported while disconnected.
fn clear_mirror(mirror: &Arc<Mutex<Mirror>>) {
    let mut guard = mirror.lock();
    guard.nodes.clear();
    guard.clients.clear();
    guard.default_sink = None;
    guard.default_source = None;
}

/// Run one PipeWire connection until its loop exits (the daemon went away or errored).
///
/// Returns `Err` if the connection could not be established at all (daemon down), or `Ok`
/// once a successfully-connected loop has quit. `ready_tx`, when present (first session
/// only), is signalled after the initial two-roundtrip sync so `new()` can unblock.
fn run_pw_session(
    cmd_rx: pw::channel::Receiver<Command>,
    mirror: &Arc<Mutex<Mirror>>,
    change_tx: &mpsc::Sender<()>,
    ready_tx: Option<mpsc::Sender<anyhow::Result<()>>>,
) -> anyhow::Result<()> {
    // Report a setup failure both to `new()` (if it's still waiting) and to the caller.
    let fail = |ready: &Option<mpsc::Sender<anyhow::Result<()>>>, error: anyhow::Error| {
        if let Some(tx) = ready {
            let _ = tx.send(Err(anyhow!("{error}")));
        }
        Err(error)
    };

    let main_loop = match pw::main_loop::MainLoopRc::new(None) {
        Ok(loop_) => loop_,
        Err(error) => {
            return fail(
                &ready_tx,
                anyhow!("failed to create PipeWire loop: {error}"),
            )
        }
    };
    let context = match pw::context::ContextRc::new(&main_loop, None) {
        Ok(context) => context,
        Err(error) => {
            return fail(
                &ready_tx,
                anyhow!("failed to create PipeWire context: {error}"),
            )
        }
    };
    let core = match context.connect_rc(None) {
        Ok(core) => core,
        Err(error) => {
            return fail(
                &ready_tx,
                anyhow!("failed to connect to PipeWire (is the daemon running?): {error}"),
            )
        }
    };
    let registry = match core.get_registry_rc() {
        Ok(registry) => registry,
        Err(error) => {
            return fail(
                &ready_tx,
                anyhow!("failed to get PipeWire registry: {error}"),
            )
        }
    };

    let loop_state = Rc::new(RefCell::new(LoopState::default()));

    // Signal readiness after *two* roundtrips: the first delivers every existing global
    // (sinks, sources, streams, the `default` metadata object); binding the metadata proxy
    // in that pass triggers its property replay, which only lands on the second roundtrip.
    // Waiting for both means the first `endpoints()` already knows the current defaults.
    enum ReadyPhase {
        AwaitingFirst(spa::utils::result::AsyncSeq),
        AwaitingSecond(spa::utils::result::AsyncSeq),
        Done,
    }

    let pending = match core.sync(0) {
        Ok(seq) => seq,
        Err(error) => return fail(&ready_tx, anyhow!("PipeWire sync failed: {error}")),
    };
    let phase = RefCell::new(ReadyPhase::AwaitingFirst(pending));
    let ready_slot = RefCell::new(ready_tx);
    let core_for_done = core.clone();
    let main_loop_weak = main_loop.downgrade();
    let _core_listener = core
        .add_listener_local()
        .done(move |id, seq| {
            if id != pw::core::PW_ID_CORE {
                return;
            }
            let next = match *phase.borrow() {
                // First roundtrip complete: issue the second. If that fails, signal
                // ready now rather than never.
                ReadyPhase::AwaitingFirst(expected) if seq == expected => {
                    match core_for_done.sync(0) {
                        Ok(second) => ReadyPhase::AwaitingSecond(second),
                        Err(_) => ReadyPhase::Done,
                    }
                }
                ReadyPhase::AwaitingSecond(expected) if seq == expected => ReadyPhase::Done,
                _ => return,
            };
            let became_done = matches!(next, ReadyPhase::Done);
            *phase.borrow_mut() = next;
            if became_done {
                if let Some(tx) = ready_slot.borrow_mut().take() {
                    let _ = tx.send(Ok(()));
                }
            }
        })
        .error(move |id, _seq, res, message| {
            eprintln!("PipeWire core error id:{id} res:{res}: {message}");
            if id == 0 {
                if let Some(main_loop) = main_loop_weak.upgrade() {
                    main_loop.quit();
                }
            }
        })
        .register();

    let registry_weak = registry.downgrade();
    let _registry_listener = {
        let mirror_global = mirror.clone();
        let change_global = change_tx.clone();
        let state_global = loop_state.clone();
        let mirror_remove = mirror.clone();
        let change_remove = change_tx.clone();
        let state_remove = loop_state.clone();
        registry
            .add_listener_local()
            .global(move |global| {
                if let Some(registry) = registry_weak.upgrade() {
                    handle_global(
                        &registry,
                        global,
                        &mirror_global,
                        &change_global,
                        &state_global,
                    );
                }
            })
            .global_remove(move |id| {
                let removed_kind = {
                    let mut guard = mirror_remove.lock();
                    guard.clients.remove(&id);
                    guard.nodes.remove(&id).map(|node| node.kind)
                };
                {
                    let mut state = state_remove.borrow_mut();
                    state.stream_nodes.remove(&id);
                    state.stream_listeners.remove(&id);
                    state.client_proxies.remove(&id);
                    state.client_listeners.remove(&id);
                }
                if matches!(removed_kind, Some(NodeKind::Sink) | Some(NodeKind::Source)) {
                    let _ = change_remove.send(());
                }
            })
            .register()
    };

    // Process commands sent from the trait methods on other threads.
    let _attached_rx = cmd_rx.attach(main_loop.loop_(), {
        let loop_state = loop_state.clone();
        let mirror = mirror.clone();
        move |command| handle_command(command, &loop_state, &mirror)
    });

    // Blocks until the loop quits — either a core error (handled above) or shutdown.
    main_loop.run();
    Ok(())
}

fn handle_global(
    registry: &pw::registry::RegistryRc,
    global: &pw::registry::GlobalObject<&spa::utils::dict::DictRef>,
    mirror: &Arc<Mutex<Mirror>>,
    change_tx: &mpsc::Sender<()>,
    loop_state: &Rc<RefCell<LoopState>>,
) {
    use pw::types::ObjectType;

    match global.type_ {
        ObjectType::Node => {
            let Some(props) = global.props else { return };
            let Some(kind) = props.get("media.class").and_then(classify) else {
                return;
            };

            let node = MirrorNode {
                kind,
                node_name: props.get("node.name").map(str::to_string),
                description: props
                    .get("node.description")
                    .or_else(|| props.get("node.nick"))
                    .or_else(|| props.get("application.name"))
                    .or_else(|| props.get("media.name"))
                    .map(str::to_string),
                app_name: props
                    .get("application.name")
                    .or_else(|| props.get("media.name"))
                    .map(str::to_string),
                app_binary: props.get("application.process.binary").map(str::to_string),
                process_id: props
                    .get("application.process.id")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0),
                client_id: props.get("client.id").and_then(|value| value.parse().ok()),
                channel_volumes: Vec::new(),
                muted: false,
            };
            mirror.lock().nodes.insert(global.id, node);

            // Bind stream nodes to track their live volume/mute via Props params. Sinks and
            // sources need no proxy — their identity comes from the registry props and their
            // default status from the metadata object.
            if kind == NodeKind::Stream {
                if let Ok(proxy) = registry.bind::<pw::node::Node, _>(global) {
                    proxy.subscribe_params(&[spa::param::ParamType::Props]);
                    let id = global.id;
                    let param_listener = proxy
                        .add_listener_local()
                        .param({
                            let mirror = mirror.clone();
                            move |_seq, param_type, _index, _next, pod| {
                                if param_type != spa::param::ParamType::Props {
                                    return;
                                }
                                let Some(pod) = pod else { return };
                                let (volumes, mute) = parse_props(pod);
                                let mut guard = mirror.lock();
                                if let Some(node) = guard.nodes.get_mut(&id) {
                                    if let Some(volumes) = volumes {
                                        node.channel_volumes = volumes;
                                    }
                                    if let Some(mute) = mute {
                                        node.muted = mute;
                                    }
                                }
                            }
                        })
                        .register();
                    let proxy_listener = proxy
                        .upcast_ref()
                        .add_listener_local()
                        .removed({
                            let loop_state = loop_state.clone();
                            move || {
                                let mut state = loop_state.borrow_mut();
                                state.stream_nodes.remove(&id);
                                state.stream_listeners.remove(&id);
                            }
                        })
                        .register();
                    let mut state = loop_state.borrow_mut();
                    state.stream_nodes.insert(id, proxy);
                    state
                        .stream_listeners
                        .insert(id, (param_listener, proxy_listener));
                }
            } else {
                // A new sink/source may change which devices are available — re-evaluate.
                let _ = change_tx.send(());
            }
        }
        ObjectType::Metadata => {
            let is_default =
                global.props.and_then(|props| props.get("metadata.name")) == Some("default");
            if !is_default {
                return;
            }
            if let Ok(metadata) = registry.bind::<pw::metadata::Metadata, _>(global) {
                let listener = metadata
                    .add_listener_local()
                    .property({
                        let mirror = mirror.clone();
                        let change_tx = change_tx.clone();
                        move |_subject, key, _type, value| {
                            if let Some(key) = key {
                                let name = value.and_then(parse_default_name);
                                let mut guard = mirror.lock();
                                match key {
                                    "default.audio.sink" => guard.default_sink = name,
                                    "default.audio.source" => guard.default_source = name,
                                    _ => return 0,
                                }
                                drop(guard);
                                let _ = change_tx.send(());
                            }
                            0
                        }
                    })
                    .register();
                let mut state = loop_state.borrow_mut();
                state.metadata = Some(metadata);
                state._metadata_listeners.push(Box::new(listener));
            }
        }
        ObjectType::Client => {
            // Seed identity from the registry global (application.name + the authenticated
            // host pid are present immediately), then bind to fill in the binary/info pid.
            let id = global.id;
            let mut info = ClientInfo::default();
            if let Some(props) = global.props {
                info.app_name = props.get("application.name").map(str::to_string);
                info.sec_pid = props
                    .get("pipewire.sec.pid")
                    .and_then(|value| value.parse().ok());
            }
            mirror.lock().clients.insert(id, info);

            if let Ok(proxy) = registry.bind::<pw::client::Client, _>(global) {
                let listener = proxy
                    .add_listener_local()
                    .info({
                        let mirror = mirror.clone();
                        move |info| {
                            let Some(props) = info.props() else { return };
                            let mut guard = mirror.lock();
                            let entry = guard.clients.entry(id).or_default();
                            if let Some(name) = props.get("application.name") {
                                entry.app_name = Some(name.to_string());
                            }
                            if let Some(binary) = props.get("application.process.binary") {
                                entry.app_binary = Some(binary.to_string());
                            }
                            if let Some(pid) = props
                                .get("application.process.id")
                                .and_then(|value| value.parse::<u32>().ok())
                                .filter(|pid| *pid != 0)
                            {
                                entry.info_pid = Some(pid);
                            }
                        }
                    })
                    .register();
                let mut state = loop_state.borrow_mut();
                state.client_proxies.insert(id, proxy);
                state.client_listeners.insert(id, listener);
            }
        }
        _ => {}
    }
}

fn handle_command(
    command: Command,
    loop_state: &Rc<RefCell<LoopState>>,
    mirror: &Arc<Mutex<Mirror>>,
) {
    match command {
        Command::SetDefault {
            node_name,
            flow,
            reply,
        } => {
            // Write the *configured* default (the user's choice); WirePlumber then moves the
            // active default. This is the same key `wpctl set-default` writes.
            let key = match flow {
                EndpointFlow::Render => "default.configured.audio.sink",
                EndpointFlow::Capture => "default.configured.audio.source",
            };
            let value = format!("{{\"name\":\"{}\"}}", escape_json(&node_name));
            let state = loop_state.borrow();
            let result = match state.metadata.as_ref() {
                Some(metadata) => {
                    metadata.set_property(0, key, Some("Spa:String:JSON"), Some(&value));
                    Ok(())
                }
                None => Err(anyhow!("PipeWire default-metadata object not available")),
            };
            let _ = reply.send(result);
        }
        Command::SetSessionVolume {
            node_id,
            volume,
            reply,
        } => {
            let channels = mirror
                .lock()
                .nodes
                .get(&node_id)
                .map(|node| node.channel_volumes.len())
                .filter(|len| *len > 0)
                .unwrap_or(2);
            let state = loop_state.borrow();
            let result = match state.stream_nodes.get(&node_id) {
                Some(node) => set_node_volume(node, volume, channels),
                None => Err(anyhow!("PipeWire stream {node_id} is no longer present")),
            };
            let _ = reply.send(result);
        }
    }
}

fn set_node_volume(node: &pw::node::Node, volume: f32, channels: usize) -> anyhow::Result<()> {
    let bytes = build_volume_pod(volume, channels)?;
    let pod = spa::pod::Pod::from_bytes(&bytes)
        .ok_or_else(|| anyhow!("failed to build channelVolumes pod"))?;
    node.set_param(spa::param::ParamType::Props, 0, pod);
    Ok(())
}

/// Build a `SPA_TYPE_OBJECT_Props` pod carrying `channelVolumes = [volume; channels]`.
fn build_volume_pod(volume: f32, channels: usize) -> anyhow::Result<Vec<u8>> {
    use spa::pod::{serialize::PodSerializer, Object, Property, PropertyFlags, Value, ValueArray};

    let volumes = vec![volume; channels.max(1)];
    let object = Value::Object(Object {
        type_: spa::utils::SpaTypes::ObjectParamProps.as_raw(),
        id: spa::param::ParamType::Props.as_raw(),
        properties: vec![Property {
            key: spa::sys::SPA_PROP_channelVolumes,
            flags: PropertyFlags::empty(),
            value: Value::ValueArray(ValueArray::Float(volumes)),
        }],
    });

    let (cursor, _len) = PodSerializer::serialize(Cursor::new(Vec::new()), &object)
        .map_err(|error| anyhow!("failed to serialize channelVolumes pod: {error:?}"))?;
    Ok(cursor.into_inner())
}

/// Extract `channelVolumes` and `mute` from a node `Props` param pod.
fn parse_props(pod: &spa::pod::Pod) -> (Option<Vec<f32>>, Option<bool>) {
    use spa::pod::{deserialize::PodDeserializer, Value, ValueArray};

    let Ok((_, Value::Object(object))) = PodDeserializer::deserialize_any_from(pod.as_bytes())
    else {
        return (None, None);
    };

    let mut volumes = None;
    let mut mute = None;
    for property in object.properties {
        if property.key == spa::sys::SPA_PROP_channelVolumes {
            if let Value::ValueArray(ValueArray::Float(values)) = property.value {
                volumes = Some(values);
            }
        } else if property.key == spa::sys::SPA_PROP_mute {
            if let Value::Bool(value) = property.value {
                mute = Some(value);
            }
        }
    }
    (volumes, mute)
}

/// The default sink/source metadata value is JSON like `{"name":"alsa_output.…"}`.
fn parse_default_name(value: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(value)
        .ok()?
        .get("name")?
        .as_str()
        .map(str::to_string)
}

/// Minimal JSON string escaping for the node name we inject into the metadata value.
fn escape_json(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use spa::pod::{serialize::PodSerializer, Object, Property, PropertyFlags, Value, ValueArray};

    /// Serialize a Props object carrying both `channelVolumes` and `mute`, mirroring what a
    /// node emits, so we can exercise `parse_props` end-to-end without a live daemon.
    fn build_props_pod(volumes: &[f32], mute: bool) -> Vec<u8> {
        let object = Value::Object(Object {
            type_: spa::utils::SpaTypes::ObjectParamProps.as_raw(),
            id: spa::param::ParamType::Props.as_raw(),
            properties: vec![
                Property {
                    key: spa::sys::SPA_PROP_channelVolumes,
                    flags: PropertyFlags::empty(),
                    value: Value::ValueArray(ValueArray::Float(volumes.to_vec())),
                },
                Property {
                    key: spa::sys::SPA_PROP_mute,
                    flags: PropertyFlags::empty(),
                    value: Value::Bool(mute),
                },
            ],
        });
        PodSerializer::serialize(Cursor::new(Vec::new()), &object)
            .expect("serialize props pod")
            .0
            .into_inner()
    }

    #[test]
    fn build_volume_pod_round_trips_channel_volumes() {
        let bytes = build_volume_pod(0.42, 2).expect("build volume pod");
        let pod = spa::pod::Pod::from_bytes(&bytes).expect("pod from bytes");
        let (volumes, mute) = parse_props(pod);

        assert_eq!(volumes, Some(vec![0.42_f32, 0.42_f32]));
        // build_volume_pod only writes channelVolumes, so mute must be absent (not `false`).
        assert_eq!(mute, None);
    }

    #[test]
    fn build_volume_pod_fills_every_channel() {
        let bytes = build_volume_pod(0.5, 6).expect("build volume pod");
        let pod = spa::pod::Pod::from_bytes(&bytes).expect("pod from bytes");
        let (volumes, _) = parse_props(pod);
        assert_eq!(volumes, Some(vec![0.5_f32; 6]));
    }

    #[test]
    fn build_volume_pod_never_emits_zero_channels() {
        // A node we haven't seen volumes for yet defaults to 0 channels upstream; the pod
        // must still be valid (at least one channel) rather than an empty array.
        let bytes = build_volume_pod(0.3, 0).expect("build volume pod");
        let pod = spa::pod::Pod::from_bytes(&bytes).expect("pod from bytes");
        let (volumes, _) = parse_props(pod);
        assert_eq!(volumes, Some(vec![0.3_f32]));
    }

    #[test]
    fn parse_props_reads_volumes_and_mute() {
        let bytes = build_props_pod(&[0.25, 0.75], true);
        let pod = spa::pod::Pod::from_bytes(&bytes).expect("pod from bytes");
        let (volumes, mute) = parse_props(pod);

        assert_eq!(volumes, Some(vec![0.25_f32, 0.75_f32]));
        assert_eq!(mute, Some(true));
    }

    #[test]
    fn average_volume_handles_empty_and_mixed() {
        assert_eq!(average_volume(&[]), 1.0);
        assert_eq!(average_volume(&[0.2, 0.4]), 0.3);
    }
}
