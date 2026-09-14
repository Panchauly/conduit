# embedded

The "embedded crate" deployment mode (`architecture.md` §1.3): link `conduit-core`
straight into a host service and call `execute_event` — no CLI, no config file on
disk beyond a routing table, no gRPC.

```sh
cargo run --manifest-path examples/embedded/Cargo.toml
```

```
status: Succeeded
  sql-primary -> Created
projected: users.u1.name = "Ada"
```

## The one gotcha

`execute_event` routes through a **process-global** routing table
(`crate::routing::global_routing_table`), loaded once from `$ROUTING_CONFIG`
(default `./routing.json`) — unlike `pipeline::run` / `run_sources`, there's no
explicit-routing-map overload for the single-call API. A real host sets
`ROUTING_CONFIG` once at process startup; this example does the same in-process
before its one call, purely to keep the example self-contained.

## Next

- [`docs/concepts.md`](../../docs/concepts.md) for the mental model.
- [`examples/sqlite-quickstart/`](../sqlite-quickstart/) for the same projection through
  the `conduit` CLI and a real event source instead of one direct call.
