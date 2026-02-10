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
