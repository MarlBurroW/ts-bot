# TS Bot Feedback Queue

Items here are picked up by the cron improvement agent. Format:

```
## [DATE] - TITLE
Status: pending | in-progress | done
Priority: high | medium | low

Description of the feedback / bug / feature request.
```

When an item is addressed, the cron agent marks it `done` and notes what was done.

---

<!-- Add feedback below this line -->

## 2026-02-10 - !viens ne fonctionne pas
Status: done
Priority: high
Note: Fixed in commit 2a1ee92. Root cause: `state.clients.get(&sender_cid)` returned None because the sender's ClientId wasn't in the state map (likely due to reconnect giving a new ClientId, or race between disconnect/message events). Added fallback: if ClientId lookup fails, search clients by name. Same fix applied to `!who`.

La commande `!viens` échoue avec "❌ Impossible de trouver ton channel" alors que le bot a les droits pour voir tous les channels. Log TS :
```
<21:22:46> "marlburrow": !viens
<21:22:46> "Marlbot": ❌ Impossible de trouver ton channel.
```
Le bot devrait pouvoir résoudre le channel de l'utilisateur qui envoie la commande et s'y déplacer. Vérifier la logique de résolution du channel de l'invocateur (clientinfo ? channelid du sender ?).

## 2026-02-10 - !viens TOUJOURS cassé après le fix
Status: done
Priority: high
Note: Fixed in commit 8fcca0c. Root cause was NOT the ClientId lookup — it was channel subscription. tsclientlib only sees clients in subscribed channels, and by default only the bot's own channel is subscribed. Added `channelsubscribeall` command sent 3s after connection, which makes ALL server clients visible. Now !viens, !who, !channels etc. all see every connected client regardless of channel.

Le fix par fallback nom (commit 2a1ee92) ne marche pas. Le problème réel : `state.clients` ne contient probablement PAS les clients qui étaient déjà connectés avant le bot. Seuls les clients qui join/move APRÈS la connexion du bot sont trackés.

Preuve : après restart à 20:36:46, le bot ne log que lui-même (Marlbot1 id:9710). marlburrow était déjà connecté mais n'apparaît nulle part dans les logs. Le name fallback échoue aussi car marlburrow n'est tout simplement pas dans `state.clients`.

**Diagnostic step** : Ajoute un log INFO dans le handler `!viens` qui dump `state.clients.len()` et les noms+ids des clients connus. Ça confirmera que la map est incomplète.

**Fix probable** : `tsclientlib` devrait normalement peupler `state.clients` avec la liste initiale des clients au connect (c'est la lib qui gère l'état). Si ce n'est pas le cas, il y a peut-être un bug dans comment on utilise la lib, ou il faut attendre que le state sync soit complet avant de servir des commandes. Vérifier aussi si `con.get_state()` retourne bien un snapshot complet ou partiel.

## 2026-02-11 - Changement de voix TTS ne fonctionne pas
Status: done
Priority: medium
Note: Root cause: the OpenClaw plugin was sending `voice: "onyx"` (from static config) on every `speak` command, overriding the bot's runtime default set by `set_voice`. Fix: removed the voice override from the plugin's `deliverReply()` — now `speak` commands don't specify a voice, letting the bot use its runtime default (changeable via `teamspeak_set_voice` tool or `!voice` chat command). The bot already had `set_voice`/`get_voice` WS commands and `!voice` chat command working correctly — only the plugin was bypassing them.

## 2026-02-12 - Commande !speed pour changer la vitesse TTS par défaut
Status: done
Priority: medium
Note: Already implemented! `!speed` command exists in main.rs (line ~1553), persists to `bot_state.json`, validates range 0.25-4.0, shows current value without args, and is used as default fallback for all TTS output. Also available via WS API (`set_speed`/`get_speed`). In help text and tested.

Ajouter une commande `!speed <valeur>` (ex: `!speed 1.0`, `!speed 1.3`) pour changer la vitesse TTS par défaut, similaire à `!voice`. Actuellement la vitesse est hardcodée à 1.15x. La valeur doit être persistée dans `bot_state.json` comme pour voice/volume/muted, et utilisée comme fallback quand aucun `speed:X` n'est spécifié dans `!tts`. Range valide : 0.25 à 4.0.

## 2026-02-12 - Valider les voix selon le modèle TTS
Status: done
Priority: low
Note: Implemented in commit 20830ca. Added `valid_voices_for_model()` utility — tts-1/tts-1-hd accept only 6 classic voices (alloy, echo, fable, nova, onyx, shimmer), gpt-4o-mini-tts accepts all 11. Validation applied in 3 places: `!voice` command, `!tts voice:X` prefix, and WS `set_voice` handler. Invalid voices now get a clear error with the list of valid options for the configured model. 3 unit tests added.

Les voix ash, ballad, coral, sage, verse ne sont supportées que par `gpt-4o-mini-tts`, pas par `tts-1`. Le bot les accepte dans `!voice` mais l'API plante ensuite. Soit filtrer les voix invalides selon le modèle configuré, soit passer au modèle `gpt-4o-mini-tts`.

## Item — Voxtral Study
- **Date:** 2026-02-12
- **Priority:** high
- **Status:** done
- **Description:** Étude de faisabilité : remplacer l'API OpenAI Whisper par Voxtral Transcribe 2 (Mistral)
- **Details:**
  - Voxtral Transcribe 2 : modèle open source (Apache 2.0), 4B params, bat Whisper sur les benchmarks
  - Actuellement le bot utilise l'API OpenAI Whisper pour le STT (~1.2s latence)
  - Questions à étudier :
    1. Peut-on faire tourner Voxtral en local sur la machine host (specs CPU/RAM/GPU dispo) ?
    2. Si pas assez de ressources locales, est-ce que Mistral a une API hébergée pour Voxtral ?
    3. Comparaison latence/qualité/coût vs OpenAI Whisper API actuel
    4. Format d'entrée audio supporté (opus/pcm/wav ?) — compatibilité avec le pipeline actuel
    5. Qualité de transcription FR vs EN comparée à Whisper
  - Ne PAS implémenter le changement — juste produire un rapport avec recommandation
  - Écrire le rapport dans `~/ts-bot/VOXTRAL_STUDY.md`
- **Note:** Report written in `~/ts-bot/VOXTRAL_STUDY.md`. TL;DR: self-hosting impossible (no GPU), hosted API is 50% cheaper for batch but same price for realtime. Recommendation: keep Whisper for now — cost savings are negligible for our volume, Voxtral API is brand new. Revisit Q2 2026.
