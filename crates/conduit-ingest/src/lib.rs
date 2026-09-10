//! Phase 20: Conduit's gRPC ingestion service.
//!
//! Part 3 of the three-part model (`architecture.md` §1.2): a
//! technology-agnostic producer contract. A producer in any language streams
//! [`EventEnvelope`](pb::EventEnvelope)s and gets back position
//! [`Ack`](pb::Ack)s once the batch's guards commit. Conduit persists nothing
//! on the ingestion side — the producer owns replay from its last ack.
//!
//! `tonic` lives here, never in `conduit-core`: [`GrpcSource`] implements the
//! `conduit_core::source::EventSource` trait, so `conduit_core::run_sources`
//! drives it exactly like a `directory` source.

pub mod pb {
    tonic::include_proto!("conduit.v1");
}

use std::net::SocketAddr;

use conduit_core::event::Event;
use conduit_core::source::{EventSource, SourceError, SourcePosition, SourcedEvent};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::{BroadcastStream, TcpListenerStream};
use tonic::{Request, Response, Status, Streaming, transport::Server};

/// Bound of the receive channel. Not event storage — a socket buffer. When
/// full, the gRPC handler's `send` awaits, HTTP/2 flow control pauses the
/// producer's stream: backpressure, not buffering (Phase 20.2).
const CHANNEL_BOUND: usize = 1024;

#[derive(Debug)]
pub enum IngestError {
    Bind(String),
    Runtime(std::io::Error),
    Config(String),
}

impl std::fmt::Display for IngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IngestError::Bind(s) => write!(f, "gRPC bind failed: {s}"),
            IngestError::Runtime(e) => write!(f, "tokio runtime: {e}"),
            IngestError::Config(s) => write!(f, "ingest config: {s}"),
        }
    }
}
impl std::error::Error for IngestError {}

// ---------------------------------------------------------------------------
// EventEnvelope <-> Event
// ---------------------------------------------------------------------------

impl pb::EventEnvelope {
    /// Decode into a Conduit [`Event`] + its opaque [`SourcePosition`].
    /// `version 0` → 1 (Phase 10 default); an empty `event_id` is derived (it
    /// is not load-bearing for gating — `sequence` is).
    pub fn into_sourced(self) -> SourcedEvent {
        let event_id = if !self.event_id.is_empty() {
            self.event_id
        } else if !self.position.is_empty() {
            self.position.clone()
        } else {
            format!("{}-{}", self.event_type, self.sequence)
        };
        SourcedEvent {
            event: Event {
                event_id,
                event_type: self.event_type,
                payload: self.payload,
                metadata: self.metadata.into_iter().collect(),
                version: if self.version == 0 { 1 } else { self.version },
                sequence: self.sequence,
            },
            position: SourcePosition(self.position),
        }
    }
}

