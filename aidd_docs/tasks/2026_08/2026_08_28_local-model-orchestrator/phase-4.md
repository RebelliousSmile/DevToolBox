---
status: done
---

# Instruction: Download through Hugging Face/Xet and direct HTTPS

## Architecture projection

> Tree of the final files. ✅ create · ✏️ modify · ❌ delete

```txt
scripts/model_orchestrator/
├── providers/
│   ├── __init__.py                 ✅ provider protocol and registry
│   ├── huggingface.py              ✅ exact HF resolver and Xet download
│   └── direct.py                   ✅ exact HTTPS download and guarded resume
├── download.py                     ✅ request offer plan and execution core
├── events.py                       ✅ versioned NDJSON progress protocol
├── models.py                       ✏️ acquisition and event records
├── __main__.py                     ✏️ provider resolve and download commands
└── tests/                          ✏️ fake hf and HTTP coverage
```

Deleted files: none.

## User Journey

```mermaid
flowchart TD
  A[Enter exact HF locator or URL] --> B[Resolve immutable file evidence]
  B --> C[Review no-conversion offer and disk cost]
  C --> D[Stream into staging with progress]
  D --> E[Validate identity and commit to library]
```

## Test Scope

```mermaid
---
title: Test scope
---
journey
  section Setup
    Install fake hf CLI and HTTP range server => bytes metadata and failures are deterministic: 5: system
  section Happy path
    Download exact GGUF and image locators => monotonic events end in verified library artifacts: 5: cli
  section Edge case - missing dependency
    Remove hf CLI => setup guidance appears and nothing is installed: 1: cli
  section Edge case - unsafe resume
    Change ETag length or redirect scheme => partial bytes are not appended or committed: 1: cli
```

## Tasks to do

### `1)` Define exact acquisition and progress contracts

> Make transport comparable without promising remote federated search.

1. Define request, provider status, offer, immutable plan, progress, and result records.
2. Accept one primary exact locator plus optional user alternatives; group offers only with equal immutable revision, file path, format, and trusted digest.
3. Mark conversion-required offers visible but non-executable in v1 and expose network, local-copy, temporary-space, resume, and identity evidence.
4. Emit one NDJSON schema header, monotonic progress, and exactly one redacted terminal event.

### `2)` Implement Hugging Face/Xet

> Download directly to staging with provider-owned authentication.

1. Detect `hf`, version, and auth state without reading tokens; never install it automatically.
2. Require exact repository, immutable revision, and filename; invoke `hf download --local-dir` with `HF_XET_HIGH_PERFORMANCE=1` unless disabled.
3. Capture immutable LFS/Xet SHA-256 when available and avoid a second local hash pass.
4. Preserve resumable metadata without duplicating the canonical allocation.

### `3)` Implement direct HTTPS

> Support arbitrary exact files without weakening network safety.

1. Allow HTTPS plus explicit loopback HTTP, reject URL credentials, redact queries, cap redirects, and reject unsafe scheme/origin changes.
2. Resume only when ETag or Last-Modified plus length remains stable; otherwise restart the partial file.
3. Compute SHA-256 inline when no trusted user checksum exists.
4. Translate timeout, cancellation, disk exhaustion, checksum mismatch, malformed output, and nonzero exits into stable errors.

## Test acceptance criteria

| Task | Acceptance criteria |
| ---- | ------------------- |
| 1 | Exact same-byte offers are comparable and every transfer has parseable monotonic redacted progress. |
| 2 | HF/Xet uses immutable exact locators, existing auth, optional high-performance mode, and no avoidable second read. |
| 3 | Direct transfers resume only with stable evidence and unsafe URLs or changed content never commit. |
