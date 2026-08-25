# Voice UX Six Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Improve utterance continuity, staged TTS timing, playback continuity, and safe spoken interruption across the existing VoiceDesk pipeline.

**Architecture:** Keep the existing cpal/Silero/STT and staged queue boundaries. Delay STT dispatch inside `SpeechDetector`, pre-generate queued TTS segments at enqueue time while preserving ordered playback and epoch cancellation, and key frontend bubble reveal to backend playback-start events. A separate strict Silero probability detector processes suspended-mic samples for barge-in.

**Tech Stack:** Rust 2021, Tauri v2, cpal, ort/Silero VAD, Vue 3, TypeScript.

**Spec:** User-provided “VoiceDesk 六项语音 UX 修复” request dated 2026-08-25.

## Global Constraints

- Work on `feat/voice-ux-six-fixes` branched from `main`; commit but do not push.
- Add no dependencies and do not change JARVIS persona or Hermes API behavior.
- Preserve TTS playback counters, echo cooldown, STT in-flight gating, queue round cancellation, and generation epoch cancellation.
- Verify with `cargo check`, `npx vue-tsc --noEmit`, and `git log`.

---

### Task 1: VAD events and continuation grace

**Files:** Modify and test `src-tauri/src/audio/vad.rs`, `src-tauri/src/audio/capture.rs`.

**Interfaces:** `VadEngine::process_samples(&[i16]) -> Vec<VadEvent>` returns every event in a chunk. `SpeechDetector` retains completed audio until 1200 ms of no resumed speech, while `STT_IN_FLIGHT` reserves the pending utterance.

- [x] Add unit tests for multi-event return and continuation deadline state.
- [x] Run targeted Rust tests for the new expectations.
- [x] Set end confirmation to 22 frames and implement pending audio/deadline merging.
- [x] Run targeted tests and confirm they pass.

### Task 2: Strict barge-in detector

**Files:** Modify and test `src-tauri/src/audio/vad.rs`, `src-tauri/src/audio/capture.rs`, `src-tauri/src/audio/player.rs`, `src-tauri/src/lib.rs`.

**Interfaces:** `VadEngine::process_probabilities` supplies Silero frame probabilities. Capture exports `start_barge_in_detection`, `stop_barge_in_detection`, and `set_barge_in_enabled`; detection requires probability above 0.85 for 14 frames after an 800 ms guard.

- [x] Add tests for the consecutive-frame trigger/reset rule.
- [x] Run targeted Rust tests for the trigger rule.
- [x] Route suspended mic samples through an independent detector and reset TTS on trigger.
- [x] Start/stop detection with every playback/reset lifecycle and expose the enable command.
- [x] Run targeted tests and confirm they pass.

### Task 3: Prefetched ordered staged playback and segment-start events

**Files:** Modify and test `src-tauri/src/audio/player.rs`.

**Interfaces:** Each queued segment owns a round-local index and background generation receiver. The single queue worker consumes prepared audio in order and emits `tts:segment-start` immediately before the first `afplay` call for that segment.

- [x] Review queue/index lifecycle invariants in the implementation diff.
- [x] Run Rust compilation and existing queue coverage.
- [x] Split segment preparation from playback and start preparation at enqueue time.
- [x] Preserve round and generation-epoch cancellation plus cleanup of generated files.
- [x] Run the full Rust test suite and confirm it passes.

### Task 4: Playback-timed bubbles and UI controls

**Files:** Modify `src/views/VoiceChat.vue`, `src/components/ChatBubble.vue`.

**Interfaces:** Frontend stores `Map<number, string>` for staged bubbles, reveals on `tts:segment-start`, flushes unrevealed text on completion/failure, and calls `set_barge_in_enabled(bool)` from a session-local default-on button.

- [x] Queue text by segment index rather than immediately adding messages.
- [x] Add segment-start and barge-in event listeners plus finish/error fallbacks.
- [x] Add the barge-in toggle and left-align bubble text.
- [x] Run Vue type checking.

### Task 5: Full verification and commit

**Files:** All modified files.

- [x] Run formatting and inspect the complete diff.
- [x] Run `cargo check` under `src-tauri`.
- [x] Run `npx vue-tsc --noEmit` at repository root.
- [x] Commit the completed implementation and show `git log`.
