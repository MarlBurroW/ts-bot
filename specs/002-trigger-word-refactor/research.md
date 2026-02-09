# Research: Refonte du systeme de Trigger Word

**Date**: 2026-02-07
**Feature**: 002-trigger-word-refactor

## R1: Chargement de fichiers WAV pour les tests

**Decision**: Utiliser la crate `hound` (deja presente dans Cargo.toml) pour lire les fichiers WAV et les convertir en f32 16kHz mono.

**Rationale**: hound est deja une dependance du projet, supporte PCM 16/24/32 bits et float 32 bits. Elle gere la conversion mono/stereo et l'extraction des metadonnees (sample rate, channels). Pas besoin d'ajouter de nouvelle dependance.

**Alternatives considered**:
- `rodio`: Plus haut niveau mais beaucoup plus lourd, ajoute des dependances audio inutiles
- `symphonia`: Puissant mais surqualifie pour du simple WAV
- Lecture manuelle du format WAV: Fragile et reinvente la roue

## R2: Structure des resultats attendus pour les samples

**Decision**: Parser le fichier samples.md pour extraire les resultats attendus de maniere structuree. Format: chaque sample a un nom, une transcription humaine, et un ou plusieurs resultats attendus (detection oui/non + commande extraite).

**Rationale**: Le fichier samples.md existe deja et definit clairement les attentes. Le parser permet d'ajouter de nouveaux samples sans modifier le code de test. Un format structure (sections par sample) est suffisant.

**Alternatives considered**:
- Fichier JSON/TOML separe par sample: Plus rigide, duplication avec samples.md
- Annotations dans le code de test: Ne permet pas l'ajout dynamique de samples
- Fichier YAML: Ajouterait une dependance inutile

## R3: Architecture du pipeline testable

**Decision**: Creer un trait `AudioPipeline` avec une methode `process_audio(samples: &[f32]) -> Vec<DetectionResult>` qui encapsule transcription + detection + extraction. L'implementation concrete utilise WhisperTranscriber + WakeWordDetector.

**Rationale**: Un trait permet de mocker le pipeline dans les tests unitaires et de tester chaque composant isolement. La methode retourne un Vec pour supporter les multiples detections dans un meme flux (sample2).

**Alternatives considered**:
- Fonctions libres sans trait: Moins testable, pas de polymorphisme
- Pattern Builder: Sur-engineering pour ce cas d'usage
- Callbacks/channels: Complexite inutile pour un pipeline synchrone

## R4: Strategie de decoupage audio pour multi-detection

**Decision**: Pour les samples contenant plusieurs invocations (sample2), le pipeline doit segmenter l'audio aux pauses (silences > seuil) puis traiter chaque segment. La detection de silence utilise le RMS energy deja present.

**Rationale**: Le systeme actuel ne gere qu'une detection par buffer car il traite l'audio en streaming avec un timeout de silence de 2s. Pour les tests sur fichiers complets, il faut simuler ce comportement en decoupant aux silences.

**Alternatives considered**:
- Fenetre glissante: Plus complexe, risque de couper au milieu d'un mot
- Traitement du fichier entier en un bloc: Ne detecterait qu'une seule invocation
- Decoupage a intervalles fixes: Arbitraire, risque de couper la parole

## R5: Amelioration du matching de trigger word

**Decision**: Conserver l'approche substring matching sur texte normalise mais ameliorer la liste de variations. Ajouter un score de confiance base sur la distance de Levenshtein normalisee pour les cas limites. Seuil: distance <= 2 caracteres par rapport a "marlbot".

**Rationale**: Le matching actuel fonctionne bien quand Whisper transcrit correctement une variation connue. Le probleme est que Whisper produit parfois des variations non prevues. La distance de Levenshtein couvre ces cas sans ajouter de complexite significative. Pas besoin de crate externe, l'algorithme est simple a implementer.

**Alternatives considered**:
- Matching phonetique (Soundex/Metaphone): Concu pour l'anglais, mauvais pour le francais
- Embedding acoustique (word2vec): Surqualifie, necessite un modele supplementaire
- Regex avec patterns flexibles: Fragile et difficile a maintenir
- Modele acoustique dedie (OpenWakeWord, Porcupine): Necessite un modele entraine specifiquement pour "marlbot", complexite d'integration importante