impl From<&Event> for pb::EventEnvelope {
    fn from(e: &Event) -> Self {
        pb::EventEnvelope {
            event_type: e.event_type.clone(),
            payload: e.payload.clone(),
            metadata: e.metadata.clone().into_iter().collect(),
            version: e.version,
            sequence: e.sequence,
            position: String::new(),
            event_id: e.event_id.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// The gRPC service
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AckMsg {
    position: String,
    batch_events: u32,
}

struct IngestService {
    /// Decoded events → the bounded receive channel drained by `GrpcSource::poll`.
    ev_tx: mpsc::Sender<SourcedEvent>,
    /// Committed positions fanned out to every active session's response stream.
    ack_tx: broadcast::Sender<AckMsg>,
}

type AckStream =
    std::pin::Pin<Box<dyn futures_core::Stream<Item = Result<pb::Ack, Status>> + Send>>;

#[tonic::async_trait]
impl pb::ingest_server::Ingest for IngestService {
    type StreamStream = AckStream;

    // `Result<_, tonic::Status>` is the trait's return shape — `Status` is large
    // and there is nothing to box.
    #[allow(clippy::result_large_err)]
    async fn stream(
        &self,
        request: Request<Streaming<pb::EventEnvelope>>,
    ) -> Result<Response<Self::StreamStream>, Status> {
        let mut inbound = request.into_inner();
        let ev_tx = self.ev_tx.clone();
        let ack_rx = self.ack_tx.subscribe();

        // Reader task: inbound envelopes → bounded channel. `send().await` is
        // where backpressure lands when the engine is behind.
        tokio::spawn(async move {
            while let Ok(Some(env)) = inbound.message().await {
                if ev_tx.send(env.into_sourced()).await.is_err() {
                    break; // engine / source dropped
                }
            }
        });

        // Response stream: every committed position becomes an `Ack`.
        let out = BroadcastStream::new(ack_rx).filter_map(|r| {
            r.ok().map(|a: AckMsg| {
                Ok(pb::Ack {
                    position: a.position,
                    batch_events: a.batch_events,
                })
            })
        });
        Ok(Response::new(Box::pin(out)))
    }
}

// ---------------------------------------------------------------------------
// GrpcSource + server lifecycle
// ---------------------------------------------------------------------------

/// The `EventSource` handed to `conduit_core::run_sources`. Holds no persistent
/// state — `committed_position` is in memory only (Phase 20.2: the `Ack` is the
/// durable signal, the producer owns the offset).
pub struct GrpcSource {
    id: String,
    ev_rx: mpsc::Receiver<SourcedEvent>,
    ack_tx: broadcast::Sender<AckMsg>,
    committed: Option<SourcePosition>,
    /// Events returned by `poll` since the last `commit` — the `Ack.batch_events`.
    since_commit: u32,
}

impl EventSource for GrpcSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn poll(&mut self, max_batch: usize) -> Result<Vec<SourcedEvent>, SourceError> {
        let mut out = Vec::new();
        while out.len() < max_batch {
            match self.ev_rx.try_recv() {
                Ok(se) => out.push(se),
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => break,
            }
        }
        self.since_commit += out.len() as u32;
        Ok(out)
    }

    fn commit(&mut self, position: SourcePosition) -> Result<(), SourceError> {
        // Fan the position out to every session; a session forwards it as an
        // `Ack` and the producer advances its offset.
        let _ = self.ack_tx.send(AckMsg {
            position: position.0.clone(),
            batch_events: std::mem::take(&mut self.since_commit),
        });
        self.committed = Some(position);
        Ok(())
    }

    fn committed_position(&self) -> Option<&SourcePosition> {
        self.committed.as_ref()
    }
}

/// A running gRPC server. Keep it alive for as long as the source is polled;
/// [`GrpcServer::shutdown`] stops it gracefully.
pub struct GrpcServer {
    runtime: tokio::runtime::Runtime,
    shutdown: watch::Sender<bool>,
    /// The actually-bound address (useful when a `:0` ephemeral port was asked for).
    pub local_addr: Option<SocketAddr>,
}

impl GrpcServer {
    /// Stop accepting new stream items, let in-flight RPCs finish, then exit.
    pub fn shutdown(self) {
        let _ = self.shutdown.send(true);
        self.runtime
            .shutdown_timeout(std::time::Duration::from_secs(5));
    }
}

/// Bind a gRPC ingestion server and return it together with the [`GrpcSource`]
/// to feed `conduit_core::run_sources`.
///
/// `listen` accepts `"tcp://host:port"`, a bare `"host:port"`, or (unix only)
/// `"unix:/path/to.sock"`.
pub fn start(
    source_id: impl Into<String>,
    listen: &str,
) -> Result<(GrpcServer, GrpcSource), IngestError> {
    let addr = parse_tcp(listen)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(IngestError::Runtime)?;

    let (ev_tx, ev_rx) = mpsc::channel::<SourcedEvent>(CHANNEL_BOUND);
    let (ack_tx, _ack_rx0) = broadcast::channel::<AckMsg>(256);
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    // Bind inside the runtime task and report the actual address back — keeps
    // `start` free of `block_on`, so it is callable from any sync context.
    let (addr_tx, addr_rx) = tokio::sync::oneshot::channel::<Result<SocketAddr, String>>();

    let svc = IngestService {
        ev_tx,
        ack_tx: ack_tx.clone(),
    };

    runtime.spawn(async move {
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                let _ = addr_tx.send(Err(e.to_string()));
                return;
            }
        };
        let local = listener.local_addr();
        let _ = addr_tx.send(local.map_err(|e| e.to_string()));
        let _ = Server::builder()
            .add_service(pb::ingest_server::IngestServer::new(svc))
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async move {
                let _ = shutdown_rx.wait_for(|v| *v).await;
            })
            .await;
    });

    let local_addr = addr_rx
        .blocking_recv()
        .map_err(|_| IngestError::Bind("server task exited before binding".into()))?
        .map_err(IngestError::Bind)?;

    let source = GrpcSource {
        id: source_id.into(),
        ev_rx,
        ack_tx,
        committed: None,
        since_commit: 0,
    };
    Ok((
        GrpcServer {
            runtime,
            shutdown: shutdown_tx,
            local_addr: Some(local_addr),
        },
        source,
    ))
}

