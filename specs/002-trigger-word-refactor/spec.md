# Feature Specification: Refonte du systeme de Trigger Word

**Feature Branch**: `002-trigger-word-refactor`
**Created**: 2026-02-07
**Status**: Draft
**Input**: User description: "Ameliore le systeme de trigger word car ca marche trop mal. Refactorisation pour avoir un systeme bien clean. Faire en sorte que le systeme de detection/transcription soit testable avec les samples (dossier samples), il y a la transcription dans le fichier samples.md, on peut donc voir le resultat attendu sur le declenchement et la transcription, ce qui devrait permettre d'affiner le systeme."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Test automatise de la detection avec des samples audio (Priority: P1)

Le developpeur lance une suite de tests automatises qui utilise les fichiers audio WAV du dossier `samples/` et les resultats attendus decrits dans `samples.md`. Chaque sample est traite par le pipeline de detection (transcription + matching du trigger word), et le resultat est compare aux attentes definies : le trigger word doit-il etre detecte ? Quelle commande doit etre extraite ?

Cela permet de mesurer objectivement la qualite de la detection, d'identifier les regressions, et d'iterer sur les parametres et algorithmes en toute confiance.

**Why this priority**: Sans testabilite, toute amelioration est faite a l'aveugle. Les tests sur samples reels sont la fondation qui rend possible toutes les autres ameliorations. C'est le prerequis pour pouvoir iterer efficacement.

**Independent Test**: Peut etre teste en executant la suite de tests avec les 4 samples existants. Chaque test verifie independamment : (1) si le trigger word est detecte quand il le devrait, (2) si la commande extraite correspond au resultat attendu, (3) si aucun faux positif ne se produit sur la parole sans trigger word.

**Acceptance Scenarios**:

