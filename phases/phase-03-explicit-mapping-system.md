# Phase 3 — Explicit Mapping System ✅ (Completed)

**Goal:** Separate *what happened* from *how it is stored*.

### Scope
- SQL mappings (YAML)
- Document mappings (YAML)
- Field-level projection
- Event-to-schema validation

### Guarantees
- Same event can be projected differently per adapter
- No schema inference
- Mapping errors fail fast
