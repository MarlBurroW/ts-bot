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
Status: pending
Priority: high

La commande `!viens` échoue avec "❌ Impossible de trouver ton channel" alors que le bot a les droits pour voir tous les channels. Log TS :
```
<21:22:46> "marlburrow": !viens
<21:22:46> "Marlbot": ❌ Impossible de trouver ton channel.
```
Le bot devrait pouvoir résoudre le channel de l'utilisateur qui envoie la commande et s'y déplacer. Vérifier la logique de résolution du channel de l'invocateur (clientinfo ? channelid du sender ?).
