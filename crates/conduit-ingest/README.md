# conduit-ingest

Conduit's gRPC ingestion server: a `tonic` server implementing the
[`conduit/v1`](https://github.com/Panchauly/conduit/blob/master/proto/conduit/v1/ingest.proto)
`Ingest` contract, plus `GrpcSource` — an `EventSource` (from `conduit-core`) that a normal
`conduit_core::run_sources` loop drives like any file source. `tonic` lives only in this
crate; `conduit-core` stays gRPC-free.

Producers stream `EventEnvelope`s and get back position `Ack`s once each batch's guards
commit — Conduit holds no ingestion checkpoint; the producer owns resume-from-ack.

Most users reach this through the `conduit ingest` subcommand in `conduit-cli`. To embed
the server directly:

```rust,no_run
use std::sync::atomic::AtomicBool;

let (server, source) = conduit_ingest::start("grpc", "tcp://127.0.0.1:50051").unwrap();
println!("listening on {:?}", server.local_addr);
// hand `source` to conduit_core::run_sources(...) alongside your other sources.
```

See [`proto/README.md`](https://github.com/Panchauly/conduit/blob/master/proto/README.md)
for the wire contract and how to generate a client in another language.

Licensed under either of [MIT](https://github.com/Panchauly/conduit/blob/master/LICENSE-MIT)
or [Apache-2.0](https://github.com/Panchauly/conduit/blob/master/LICENSE-APACHE) at your option.
