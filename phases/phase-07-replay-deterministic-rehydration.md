# Phase 7 — Replay & Deterministic Rehydration ✅ (Completed)

**Scope**
- **`ReplayContext`:** builds adapters **once** + `adapter_metadata_map`; `run_stream` calls `dispatch_with_routing` per event (Phase 9 dependency order).
- **Explicit routing:** replay loads `routing.json` via path (not process-global cwd); `dispatch_with_routing` for tests/replay.
- **Streaming loader:** `events_from_path` → `Iterator<Item = Result<Event, ReplayLoadError>>` — directory (`*.json` sorted lexicographically), single JSON file, or NDJSON line-by-line.
- **`ReplayReport`:** counts, `stopped_early`, `first_failure_event_id`, `per_event` summaries; serde JSON.
- **Failure policy:** replay respects config `fail_fast` / `continue_on_error` (stop after first failed event vs process all).
- **CLI:** `conduit replay --config --mappings --events <file|dir> [--output text|json]`; exit `1` if any event failed.

**Guarantees**
- Deterministic directory order (sorted paths).
- Integration tests: ordering, NDJSON, fail-fast stop, continue-on-error.

**Phase 7 follow-ups (optimizations)**
- Multiple SQL (or file) adapters per config: same mapping bundle is **cloned** per adapter; routing can list several IDs per event; run order by **Phase 9** (`depends_on` graph, else priority then id).
- Directory loader uses `serde_json::from_reader`; NDJSON uses a single file read + in-memory cursor (no second open).
- `ReplayRunOptions`: cap successful per-event rows (`max_per_event_summaries`), stderr progress (`progress_interval`), strict routing (`validate_routing`); `ReplayReport.per_event_summaries_omitted` when capped.
- CLI: `--max-per-event`, `--progress-every`, `--validate-routing`.
