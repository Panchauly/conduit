# Phase 4 — Routing System (Adapter ID Based) ✅ (Completed)

**Goal:** Make routing explicit, deterministic, and replica-safe.

### Scope
- `routing.json`
- Routing by **adapter ID** (not storage kind)
- Removal of implicit/default routing
- Support for multiple adapters of same storage kind

### Example
```json
{
  "UserCreated": ["sql-primary", "doc-readmodel"],
  "CacheInvalidated": ["kv-cache"]
}
```

### Guarantees
- No routing → no execution
- Same event can fan out to multiple adapters
- Read/write replicas are first-class
