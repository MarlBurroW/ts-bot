# Data Model: Refonte du systeme de Trigger Word

**Date**: 2026-02-07
**Feature**: 002-trigger-word-refactor

## Entities

### DetectionResult

Represente le resultat du pipeline de detection pour un segment audio.

| Field          | Type            | Description                                              |
|----------------|-----------------|----------------------------------------------------------|
| detected       | bool            | Le trigger word a-t-il ete detecte dans ce segment       |
| transcription  | String          | Texte brut transcrit par le moteur STT                   |
| command        | Option<String>  | Commande extraite apres le trigger word (None si pas detecte) |
| confidence     | f32             | Score de confiance de la detection (0.0 - 1.0)           |

### AudioSegment

Represente un segment audio isole (decoupage par silences).

| Field       | Type     | Description                                          |
|-------------|----------|------------------------------------------------------|
| samples     | Vec<f32> | Donnees audio en f32 normalise [-1.0, 1.0] a 16kHz  |
| start_ms    | u64      | Position de debut du segment dans l'audio source     |
| end_ms      | u64      | Position de fin du segment dans l'audio source       |
| rms_energy  | f32      | Energie RMS du segment (pour filtrer le silence)     |

### SampleExpectation

Represente le resultat attendu pour un sample de test.

| Field              | Type                | Description                                      |
|--------------------|---------------------|--------------------------------------------------|
| name               | String              | Identifiant du sample (ex: "sample1")            |
| description        | String              | Description humaine de l'audio                   |
| expected_messages  | Vec<ExpectedMessage>| Liste ordonnee des messages attendus             |

### ExpectedMessage

Un message individuel attendu apres detection.

| Field      | Type   | Description                                              |
|------------|--------|----------------------------------------------------------|
| detected   | bool   | Le trigger word devrait-il etre detecte pour ce segment  |
| command    | String | Commande attendue apres extraction                       |

## State Transitions

### Speaker Detection State (existant, inchange)

```
Inactive ──[wake word detected]──> Active
Active ──[silence timeout 2s]──> Transcribing
Transcribing ──[transcription complete]──> Inactive
```

### Pipeline Processing (nouveau, pour les tests)

```
RawAudio ──[load WAV]──> AudioSegments ──[segment by silence]──> Segments[]
Segment ──[transcribe]──> TranscribedText ──[detect wake word]──> DetectionResult
DetectionResult ──[extract command]──> Command (si detecte)
```

## Relationships

```
SampleExpectation 1──*─ ExpectedMessage
AudioSegment *──1─ Pipeline (traite par)
DetectionResult 1──1─ AudioSegment (produit de)
```
