---
objective: "winclean can safely plan and remove explicitly named local Ollama models through Ollama itself without ever deleting the blob store directly."
status: implemented
---

# Plan: Safe Ollama model cleanup

## Overview

| Field      | Value |
| ---------- | ----- |
| **Goal**   | Add an opt-in, dry-run-first cleanup path for explicitly selected Ollama models. |
| **Source** | User request on 2026-08-14, following the disk-usage screenshot of `/usr/share/ollama/.ollama/models/blobs` and the identified winclean coverage gap. |

## Phases

| #   | Phase | File |
| --- | ----- | ---- |
| 1   | Build the fail-closed Ollama adapter | [`phase-1.md`](./phase-1.md) |
| 2   | Wire explicit targeting into winclean | [`phase-2.md`](./phase-2.md) |

## Resources

| Source | Verified |
| ------ | -------- |
| [Ollama FAQ — model storage](https://github.com/ollama/ollama/blob/main/docs/faq.mdx#where-are-models-stored) | Linux normally stores models under `/usr/share/ollama/.ollama/models`, while `OLLAMA_MODELS` can relocate the store. |
| [Ollama API — list models](https://docs.ollama.com/api/tags) | `GET /api/tags` lists locally available models and returns canonical names, digests, modification timestamps, and on-disk byte sizes as JSON. |
| [Ollama API — list running models](https://docs.ollama.com/api/ps) | `GET /api/ps` identifies models currently loaded by the local Ollama server. |
| [Ollama API — delete a model](https://docs.ollama.com/api/delete) | `DELETE /api/delete` accepts one model name and delegates deletion to Ollama. |
| [Ollama CLI reference](https://docs.ollama.com/cli) | `ollama ls`, `ollama ps`, `ollama stop`, and `ollama rm` are the supported user-facing lifecycle operations; there is no documented general `prune` command. |

## Decisions

| Decision | Why |
| -------- | --- |
| Delete through the local Ollama API and never mutate `models/blobs` or `manifests` directly. | Ollama owns digest references and shared layers; bypassing it can corrupt models or delete data still referenced elsewhere. |
| Require exact names through a repeatable `--ollama-model MODEL` flag and exclude the module from ordinary level-wide selection. | Ollama exposes modification time, not last-use evidence, so “unused” cannot be inferred safely and an aggressive run must never mean “remove every model”; a dedicated flag is clearer than introducing a generic targeting language for one module. |
| Restrict the adapter to loopback Ollama endpoints. | winclean is a local disk cleaner; following a remote `OLLAMA_HOST` could destroy models on another machine without reclaiming local space. |
| Classify model deletion as `aggressive`, `no_undo`, and `needs_network`. | Removal is irreversible in the run, and restoring a deleted model normally requires downloading it again. |
| Report API model size as the conservative plan estimate but keep actual freed bytes unknown. | Models may share blobs, so summed logical sizes can overstate reclaimed disk space and must not be presented as a measurement. |
| Record delegated-operation successes and failures independently from byte counts. | An Ollama deletion can succeed or fail while reclaimed bytes remain unknowable; status must not be inferred from `freed`. |
