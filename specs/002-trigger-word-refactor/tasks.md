# Tasks: Refonte du systeme de Trigger Word

**Input**: Design documents from `/specs/002-trigger-word-refactor/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, quickstart.md

**Tests**: Tests are included as explicitly requested in spec (FR-001, FR-002, FR-012, SC-001, SC-002, SC-006).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup

**Purpose**: Prepare shared infrastructure and structured test data

- [X] T001 Restructure samples/samples.md with parsable format: `## sampleN` headers, `Transcription:` field, `Expected:` list with `- detected: yes/no` and `- command: "text"` markers per expected message
- [X] T002 [P] Create src/audio/wav_loader.rs with `load_wav(path) -> Result<Vec<f32>>` function: read WAV via hound crate, convert to mono f32 at 16kHz (resample if needed), normalize to [-1.0, 1.0]
- [X] T003 [P] Create data structures for DetectionResult (detected, transcription, command, confidence) and AudioSegment (samples, start_ms, end_ms, rms_energy) in src/audio/pipeline.rs
- [X] T004 Update src/audio/mod.rs to add `pub mod pipeline;` and `pub mod wav_loader;` declarations and re-exports

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core audio segmentation and sample expectation parsing that ALL user stories depend on

**CRITICAL**: No user story work can begin until this phase is complete

- [X] T005 Implement `segment_by_silence(samples: &[f32], sample_rate: u32, silence_threshold: f32, min_silence_duration_ms: u64) -> Vec<AudioSegment>` function in src/audio/pipeline.rs: slide a window over audio, calculate RMS per window, split at silence gaps using existing MIN_SPEECH_RMS logic from src/audio/whisper.rs
- [X] T006 [P] Create SampleExpectation and ExpectedMessage structs in src/audio/pipeline.rs, implement `parse_samples_md(path: &Path) -> Result<Vec<SampleExpectation>>` to parse the structured samples/samples.md format
- [X] T007 [P] Add unit tests for `segment_by_silence` in src/audio/pipeline.rs: test with synthetic audio (silence + tone patterns), verify correct segment boundaries
- [X] T008 [P] Add unit tests for `parse_samples_md` in src/audio/pipeline.rs: test with the actual samples/samples.md file, verify all 4 samples are parsed with correct expectations

**Checkpoint**: Foundation ready - wav loading, audio segmentation, and sample parsing all working

---

## Phase 3: User Story 1 - Test automatise avec samples audio (Priority: P1) MVP

**Goal**: Run automated tests on the 4 WAV samples that verify trigger word detection and command extraction against expected results from samples.md

**Independent Test**: `cargo test --test trigger_word_tests` executes all sample-based tests and reports pass/fail with transcription details

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [X] T009 [US1] Create tests/trigger_word_tests.rs with test scaffold: load Whisper model once via `lazy_static` or `std::sync::OnceLock`, parse samples/samples.md for expectations, define one `#[test]` function per sample (test_sample1, test_sample2, test_sample3, test_sample4)
- [X] T010 [US1] Implement test_sample1 in tests/trigger_word_tests.rs: load samples/sample1.wav, run pipeline, assert trigger word detected, assert extracted command matches "connecte toi a home assistant et eteins toutes mes lumieres" (fuzzy string comparison)
- [X] T011 [US1] Implement test_sample2 in tests/trigger_word_tests.rs: load samples/sample2.wav, run pipeline, assert exactly 2 detection results, assert first command matches "comment ca va ? tu va bien ? moi ca va c'est cool", assert second matches "qu'est ce que tu fait de beau la ?"
- [X] T012 [US1] Implement test_sample3 in tests/trigger_word_tests.rs: load samples/sample3.wav, run pipeline, assert non-trigger speech is ignored, assert command matches "comment ca va ? qu'est ce que tu fait de beau ?"
- [X] T013 [US1] Implement test_sample4 in tests/trigger_word_tests.rs: load samples/sample4.wav, run pipeline, assert "Carlbot"/"Yarlbot"/"Sarlbot" do NOT trigger detection, assert only "marlbot" triggers, assert command matches "comment ca va ?"

### Implementation for User Story 1