// ---------------------------------------------------------------------------
// `conduit ingest` orchestration (kept out of the thin CLI)
// ---------------------------------------------------------------------------

/// Options for [`serve`].
pub struct ServeOptions {
    pub listen: String,
    pub max_batch: usize,
    pub retry_budget: u32,
    pub dlq_dir: Option<std::path::PathBuf>,
    /// Poll cadence when caught up — the max ingest→dispatch latency.
    pub poll_interval: std::time::Duration,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            listen: "tcp://127.0.0.1:50051".into(),
            max_batch: 256,
            retry_budget: 3,
            dlq_dir: None,
            poll_interval: std::time::Duration::from_millis(200),
        }
    }
}

#[derive(Debug)]
pub enum ServeError {
    Pipeline(conduit_core::pipeline::PipelineError),
    Ingest(IngestError),
    Source(SourceError),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServeError::Pipeline(e) => write!(f, "{e}"),
            ServeError::Ingest(e) => write!(f, "{e}"),
            ServeError::Source(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for ServeError {}

/// Load the project, start the gRPC server, and run the
/// `poll → dispatch → commit → ack` loop until `stop` is set (SIGINT/SIGTERM).
/// On shutdown the in-flight batch is committed and acked before the server
/// stops (Phase 20.4).
pub fn serve(
    config_path: &std::path::Path,
    mappings_dir: &std::path::Path,
    opts: &ServeOptions,
    stop: &std::sync::atomic::AtomicBool,
) -> Result<conduit_core::SourceRunReport, ServeError> {
    let project = conduit_core::pipeline::load_project(config_path, mappings_dir)
        .map_err(ServeError::Pipeline)?;

    let (server, source) = start("grpc", &opts.listen).map_err(ServeError::Ingest)?;
    if let Some(a) = server.local_addr {
        eprintln!("conduit ingest: listening on {a}");
    }

    let run_opts = conduit_core::SourceRunOptions {
        mode: conduit_core::RunMode::Continuous {
            poll_interval: opts.poll_interval,
        },
        max_batch: opts.max_batch,
        retry_budget: opts.retry_budget,
        dlq_dir: opts.dlq_dir.clone(),
    };

    let result = conduit_core::run_sources(
        &project.config,
        project.routing_rules,
        project.sql_mappings,
        project.doc_mappings,
        project.kv_mappings,
        project.graph_mappings,
        vec![Box::new(source)],
        &run_opts,
        stop,
    );

    server.shutdown();
    result.map_err(ServeError::Source)
}

/// `tcp://h:p`, `h:p`, or (unix) `unix:/path` — the last returns an error here
/// pending `#[cfg(unix)]` UDS support; TCP is the portable default.
fn parse_tcp(listen: &str) -> Result<SocketAddr, IngestError> {
    let s = listen.strip_prefix("tcp://").unwrap_or(listen);
    if s.starts_with("unix:") {
        return Err(IngestError::Config(
            "unix-domain-socket listener is not built on this platform; use tcp://host:port".into(),
        ));
    }
    s.parse()
        .map_err(|e| IngestError::Config(format!("invalid listen address {listen:?}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn envelope_round_trips_through_event() {
        let env = pb::EventEnvelope {
            event_type: "UserCreated".into(),
            payload: r#"{"id":"u1"}"#.into(),
            metadata: HashMap::from([("trace".to_string(), "abc".to_string())]),
            version: 0,
            sequence: 7,
            position: "offset-42".into(),
            event_id: String::new(),
        };
        let se = env.into_sourced();
        assert_eq!(se.event.event_type, "UserCreated");
        assert_eq!(se.event.version, 1, "version 0 defaults to 1");
        assert_eq!(se.event.sequence, 7);
        assert_eq!(se.event.event_id, "offset-42", "derived from position");
        assert_eq!(se.position, SourcePosition("offset-42".into()));

        let back = pb::EventEnvelope::from(&se.event);
        assert_eq!(back.event_type, "UserCreated");
        assert_eq!(back.sequence, 7);
        assert_eq!(back.version, 1);
    }

    #[test]
    fn parse_tcp_forms() {
        assert!(parse_tcp("tcp://127.0.0.1:50051").is_ok());
        assert!(parse_tcp("127.0.0.1:0").is_ok());
        assert!(parse_tcp("unix:/tmp/x.sock").is_err());
        assert!(parse_tcp("not-an-addr").is_err());
    }
}