1. **Given** le sample1 (audio contenant "Hey narlbot, connecte toi a home assistant et eteins toutes mes lumieres"), **When** le pipeline de detection traite cet audio, **Then** le trigger word est detecte et la commande extraite est "connecte toi a home assistant et eteins toutes mes lumieres"
2. **Given** le sample2 (audio contenant deux invocations du trigger word separees par une pause), **When** le pipeline traite cet audio, **Then** deux messages distincts sont produits : "comment ca va ? tu va bien ? moi ca va c'est cool" et "qu'est ce que tu fait de beau la ?"
3. **Given** le sample3 (audio contenant de la parole sans trigger word, puis le trigger word suivi d'une commande), **When** le pipeline traite cet audio, **Then** seule la partie apres le trigger word est capturee : "comment ca va ? qu'est ce que tu fait de beau ?"
4. **Given** le sample4 (audio contenant des prononciations erronees "Carlbot", "Marlmot", "Yarlbot", "Sarlbot" puis "marlbot"), **When** le pipeline traite cet audio, **Then** seul "Hey marlbot" declenche la detection, et la commande extraite est "comment ca va ?"
5. **Given** un test qui echoue suite a une modification du pipeline, **When** le developpeur consulte le rapport de test, **Then** il voit clairement quel sample a echoue, la transcription obtenue vs attendue, et si le trigger word a ete detecte ou non

---

### User Story 2 - Architecture modulaire et decouplage du pipeline (Priority: P2)

Le pipeline de detection du trigger word est actuellement entremele dans la boucle principale du bot. Le developpeur souhaite que le systeme de detection soit un composant autonome, avec des responsabilites clairement separees : transcription audio, detection du trigger word, extraction de la commande. Chaque composant peut etre teste et modifie independamment.

**Why this priority**: Un systeme decouple permet de modifier un composant (ex: ameliorer l'algorithme de matching) sans risquer de casser les autres. C'est aussi ce qui rend la User Story 1 (testabilite) possible de maniere elegante. Cependant, les tests sur samples sont plus urgents car ils permettent de valider l'etat actuel avant de refactorer.

**Independent Test**: Peut etre teste en verifiant que chaque composant du pipeline (transcription, detection, extraction) est utilisable et testable isolement, sans dependre de la connexion TeamSpeak ou de la boucle principale.

**Acceptance Scenarios**:

1. **Given** un composant de detection du trigger word, **When** on lui fournit un texte transcrit, **Then** il retourne si le trigger word est present et la commande associee, sans dependre d'aucun autre composant du bot
2. **Given** un composant de transcription, **When** on lui fournit un buffer audio brut (f32, 16kHz), **Then** il retourne le texte transcrit, sans dependre de la gestion des speakers ou de la connexion TS3
3. **Given** le pipeline complet assemble, **When** on lui fournit un fichier audio WAV, **Then** il produit le meme resultat que lorsqu'il recoit de l'audio en temps reel via TS3

---

### User Story 3 - Amelioration de la precision de detection (Priority: P3)

Le systeme actuel detecte mal le trigger word : beaucoup de faux negatifs (le bot ne reagit pas quand on l'appelle) et parfois des faux positifs (le bot reagit sur de la parole sans trigger word). Le developpeur souhaite que la detection soit plus fiable, en particulier pour les prononciations naturelles et variees du nom du bot.

**Why this priority**: L'amelioration de la precision est l'objectif final, mais elle ne peut etre mesuree et validee que grace aux tests automatises (P1) et au systeme decouple (P2). C'est le resultat rendu possible par les deux premieres stories.

**Independent Test**: Peut etre mesure en executant les tests sur les 4 samples et en verifiant que 100% des resultats attendus sont corrects. De nouveaux samples pourront etre ajoutes pour couvrir des cas supplementaires.

**Acceptance Scenarios**:

1. **Given** les 4 samples audio existants, **When** le pipeline de detection est execute, **Then** tous les resultats correspondent aux attentes definies dans samples.md
2. **Given** un utilisateur prononcant le nom du bot de maniere naturelle (variations phonetiques courantes), **When** le systeme analyse l'audio, **Then** le trigger word est detecte dans au moins 90% des cas
3. **Given** une conversation ne contenant pas le trigger word, **When** le systeme analyse l'audio, **Then** aucune fausse detection ne se produit

---

### Edge Cases

- Que se passe-t-il quand l'audio contient uniquement du bruit de fond sans aucune parole ?
- Que se passe-t-il quand le trigger word est prononce tres rapidement ou de maniere a peine audible ?
- Que se passe-t-il quand plusieurs personnes parlent en meme temps et que l'une prononce le trigger word ?
- Que se passe-t-il quand le trigger word est coupe entre deux buffers audio (prononce a la frontiere d'un segment) ?
- Que se passe-t-il quand l'utilisateur dit un mot phonetiquement tres proche du trigger word dans une phrase normale (ex: "il m'a rebute") ?
- Que se passe-t-il quand un fichier sample WAV est corrompu ou dans un format inattendu ?

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Le systeme DOIT fournir un moyen d'executer la detection de trigger word sur un fichier audio WAV de test, sans connexion TeamSpeak active
- **FR-002**: Le systeme DOIT comparer les resultats de detection aux resultats attendus definis dans le fichier samples.md (trigger word detecte oui/non, commande extraite)
- **FR-003**: Le systeme DOIT separer la logique de transcription audio de la logique de detection du trigger word, de sorte que chaque composant puisse etre teste isolement
- **FR-004**: Le systeme DOIT separer la logique de detection du trigger word de la boucle evenementielle principale du bot
- **FR-005**: Le systeme DOIT supporter la detection de multiples invocations du trigger word dans un meme flux audio (ex: deux commandes successives)
- **FR-006**: Le systeme DOIT ignorer la parole qui ne contient pas le trigger word (pas de faux positifs)
- **FR-007**: Le systeme DOIT tolerer les variations phonetiques courantes du nom du bot (prononciations approximatives, accents)
- **FR-008**: Le systeme DOIT detecter le trigger word meme quand il est precede de mots-cles d'invocation varies ("hey", "ok", "he", "eh")
- **FR-009**: Le systeme DOIT extraire uniquement la commande qui suit le trigger word, en excluant la parole anterieure
- **FR-010**: Le systeme DOIT rejeter les noms trop differents phonetiquement du nom du bot (ex: "Carlbot", "Yarlbot", "Sarlbot" ne doivent PAS declencher la detection)
- **FR-011**: Le systeme DOIT permettre l'ajout de nouveaux samples de test sans modification de code (convention de nommage et format standard)
- **FR-012**: Le systeme DOIT produire un rapport de test clair indiquant pour chaque sample : transcription obtenue, trigger word detecte (oui/non), commande extraite, et comparaison avec le resultat attendu

### Key Entities

- **Sample de test**: Un fichier audio WAV accompagne de son resultat attendu (transcription, detection du trigger word, commande extraite). Represente un cas de test reproductible.
- **Resultat de detection**: Le produit du pipeline de trigger word pour un segment audio donne : le trigger word a-t-il ete detecte, et si oui, quelle commande a ete extraite.
- **Pipeline de detection**: La chaine de traitement complette d'un segment audio, de l'audio brut jusqu'a la decision de detection et l'extraction de commande.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 100% des 4 samples de test existants produisent les resultats attendus definis dans samples.md (detection correcte et commande extraite conforme)
- **SC-002**: Les tests automatises sur les samples peuvent etre executes en une seule commande, sans configuration manuelle ni connexion reseau
- **SC-003**: L'ajout d'un nouveau sample de test necessite uniquement l'ajout d'un fichier audio et de sa description dans samples.md, sans modification de code de test
- **SC-004**: Le taux de faux positifs (detection du trigger word quand il n'est pas prononce) est de 0% sur les samples de test
- **SC-005**: Le taux de faux negatifs (non-detection du trigger word quand il est prononce correctement) est inferieur a 10% sur les samples de test
- **SC-006**: Chaque composant du pipeline (transcription, detection, extraction) est testable isolement avec ses propres tests unitaires

## Assumptions

- Les fichiers WAV dans le dossier `samples/` sont au format PCM standard (mono ou stereo, convertible en mono 16kHz f32)
- Le fichier `samples.md` sert de source de verite pour les resultats attendus de chaque sample
- Le modele Whisper utilise (`ggml-small.bin`) est disponible localement pour les tests
- Les prononciations volontairement tres eloignees ("Carlbot", "Yarlbot", "Sarlbot") du sample4 sont considerees comme des negatifs attendus (ne doivent PAS declencher la detection)
- Le systeme continuera a utiliser une approche STT + matching textuel (et non un modele acoustique dedie au wake word)