- [X] T014 [US1] Implement `TriggerWordPipeline` struct in src/audio/pipeline.rs: holds WhisperTranscriber + WakeWordDetector, constructor `new(model_path, bot_name) -> Result<Self>`
- [X] T015 [US1] Implement `TriggerWordPipeline::process_audio(samples: &[f32]) -> Result<Vec<DetectionResult>>` in src/audio/pipeline.rs: segment audio by silence, transcribe each segment via WhisperTranscriber, detect wake word via WakeWordDetector, extract command, return Vec<DetectionResult>
- [X] T016 [US1] Implement `TriggerWordPipeline::process_wav_file(path: &Path) -> Result<Vec<DetectionResult>>` in src/audio/pipeline.rs: load WAV via wav_loader, delegate to process_audio
- [X] T017 [US1] Add detailed test output formatting in tests/trigger_word_tests.rs: on assertion failure, print transcription obtained vs expected, detection status, and segment boundaries for debugging (FR-012)
- [X] T018 [US1] Run `cargo test --test trigger_word_tests` and verify all 4 sample tests pass. Iterate on segmentation parameters (silence threshold, min silence duration) if needed

**Checkpoint**: All 4 sample tests pass with correct detection and command extraction. `cargo test --test trigger_word_tests` works as single command (SC-002)

---

## Phase 4: User Story 2 - Architecture modulaire et decouplage (Priority: P2)

**Goal**: Each component (transcription, detection, extraction) is independently testable without TS3 connection or main event loop dependency

**Independent Test**: Unit tests for each component pass in isolation; pipeline produces same results from WAV file as from raw audio buffer

### Tests for User Story 2

- [X] T019 [P] [US2] Add unit tests for WakeWordDetector in src/audio/wake_word.rs: test detect() with various Whisper transcription outputs, test extract_command() and extract_command_clean() with multi-trigger inputs, test normalization edge cases (accents, punctuation, mixed case)
- [X] T020 [P] [US2] Add unit tests for WhisperTranscriber in src/audio/whisper.rs: test has_speech_energy() with silent vs speech samples, test is_repetition() with known hallucination patterns, test transcribe_wake_word() returns lowercase French text

### Implementation for User Story 2

- [X] T021 [US2] Extract `segment_by_silence` and RMS energy functions from src/audio/whisper.rs private scope to public in src/audio/pipeline.rs (make has_speech_energy and rms_energy accessible for testing)
- [X] T022 [US2] Refactor src/audio/whisper.rs: make `rms_energy()`, `has_speech_energy()`, and `is_repetition()` public so they can be unit tested directly, keep MIN_SPEECH_RMS as pub const
- [X] T023 [US2] Refactor src/main.rs: replace inline wake word check logic (lines ~358-383) with call to `TriggerWordPipeline` methods, remove duplicated transcription/detection code from event loop
- [X] T024 [US2] Refactor src/main.rs: replace inline silence timeout transcription logic (lines ~220-268) with pipeline delegation, ensure active speaker buffer → pipeline → DetectionResult flow
- [X] T025 [US2] Update src/lib.rs to export TriggerWordPipeline and DetectionResult from audio module
- [X] T026 [US2] Run `cargo test` to verify all existing tests + new unit tests pass, run `cargo clippy` for lint check

**Checkpoint**: Each component (WhisperTranscriber, WakeWordDetector, TriggerWordPipeline) is testable in isolation. main.rs delegates to pipeline

---

## Phase 5: User Story 3 - Amelioration precision de detection (Priority: P3)

**Goal**: Trigger word detection is more reliable with Levenshtein fuzzy matching, reducing false negatives while maintaining 0% false positives

**Independent Test**: All 4 sample tests still pass; Levenshtein correctly accepts "marbut" (dist 2) and rejects "carlbot" (dist 3)

### Tests for User Story 3

- [X] T027 [P] [US3] Add unit tests for Levenshtein matching in src/audio/wake_word.rs: test levenshtein_distance("marlbot", "malbot") == 1, test levenshtein_distance("marlbot", "marbut") == 2, test levenshtein_distance("marlbot", "carlbot") == 3, test levenshtein_distance("marlbot", "yarlbot") == 3, test detection threshold (accept <= 2, reject > 2)
- [X] T028 [P] [US3] Add unit test for combined matching in src/audio/wake_word.rs: test that exact list match still works, test that Levenshtein fallback catches novel variations, test that confidence score is higher for exact matches than Levenshtein matches

### Implementation for User Story 3

