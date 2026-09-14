# conduit-cli

The command-line binary for [Conduit](https://github.com/Panchauly/conduit) — a thin
wrapper (argument parsing + rendering only) over `conduit-core`.

```sh
cargo install conduit-cli

conduit run --config config.yaml --mappings mappings/ --once   # drain configured sources once
conduit sources --config config.yaml                            # list sources + committed positions
conduit ingest --config config.yaml --mappings mappings/ --listen tcp://0.0.0.0:50051
conduit explain --config config.yaml --event event.json         # routing/dependency preview, no writes
conduit dry-run  --config config.yaml --mappings mappings/ --event event.json
```

See the root [README](https://github.com/Panchauly/conduit#readme) for a full quickstart
and [`docs/concepts.md`](https://github.com/Panchauly/conduit/blob/master/docs/concepts.md)
for the mental model.

Licensed under either of [MIT](https://github.com/Panchauly/conduit/blob/master/LICENSE-MIT)
or [Apache-2.0](https://github.com/Panchauly/conduit/blob/master/LICENSE-APACHE) at your option.
