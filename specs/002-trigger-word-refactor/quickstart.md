# Quickstart: Refonte du systeme de Trigger Word

**Feature**: 002-trigger-word-refactor

## Prerequis

- Rust 1.75+ (stable)
- LIBCLANG_PATH configure (requis par whisper-rs-sys)
- CMAKE_GENERATOR="Visual Studio 17 2022"
- Modele Whisper: `models/ggml-small.bin`
- Fichiers samples WAV dans `samples/`

## Structure des changements

```text
src/
  audio/
    pipeline.rs        # NOUVEAU - Pipeline de detection testable
    wake_word.rs       # MODIFIE - Amelioration du matching
    whisper.rs         # MODIFIE - Interface testable
    wav_loader.rs      # NOUVEAU - Chargement WAV pour tests
    mod.rs             # MODIFIE - Exports
  main.rs              # MODIFIE - Utilise le pipeline
samples/
  samples.md           # MODIFIE - Format structure pour parsing
tests/
  trigger_word_tests.rs # NOUVEAU - Tests sur samples
```

## Commandes

### Lancer les tests sur les samples
```bash
cargo test --test trigger_word_tests
```

### Lancer tous les tests
```bash
cargo test
```

### Lancer clippy
```bash
cargo clippy
```

## Ordre d'implementation suggere

1. `wav_loader.rs` - Chargement et conversion WAV (testable isolement)
2. `pipeline.rs` - Pipeline de detection (trait + implementation)
3. Refactoring `wake_word.rs` - Amelioration du matching
4. Refactoring `whisper.rs` - Interface pipeline
5. `trigger_word_tests.rs` - Tests d'integration sur samples
6. Refactoring `main.rs` - Utilisation du pipeline
7. Validation sur les 4 samples

## Notes importantes

- Le build de whisper-rs est long (~2-3 min premiere fois) a cause de la compilation C++
- Les variables d'environnement LIBCLANG_PATH et CMAKE_GENERATOR doivent etre exportees dans chaque session shell
- Les tests sur samples necessitent le modele Whisper telecharge localement