- [X] T029 [US3] Implement `levenshtein_distance(a: &str, b: &str) -> usize` function in src/audio/wake_word.rs: standard dynamic programming algorithm, operates on normalized (lowercase, no punctuation) strings
- [X] T030 [US3] Implement `fuzzy_detect(text: &str, max_distance: usize) -> Option<(String, f32)>` in WakeWordDetector in src/audio/wake_word.rs: tokenize transcribed text, compute Levenshtein distance of each word to bot_name, return matched word + confidence score (1.0 - distance/max_distance) if distance <= max_distance
- [X] T031 [US3] Update `WakeWordDetector::detect()` in src/audio/wake_word.rs: first try exact substring match (existing logic), if no match try fuzzy_detect with max_distance=2 as fallback, return true if either matches
- [X] T032 [US3] Update `WakeWordDetector::extract_command()` and `extract_command_clean()` in src/audio/wake_word.rs to handle fuzzy-matched trigger words (strip the matched word variant, not just the exact wake word list)
- [X] T033 [US3] Run all sample tests (`cargo test --test trigger_word_tests`) to verify improved detection on all 4 samples, especially sample4 (reject Carlbot/Yarlbot/Sarlbot, accept marlbot)
- [X] T034 [US3] Run `cargo clippy` and fix any warnings introduced by new code

**Checkpoint**: All 4 samples pass, Levenshtein matching rejects distant names (carlbot, yarlbot, sarlbot) while accepting close variations

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Final cleanup, documentation, and validation

- [X] T035 [P] Verify SC-003: add a placeholder sample5 entry to samples/samples.md (no WAV file needed), confirm test framework gracefully handles missing WAV files or skips the test
- [X] T036 Run full validation: `cargo test` (all tests), `cargo clippy` (zero warnings), verify all 4 sample tests pass (SC-001)
- [X] T037 Clean up any unused imports, dead code, or TODO comments across modified files (src/audio/pipeline.rs, src/audio/wake_word.rs, src/audio/whisper.rs, src/audio/wav_loader.rs, src/main.rs)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies - can start immediately
- **Foundational (Phase 2)**: Depends on T002, T003, T004 from Setup - BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Phase 2 completion (segmentation + sample parsing)
- **US2 (Phase 4)**: Depends on Phase 3 (T014-T015 pipeline struct must exist before refactoring main.rs to use it)
- **US3 (Phase 5)**: Depends on Phase 4 (T022 wake_word refactoring must be done before adding Levenshtein)
- **Polish (Phase 6)**: Depends on all user stories being complete

### User Story Dependencies

- **US1 (P1)**: Can start after Foundational (Phase 2) - No dependencies on other stories
- **US2 (P2)**: Depends on US1's TriggerWordPipeline struct (T014-T015) existing, since main.rs refactoring needs to delegate to it
- **US3 (P3)**: Depends on US2's wake_word.rs refactoring being stable before adding Levenshtein matching

### Within Each User Story

- Tests MUST be written and FAIL before implementation
- Data structures before logic
- Core pipeline before integration with main.rs
- Validate all sample tests pass before moving to next story

### Parallel Opportunities

- T002 and T003 can run in parallel (different files: wav_loader.rs vs pipeline.rs)
- T005, T006, T007, T008 can partially parallelize (T006-T008 parallel, T005 independent)
- T019 and T020 can run in parallel (different files: wake_word.rs vs whisper.rs)
- T027 and T028 can run in parallel (different test functions in same file)

---

## Parallel Example: User Story 1

```bash
# Write all test stubs in parallel (once scaffold T009 is done):
Task T010: "test_sample1 in tests/trigger_word_tests.rs"
Task T011: "test_sample2 in tests/trigger_word_tests.rs"
Task T012: "test_sample3 in tests/trigger_word_tests.rs"
Task T013: "test_sample4 in tests/trigger_word_tests.rs"

# These are in the same file but can be written together since they are independent test functions
```

## Parallel Example: User Story 2

```bash
# Unit tests can be written in parallel (different files):
Task T019: "WakeWordDetector tests in src/audio/wake_word.rs"
Task T020: "WhisperTranscriber tests in src/audio/whisper.rs"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (T001-T004)
2. Complete Phase 2: Foundational (T005-T008)
3. Complete Phase 3: User Story 1 (T009-T018)
4. **STOP and VALIDATE**: Run `cargo test --test trigger_word_tests` - all 4 samples pass
5. MVP delivered: testable trigger word pipeline

### Incremental Delivery

1. Complete Setup + Foundational -> Foundation ready
2. Add User Story 1 -> Test with samples -> MVP delivered
3. Add User Story 2 -> Modular architecture -> Clean codebase
4. Add User Story 3 -> Improved precision -> Final quality
5. Each story adds value without breaking previous stories

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Tests require Whisper model at models/ggml-small.bin and sample WAV files in samples/
- Build environment: LIBCLANG_PATH and CMAKE_GENERATOR must be set
- First build will be slow (~2-3 min) due to whisper-rs C++ compilation
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
