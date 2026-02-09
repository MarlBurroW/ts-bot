# Implementation Plan: Refonte du systeme de Trigger Word

**Branch**: `002-trigger-word-refactor` | **Date**: 2026-02-07 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/002-trigger-word-refactor/spec.md`

## Summary

Refactoriser le systeme de detection de trigger word pour le rendre testable et modulaire. Le pipeline actuel est entremele dans main.rs avec des constantes hardcodees. L'objectif est d'extraire un pipeline de detection autonome, testable avec les fichiers audio WAV existants dans `samples/`, et d'ameliorer l'algorithme de matching du trigger word avec la distance de Levenshtein pour couvrir les variations non prevues de Whisper.

## Technical Context

**Language/Version**: Rust 1.75+ (latest stable)
**Primary Dependencies**: whisper-rs 0.12 (whisper-cpp-tracing), audiopus 0.2, hound 3.5, tokio 1, tsclientlib 0.2
**Storage**: N/A (fichiers WAV locaux pour tests)
**Testing**: cargo test (unit + integration tests sur samples WAV)
**Target Platform**: Windows (build avec Visual Studio 2022, LIBCLANG_PATH requis)
**Project Type**: Single binary avec modules
**Performance Goals**: Detection du trigger word en < 2s sur un segment de 5s d'audio
**Constraints**: Modele Whisper ggml-small.bin doit etre present localement, build C++ long
**Scale/Scope**: 4 samples de test, pipeline audio mono-speaker

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

Constitution non configuree (template vierge). Aucune gate specifique a verifier.
Principes generaux appliques: modularite, testabilite, simplicite (YAGNI).

## Project Structure

### Documentation (this feature)

```text
specs/002-trigger-word-refactor/
├── plan.md              # This file
├── research.md          # Phase 0 output - decisions techniques
├── data-model.md        # Phase 1 output - entites et relations
├── quickstart.md        # Phase 1 output - guide de demarrage
└── tasks.md             # Phase 2 output (via /speckit.tasks)
```

### Source Code (repository root)

```text
src/
├── audio/
│   ├── mod.rs             # MODIFIE - Ajout exports pipeline, wav_loader
│   ├── buffer.rs          # INCHANGE - Buffer circulaire par speaker
│   ├── decoder.rs         # INCHANGE - Decodeur Opus
│   ├── pipeline.rs        # NOUVEAU - Pipeline de detection testable
│   ├── wake_word.rs       # MODIFIE - Ajout distance Levenshtein, refactoring matching
│   ├── whisper.rs         # MODIFIE - Extraction segmentation, interface pipeline
│   └── wav_loader.rs      # NOUVEAU - Chargement WAV et conversion 16kHz f32
├── main.rs                # MODIFIE - Delegation au pipeline
├── models/                # INCHANGE
├── ts3/                   # INCHANGE
├── websocket/             # INCHANGE
└── lib.rs                 # MODIFIE - Export pipeline

samples/
├── samples.md             # MODIFIE - Format structure pour parsing automatique
├── sample1.wav
├── sample2.wav
├── sample3.wav
└── sample4.wav

tests/
├── trigger_word_tests.rs  # NOUVEAU - Tests d'integration sur samples WAV
└── integration/           # INCHANGE
```

**Structure Decision**: Single project existant. Les nouveaux fichiers s'integrent dans la structure `src/audio/` existante. Un seul fichier de test d'integration ajoute dans `tests/`.

## Design Decisions

### D1: Pipeline comme struct concrete (pas trait)

Le pipeline sera une struct `TriggerWordPipeline` avec des methodes publiques. Pas besoin de trait car il n'y a qu'une seule implementation et le mocking n'est pas necessaire (on teste avec le vrai Whisper sur de vrais samples).

**Raison**: Simplicite. Un trait ajouterait de l'indirection sans benefice reel puisque les tests utilisent le vrai pipeline.

### D2: Segmentation par silence dans le pipeline

Le pipeline segmente l'audio aux silences (fenetres de ~500ms avec RMS < seuil) avant de transcrire chaque segment. Cela reproduit le comportement du streaming TS3 ou le silence declenche la transcription.

**Raison**: Permet de tester le comportement multi-detection (sample2) avec un fichier audio complet.

### D3: Distance de Levenshtein pour le matching

En complement du matching par substring existant (liste de variations connues), ajouter un fallback par distance de Levenshtein normalisee. Un mot est considere comme match si sa distance a "marlbot" est <= 2 caracteres.

**Raison**: Couvre les variations Whisper non prevues sans augmenter la liste de mots hardcodes indefiniment. Le seuil de 2 rejette "carlbot" (distance 3) et "yarlbot" (distance 3) tout en acceptant "marbut" (distance 2) et "malbot" (distance 1).

### D4: Format samples.md structure

Reformater samples.md en sections parsables avec des marqueurs clairs :
- `## sample1` pour delimiter chaque sample
- `Transcription:` pour la description de l'audio
- `Expected:` pour les resultats attendus (un par ligne avec `-`)

**Raison**: Permet un parsing automatique simple sans ajouter de format de serialisation (JSON/TOML/YAML).

### D5: Tests d'integration, pas unitaires pour le pipeline complet

Les tests sur samples WAV sont des tests d'integration (dans `tests/`) car ils necessitent le modele Whisper et les fichiers audio. Les tests unitaires restent dans chaque module (wake_word.rs, whisper.rs).

**Raison**: Separation claire entre tests rapides (unitaires, sans I/O) et tests lents (integration, avec Whisper).

## Complexity Tracking

Aucune violation de constitution a justifier (constitution non configuree).
