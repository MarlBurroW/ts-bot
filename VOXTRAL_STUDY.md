# Voxtral Transcribe 2 — Feasibility Study

**Date:** 2026-02-12
**Author:** Marlbot (cron improvement agent)
**Status:** Recommendation ready

---

## 1. Current Setup

- **STT:** OpenAI Whisper API (`whisper-1`)
- **Cost:** $0.006/min
- **Latency:** ~1.2s per transcription
- **Audio input:** WAV (PCM 16-bit, 16kHz mono) — converted from Opus in the bot pipeline
- **Quality:** Good for FR and EN

## 2. Voxtral Transcribe 2 — Overview

Mistral released Voxtral Transcribe 2 (Feb 2026) in two variants:

| Model | Params | Use Case | API Price | Open Weights |
|-------|--------|----------|-----------|-------------|
| Voxtral Mini Transcribe V2 | 4B | Batch transcription | $0.003/min | No |
| Voxtral Realtime | 4B | Real-time streaming | $0.006/min | Yes (Apache 2.0) |

**Claimed benchmarks:** 4% WER on FLEURS (Whisper: higher), 3x faster than ElevenLabs Scribe v2.
**Languages:** 13 supported (includes FR and EN).
**Features:** Speaker diarization, context biasing, word-level timestamps.

## 3. Can We Self-Host?

### Host Specs
- **CPU:** Intel i3-8109U @ 3.00GHz (2 cores / 4 threads)
- **RAM:** 7.6 GB
- **GPU:** ❌ None (no NVIDIA GPU)

### Requirements for Voxtral Self-Hosted
- **Minimum:** NVIDIA GPU with 16 GB VRAM (bf16 mode needs ~10 GB)
- **Recommended:** RTX 3090/4090 (24 GB) or A40
- **CPU-only:** ❌ Not supported for real-time

### Verdict: **Self-hosting is NOT feasible**
The host has no GPU at all. Voxtral requires NVIDIA GPU with 16+ GB VRAM. Even with quantization, CPU-only inference on a 4B model would be far too slow for real-time STT (probably 10-30x realtime on this i3).

## 4. Hosted API Comparison

| | OpenAI Whisper | Voxtral Mini V2 (batch) | Voxtral Realtime |
|---|---|---|---|
| **Price** | $0.006/min | $0.003/min (50% cheaper) | $0.006/min (same) |
| **Latency** | ~1.2s | Unknown (batch, likely higher) | <200ms (streaming) |
| **FR quality** | Very good | Claims better (4% WER FLEURS) | Same model |
| **Input formats** | mp3, wav, m4a, flac, webm | mp3, wav, m4a, flac, ogg | Streaming PCM/chunks |
| **Opus support** | ❌ (convert to wav) | ❌ (convert to wav) | TBD |
| **API maturity** | Stable (2+ years) | New (Feb 2026) | New (Feb 2026) |
| **OpenAI-compatible** | N/A | Yes (similar endpoint) | WebSocket streaming |

### Key observations:

1. **Batch (Mini V2) at $0.003/min** — 50% cost savings, but batch mode may add latency. Our use case is real-time voice chat, not batch processing.

2. **Realtime at $0.006/min** — Same price as Whisper, but with streaming (<200ms latency). However, integrating streaming STT requires significant pipeline changes (WebSocket-based audio streaming instead of file upload).

3. **Audio format** — Both require WAV/MP3 (no direct Opus). Our current pipeline already converts Opus→WAV, so no change needed for the batch API. The Realtime streaming API may accept raw PCM chunks directly.

## 5. Integration Effort

### Option A: Replace Whisper with Voxtral Mini V2 (batch API)
- **Effort:** Low — swap the HTTP endpoint and adjust request format
- **Benefit:** 50% cost reduction ($0.003 vs $0.006/min)
- **Risk:** New API, possible latency regression, less battle-tested
- **Pipeline change:** Minimal — same pattern (upload audio file → get text)

### Option B: Replace Whisper with Voxtral Realtime (streaming API)
- **Effort:** High — requires WebSocket streaming client, chunked audio sending, partial result handling
- **Benefit:** Sub-200ms latency (vs ~1.2s currently), same cost
- **Risk:** Major pipeline refactor, new failure modes
- **Pipeline change:** Major — replace file-based STT with streaming WebSocket

## 6. Recommendation

**Short-term: Do nothing (keep Whisper).**
- The cost savings ($0.003/min difference) are minimal for our usage volume (a few minutes of voice per day = pennies)
- Whisper is battle-tested and reliable
- Voxtral API just launched — let it mature

**Medium-term (if cost becomes a concern): Option A (batch API swap)**
- Easy integration, 50% cost savings
- Wait 2-3 months for Voxtral API to stabilize and get community feedback on FR quality

**Long-term (if latency is critical): Option B (streaming)**
- Only worth it if the ~1.2s latency is a pain point
- Significant engineering effort for marginal UX improvement

**Bottom line:** Not worth switching right now. Whisper works well, costs are low, and Voxtral is brand new. Revisit in Q2 2026 if cost or latency become actual problems.
