---
objective: "DevToolBox inventories, downloads, shares, and validates local AI model artifacts across Ollama, Jan, LM Studio, and ComfyUI on Windows and Linux without required conversion or durable duplication, and retires only sources with a proven native-safe path."
status: in-progress
---

# Plan: Cross-platform local model orchestrator

## Overview

| Field      | Value |
| ---------- | ----- |
| **Goal**   | Add a neutral local-model library, four download paths, four tool adapters, adaptive recommendations, verified migrations, and explicitly confirmed source retirement. |
| **Source** | [GitHub issue #31](https://github.com/RebelliousSmile/DevToolBox/issues/31), refined by the 2026-08-28 product decisions in conversation. |

## Phases

| #   | Phase | File |
| --- | ----- | ---- |
| 1 | Establish catalog contracts and path safety | [`phase-1.md`](./phase-1.md) |
| 2 | Inventory Ollama, Jan, LM Studio, and ComfyUI | [`phase-2.md`](./phase-2.md) |
| 3 | Build the transactional neutral library | [`phase-3.md`](./phase-3.md) |
| 4 | Download through Hugging Face/Xet and direct HTTPS | [`phase-4.md`](./phase-4.md) |
| 5 | Download through Ollama and LM Studio | [`phase-5.md`](./phase-5.md) |
| 6 | Plan migrations and automate supported LLM destinations | [`phase-6.md`](./phase-6.md) |
| 7 | Integrate Jan and ComfyUI without private unsafe mutation | [`phase-7.md`](./phase-7.md) |
| 8 | Rank, recover, and retire eligible sources | [`phase-8.md`](./phase-8.md) |
| 9 | Add the asynchronous Rust protocol bridge | [`phase-9.md`](./phase-9.md) |
| 10 | Deliver the native Models view and delivery gates | [`phase-10.md`](./phase-10.md) |

## Resources

| Source | Verified |
| ------ | -------- |
| [Ollama FAQ](https://docs.ollama.com/faq) | Default Windows/Linux model stores, `OLLAMA_MODELS`, and the need to discover custom storage. |
| [Ollama API](https://github.com/ollama/ollama/blob/main/docs/api.md) | Tags, running models, pull, show, blob, create, and delete lifecycle surfaces. |
| [Jan model management](https://jan.ai/docs/manage-models) | GGUF import by link or duplicate and preservation of linked source files. |
| [Jan CLI](https://www.jan.ai/docs/desktop/cli) | Shared Desktop/CLI data folder and available model-listing/serving commands. |
| [LM Studio CLI](https://lmstudio.ai/docs/cli) | `lms get`, `ls`, `load`, `unload`, and headless operation. |
| [LM Studio import](https://lmstudio.ai/docs/cli/local-models/import) | Copy, move, hard-link, symbolic-link, and dry-run import modes. |
| [Hugging Face downloads](https://huggingface.co/docs/huggingface_hub/guides/download) | `hf download`, local-directory metadata, resumable cache behavior, and Xet integration. |
| [Hugging Face environment variables](https://huggingface.co/docs/huggingface_hub/package_reference/environment_variables) | `HF_XET_HIGH_PERFORMANCE`, cache locations, and provider-owned authentication. |
| [ComfyUI Linux installation](https://docs.comfy.org/installation/desktop/linux) | Linux support and Desktop data/model locations. |
| [ComfyUI Windows installation](https://docs.comfy.org/installation/desktop/windows) | Windows installation roots and shared-model migration behavior. |
| [ComfyUI folder paths](https://github.com/Comfy-Org/ComfyUI/blob/master/folder_paths.py) | Core model categories, configured model roots, and extension points. |

## Decisions

| Decision | Why |
| -------- | --- |
| Keep model orchestration in a new stdlib-only Python package with a versioned JSON/NDJSON bridge to Rust. | The repository already uses this boundary for cross-platform inventory, while streaming downloads need isolated background work and stable progress events. |
| Store canonical artifacts in a configurable neutral library with progressive identity evidence. | A tool-owned store cannot serve all four consumers safely; trusted provider digests or hashes computed during streaming avoid a blocking second read, while uncertain artifacts remain usable but ineligible for deduplication or retirement until background verification completes. |
| Offer Hugging Face/Xet, Ollama, LM Studio, and direct URL as peer download providers. | Fast Ollama pulls remain available, while developers can choose a provider suited to their connection, tooling, authentication, and model family. |
| Rank exact no-conversion artifacts before using locally learned throughput. | Network speed must not hide conversion, reconstruction, or extra-copy cost; no universal provider is fastest on every machine. |
| Do not execute conversions or install missing dependencies in the first release. | Conversion is the slow path the feature is intended to avoid, and provider setup remains an explicit developer choice. |
| Separate migration, validation, and source retirement into distinct persisted operations. | Technical success never grants permission to delete multi-gigabyte user data, and interrupted work must remain recoverable. |
| Auto-fallback only when the expected content identity is unchanged. | A different revision, format, or quantization is a product choice, not a transport retry. |
| Accept exact provider locators in v1 instead of promising federated catalog search. | Hugging Face, Ollama, LM Studio, and arbitrary URLs do not expose one stable non-interactive search contract; local catalog search remains available after acquisition. |
