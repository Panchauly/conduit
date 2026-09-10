# Phase 20 — gRPC Ingestion Service 🎯 (Completed)

**Goal:** Conduit's own technology-agnostic producer interface. A service in any language, behind any event store, streams events to Conduit over a Conduit-defined gRPC contract and gets back a durable position ack. This is the production ingestion path for the **sidecar** deployment; the file sources stay for dev and backfill.

**Why:**
- Part 3 of the three-part model (`architecture.md` §1.2): ingestion must have **no technology lock-in** — never "you must run Kafka" or "you must have a Postgres outbox." Conduit defines the contract; transports implement it.
- Phase 17 built the `EventSource` trait but shipped only `directory` / `stdin` — neither is a production ingest path.
- The `.proto` file *is* the cross-language producer SDK: `protoc` generates a client for Go, Java, C#, Python, Node with zero maintenance. For the open-core product this is the adoption funnel.
- Pure-Rust (`tonic` / `prost`) — static-links into the single-binary sidecar with no C toolchain, unlike a broker client. `tokio` is already in the tree (Phase 19's `postgres` crate).
- Native Kafka / Kinesis sources are a **Pro** module (`architecture.md` §1.4) — they implement the same `EventSource` trait; they are not core.

---

## Phase 20.1 — The contract (`.proto`) 📜 (Completed)

**Scope**
- `proto/conduit/v1/ingest.proto` — the public, versioned contract:

  ```proto
  service Ingest {
    // Bidirectional: producer streams events, Conduit streams position acks.
    rpc Stream(stream EventEnvelope) returns (stream Ack);
  }

  message EventEnvelope {
    string event_type = 1;
    string payload    = 2;   // JSON, same as the file sources
    map<string,string> metadata = 3;
    uint32 version    = 4;
    uint64 sequence   = 5;
    string position   = 6;   // opaque, producer-defined — echoed back in Ack
  }

  message Ack {
    string position = 1;     // highest position durably projected (guards committed)
    uint32 batch_events = 2;
  }
  ```

- `EventEnvelope` maps 1:1 onto `Event` (`event_type`, `payload`, `metadata`, `version`, `sequence`) — decode is the same path as an NDJSON line.
- `position` is **producer-defined and opaque to Conduit** — a Kafka offset map, an outbox `id`, an event-store revision, whatever the producer uses to resume. Conduit only echoes the last durably-projected one back.

**Guarantees**
- The `.proto` is the SDK. It is versioned (`conduit/v1`); breaking changes get `v2`, both served during a deprecation window.

**As built**
- `proto/conduit/v1/ingest.proto` at the workspace root. `EventEnvelope` gained a field 7, `string event_id` (optional) — `conduit_core::Event` requires an `event_id` and it belongs on the guard row / DLQ filename / execution report; when empty, Conduit derives it from `position` (or `event_type + sequence`). It is **not** load-bearing for gating.
- Codegen: `tonic-build` + `protoc-bin-vendored` (a vendored `protoc` binary — no system install, no `protox`). `build.rs` in both `conduit-ingest` and `conduit-client`.

---

## Phase 20.2 — The server & `GrpcSource` 🖥️ (Completed)

**Scope**
- New crate `conduit-ingest` (keeps `tonic` out of `conduit-core`): a `tonic` server implementing `Ingest`, plus `GrpcSource` implementing `EventSource`.
- Wiring: the `Stream` handler pushes decoded events into a **bounded** channel; `GrpcSource::poll(max_batch)` drains it. The bound provides backpressure — when the engine is behind, the channel fills, the gRPC handler's `send` blocks, and gRPC flow control propagates that to the producer. This bounded receive buffer is a socket buffer, not event storage — Conduit still persists nothing.
- `commit(position)` resolves the pending ack: the server sends `Ack { position }` on the response stream once that batch's guards are committed. `committed_position()` returns the last acked position (in-memory; not checkpointed — see Guarantees).
- Listen address from config: `SourceConfig::Grpc { id, listen: "unix:/var/run/conduit.sock" | "tcp://0.0.0.0:50051", tls: Option<...> }`. Unix-domain-socket is the sidecar default.

**Guarantees**
- **Stateless ingestion.** Conduit checkpoints nothing for a gRPC source. The `Ack` is the durable signal — it means the guards recorded the projection. On reconnect the producer resumes from its last received `Ack.position`; any events it re-sends are absorbed by the idempotent sinks (Phases 11–16). This is the `EventSource` "transport owns offsets" rule (Phase 18.8) taken to its limit: the producer owns the offset entirely.
- One `Stream` call = one logical producer session. Concurrent sessions are allowed but each must own a disjoint set of entities (producer-contract.md §4).

**As built**
- `conduit-ingest`: `IngestService` (the `tonic` server), `GrpcSource: EventSource`, `start(id, listen) -> (GrpcServer, GrpcSource)`, and `serve(config, mappings, opts, stop)` — the whole `load_project → start → run_sources → shutdown` orchestration, so the CLI stays thin.
- The `Stream` handler spawns a reader task: inbound envelopes → a bounded `tokio::sync::mpsc` (`CHANNEL_BOUND = 1024`); `send().await` on a full channel is the backpressure point. `GrpcSource::poll` is a non-blocking `try_recv` drain. `commit(position)` broadcasts an `AckMsg` (`tokio::sync::broadcast`) that every session forwards as an `Ack`.
- **Deviation:** `start` is **not** driven from config as a `SourceConfig::Grpc` variant — it is invoked by `conduit ingest --listen <addr>` / `serve()`. Adding a config variant + a fallible `build_sources` for a network listener was more churn than value; a directory/stdin project doesn't gain a listener, and `conduit ingest` is the intended entry point.
- **Deviation:** **TCP only** (`tcp://host:port` or a bare `host:port`). Unix-domain-socket (`unix:/path`) returns a clear config error — `tokio::net::UnixListener` is not available on Windows (the implementing platform); a `#[cfg(unix)]` UDS listener is straightforward follow-up. `local_addr` is reported so `:0` ephemeral binds work (used by every test).
- Latency: in `Continuous` mode the loop sleeps `poll_interval` (200 ms default) when caught up, so a lull adds up to that before the next batch dispatches. A notify-driven wake is possible but out of scope.

---

## Phase 20.3 — Reference client & codegen 🧰 (Completed)

**Scope**
- `conduit-client` (Rust) — a thin wrapper over the generated `tonic` client: `connect(addr)`, `send(event, position)`, `acks()` stream. The reference implementation and an integration-test dependency.
- `proto/` is published as the SDK artifact; document `protoc` / `buf` codegen for Go, Java, Python, C#, Node with a one-paragraph "how to be a producer" per language.
- A `conduit ingest --listen <addr>` CLI subcommand that runs a `GrpcSource` in a normal `conduit run` loop.

**Guarantees**
- A producer in any `protoc`-supported language can feed Conduit with generated stubs and ~20 lines of glue.

**As built**
- `conduit-client`: `Producer::connect(endpoint)` / `send(EventEnvelope)` / `next_ack() -> Option<Result<Ack, _>>` / `finish()`. The request stream is a `tokio::sync::mpsc` → `ReceiverStream`; `send` awaits when the client-side channel or the server's HTTP/2 window is full.
- `conduit ingest --config --mappings [--listen --max-batch --dlq --output]` — one CLI call into `conduit_ingest::serve` with a `ctrlc` stop flag, then renders the `SourceRunReport` (reused from Phase 17).

---

## Phase 20.4 — Failure, shutdown, TLS 🛡️ (Completed)

**Scope**
- Producer disconnects mid-stream → the un-acked tail is simply not acked; the producer reconnects and resends from its last `Ack`. No partial commit.
- Engine batch fails (hard adapter error) → no `Ack` for that batch; Phase 17.5 retry/halt semantics apply; the producer sees no ack and stalls (backpressure) until Conduit recovers or the operator intervenes.
- Poison event (undecodable / build error) → DLQ envelope (Phase 7 / 17.5), and it **is** acked (parked, not lost) so the stream doesn't wedge.
- Graceful shutdown (SIGINT/SIGTERM, Phase 17.3): stop accepting new stream items, drain and commit the in-flight batch, send its `Ack`, close the streams, exit.
- TLS: optional server-side TLS for the TCP listener (rustls, via `tonic`); the UDS listener relies on filesystem permissions. mTLS is a Pro concern.

**Guarantees**
- No event is acked unless it was projected, skipped by a guard, or DLQ'd.

**As built**
- Disconnect mid-stream → the reader task ends, the un-drained tail is not acked; a new `Stream` call resumes (verified by `grpc_reconnect_resume`).
- Hard batch failure → Phase 17.5 retry, then DLQ (if `--dlq` set) or `RetryExhausted` halt. A DLQ'd poison event **is** acked so the stream doesn't wedge (`grpc_poison_event_acked`).
- Graceful shutdown: `conduit ingest` installs a `ctrlc` handler → the `run_sources` loop finishes its current batch (atomic — the loop only checks `stop` between complete batches, so "in-flight batch committed" is automatic), commits, broadcasts the final `Ack`, returns `StoppedReason::Shutdown`; then `GrpcServer::shutdown` stops `tonic` with a 5 s drain. Buffered-but-unpolled events are not acked and the producer resends them (`grpc_graceful_shutdown_commits_in_flight`).
- **Deviation:** **TLS is not implemented.** `tonic`'s rustls integration is a small addition (`Server::builder().tls_config(...)`), but it needs a cert/key source in config and a test cert; deferred. The UDS listener (also deferred) is the sidecar-in-pod security boundary the doc leans on. Documented, not silently dropped.

---

## Phase 20.5 — Verification 🧪 (Completed)

- `.proto` compile check in CI (`prost-build` / `buf lint`).
- Unit: `EventEnvelope` ↔ `Event` round-trip; the bounded-channel backpressure (a slow consumer blocks the producer, no unbounded growth).
- Integration (in-process `tonic` server + `conduit-client`, **no external infra**):

| Test | Asserts |
|---|---|
| `grpc_stream_projects.rs` | A stream of events → projected; `Ack.position` matches the last committed batch. |
| `grpc_reconnect_resume.rs` | Kill the client mid-stream, reconnect, resend from last `Ack` → sinks converge, guards unchanged (no double-apply). |
| `grpc_backpressure.rs` | A stalled engine → the client's `send` blocks; the channel does not grow unbounded. |
| `grpc_poison_event_acked.rs` | An undecodable envelope → DLQ'd and acked; the rest of the stream projects. |
| `grpc_vs_directory.rs` | The same events via gRPC and via a directory → identical projected state. |
| `grpc_graceful_shutdown.rs` | SIGTERM mid-stream → in-flight batch committed and acked, then exit; a restart resumes cleanly from the producer's last ack. |

**Guarantee under test:** a gRPC producer is behaviourally interchangeable with the file sources for projection, and the resume-from-ack loop is effectively-once across disconnects and restarts with Conduit holding no ingestion state.

**As built** — all six scenarios live in **one file, `crates/conduit-ingest/tests/grpc_integration.rs`** (`grpc_stream_projects`, `grpc_reconnect_resume`, `grpc_backpressure_bounds_the_channel`, `grpc_poison_event_acked`, `grpc_vs_directory_converge`, `grpc_graceful_shutdown_commits_in_flight`), sharing a `Harness` (in-process `tonic` server + the engine loop on a background thread + the reference client on its own runtime). Plus unit tests in `conduit-ingest/src/lib.rs` (`EventEnvelope` ↔ `Event` round-trip, `parse_tcp` forms). No `buf` / external `protoc` in CI — the vendored `protoc` in `build.rs` is the compile check. Full workspace: `cargo build`, `cargo test` (224 tests), `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` all clean.

---

## Non-Goals (explicit)

- **No event storage or buffering beyond a bounded receive channel.** Conduit does not persist, queue, or replay events on the ingestion side — the producer owns replay from its last ack.
- **No broker / cloud-connector sources in core.** Kafka, Kinesis, Pub/Sub native `EventSource` implementations are a Pro module on the same trait.
- **No Schema Registry / Avro / Protobuf event payloads.** `payload` is JSON, like every other source; the `.proto` frames the envelope, not the domain payload.
- **No exactly-once delivery.** Effectively-once via idempotent sinks + resume-from-ack (Phase 17.2) stays the model.
- **No producer authn/authz beyond transport TLS.** API keys, mTLS identity, per-producer ACLs are Pro.
- **No multi-node ingestion coordination.** One Conduit process owns its stream sessions; a distributed coordinator is Pro.
- **`tonic` stays out of `conduit-core`.** The embeddable crate has no gRPC dependency; the server lives in `conduit-ingest`.
