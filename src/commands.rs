//! Chat command response generators.
//!
//! Pure functions that take command arguments and return response strings.
//! Extracted from main.rs to reduce its size and improve testability.

use rand::Rng;

use std::collections::HashMap;

use crate::models::{ActivePoll, BotStats, Reminder};
use crate::persistence::{load_json, save_json};
use crate::utils::{format_duration_ms, format_uptime, parse_duration_str, truncate_str};

/// Returns the help text listing all available commands.
pub fn help_text() -> String {
    "📋 Commandes disponibles :\n\
     • [b]!listen[/b] / [b]!marlbot[/b] — activer l'écoute vocale\n\
     • [b]!stop[/b] — arrêter l'écoute + couper la parole\n\
     • [b]!lang[/b] <code> — forcer la langue (fr, en, de...) ou [b]!lang auto[/b]\n\
     • [b]!who[/b] — qui est dans ton channel ?\n\
     • [b]!find[/b] <nom> — trouver un utilisateur sur le serveur\n\
     • [b]!channels[/b] — lister tous les channels du serveur\n\
     • [b]!tts[/b] [voice:X] [speed:X] <texte> — TTS (voix: alloy/echo/fable/nova/onyx/shimmer...)\n\
     • [b]!move[/b] <channel> — déplacer le bot vers un channel\n\
     • [b]!come[/b] / [b]!viens[/b] — le bot vient dans ton channel\n\
     • [b]!replay[/b] — rejouer le dernier message TTS\n\
     • [b]!voice[/b] [nom] — changer la voix par défaut (alloy/echo/nova/onyx...)\n\
     • [b]!speed[/b] [valeur] — changer la vitesse TTS par défaut (0.25-4.0)\n\
     • [b]!volume[/b] [0-200] — régler le volume TTS (100 = normal)\n\
     • [b]!mute[/b] / [b]!unmute[/b] — couper/rétablir la voix (le bot écoute toujours)\n\
     • [b]!greet[/b] [on|off] — activer/désactiver les salutations auto\n\
     • [b]!timeout[/b] [ms] — régler le délai de silence (500-10000ms, défaut 2000)\n\
     • [b]!roll[/b] [NdS+M] — lancer des dés (ex: 2d6, d20+3, 100)\n\
     • [b]!8ball[/b] <question> — boule magique 🎱\n\
     • [b]!roulette[/b] — roulette russe 🔫 (1/6 chance de kick)\n\
     • [b]!duel[/b] <nom> — défier quelqu'un en duel (2d6, perdant = kick)\n\
     • [b]!quote[/b] [add|list|count|del] — livre de quotes mémorables\n\
     • [b]!history[/b] [N] — derniers messages (défaut 10, max 50)\n\
     • [b]!seen[/b] <nom> — quand un utilisateur a été vu pour la dernière fois\n\
     • [b]!notify[/b] [nom|clear] — être notifié (poke) quand quelqu'un se connecte\n\
     • [b]!afk[/b] <message> — se marquer AFK (auto-clear quand tu parles)\n\
     • [b]!poll[/b] Question | Opt1 | Opt2 — créer un sondage\n\
     • [b]!vote[/b] <n> — voter dans le sondage en cours\n\
     • [b]!remind[/b] <durée> <msg> — rappel (ex: !remind 30m Checker le four)\n\
     • [b]!ping[/b] — latence vers le serveur TS3\n\
     • [b]!stats[/b] — statistiques d'utilisation (messages, TTS, etc.)\n\
     • [b]!status[/b] — afficher l'état du bot\n\
     • [b]!help[/b] — afficher cette aide"
        .to_string()
}

/// Returns the ping response with uptime.
pub fn ping_response(uptime_secs: u64) -> String {
    let uptime_str = format_uptime(uptime_secs, true);
    format!("🏓 Pong ! (uptime: {})", uptime_str)
}

/// Returns the stats response.
pub fn stats_response(uptime_secs: u64, stats: &BotStats) -> String {
    let uptime_str = format_uptime(uptime_secs, false);
    let msgs = stats.messages_received.load(std::sync::atomic::Ordering::Relaxed);
    let cmds = stats.commands_executed.load(std::sync::atomic::Ordering::Relaxed);
    let tts = stats.tts_calls.load(std::sync::atomic::Ordering::Relaxed);
    let transcriptions = stats.voice_transcriptions.load(std::sync::atomic::Ordering::Relaxed);
    let greets = stats.greetings_sent.load(std::sync::atomic::Ordering::Relaxed);
    format!(
        "📊 Statistiques Marlbot\n\
         • Uptime session : {}\n\
         • Messages reçus : {}\n\
         • Commandes exécutées : {}\n\
         • Appels TTS : {}\n\
         • Transcriptions vocales : {}\n\
         • Salutations envoyées : {}",
        uptime_str, msgs, cmds, tts, transcriptions, greets
    )
}

/// Parse a dice roll command and return the result string.
///
/// Supports: bare number (`!roll 20`), NdS notation (`2d6`), modifiers (`1d20+3`).
pub fn roll_dice(args: &str) -> String {
    let dice_str = if args.trim().is_empty() { "1d6" } else { args.trim() };

    let result = (|| -> Result<String, String> {
        let s = dice_str.to_lowercase();

        // Simple number (e.g., !roll 20 = random 1-20)
        if let Ok(max) = s.parse::<i64>() {
            if !(1..=1000000).contains(&max) {
                return Err("Nombre entre 1 et 1000000 svp".to_string());
            }
            let val = rand::thread_rng().gen_range(1..=max);
            return Ok(format!("🎲 1-{} → [b]{}[/b]", max, val));
        }

        // Parse NdS[+/-M]
        let d_pos = s
            .find('d')
            .ok_or("Format: NdS, NdS+M, NdS-M (ex: 2d6, 1d20+3)")?;
        let count_str = &s[..d_pos];
        let count: u32 = if count_str.is_empty() {
            1
        } else {
            count_str.parse().map_err(|_| "Nombre de dés invalide")?
        };
        if !(1..=100).contains(&count) {
            return Err("1 à 100 dés max".to_string());
        }

        let rest = &s[d_pos + 1..];
        let (sides_str, modifier) = if let Some(pos) = rest.find('+') {
            (
                &rest[..pos],
                rest[pos + 1..]
                    .parse::<i64>()
                    .map_err(|_| "Modificateur invalide")?,
            )
        } else if let Some(pos) = rest[1..].find('-') {
            let pos = pos + 1;
            (
                &rest[..pos],
                -(rest[pos + 1..]
                    .parse::<i64>()
                    .map_err(|_| "Modificateur invalide")?),
            )
        } else {
            (rest, 0i64)
        };
        let sides: u32 = sides_str.parse().map_err(|_| "Nombre de faces invalide")?;
        if !(2..=1000).contains(&sides) {
            return Err("2 à 1000 faces".to_string());
        }

        let mut rng = rand::thread_rng();
        let rolls: Vec<u32> = (0..count).map(|_| rng.gen_range(1..=sides)).collect();
        let sum: i64 = rolls.iter().map(|&r| r as i64).sum::<i64>() + modifier;

        if count == 1 && modifier == 0 {
            Ok(format!("🎲 d{} → [b]{}[/b]", sides, rolls[0]))
        } else if count <= 20 {
            let details: Vec<String> = rolls.iter().map(|r| r.to_string()).collect();
            let mod_str = if modifier > 0 {
                format!("+{}", modifier)
            } else if modifier < 0 {
                format!("{}", modifier)
            } else {
                String::new()
            };
            Ok(format!(
                "🎲 {}d{}{} → ({}) = [b]{}[/b]",
                count,
                sides,
                mod_str,
                details.join("+"),
                sum
            ))
        } else {
            let mod_str = if modifier > 0 {
                format!("+{}", modifier)
            } else if modifier < 0 {
                format!("{}", modifier)
            } else {
                String::new()
            };
            Ok(format!("🎲 {}d{}{} → [b]{}[/b]", count, sides, mod_str, sum))
        }
    })();

    match result {
        Ok(s) => s,
        Err(e) => format!("❌ {}", e),
    }
}

/// Magic 8-ball response.
///
/// Returns `None` if the question is empty (caller should show usage hint).
pub fn eight_ball(sender_name: &str, question: &str) -> Option<String> {
    if question.is_empty() {
        return None;
    }
    let answers = [
        // Positives (8)
        "🟢 Oui, absolument.",
        "🟢 C'est certain.",
        "🟢 Sans aucun doute.",
        "🟢 Oui, définitivement.",
        "🟢 Tu peux compter dessus.",
        "🟢 Les signes disent oui.",
        "🟢 Très probablement.",
        "🟢 Les astres sont favorables.",
        // Neutral (4)
        "🟡 Réponse floue, repose ta question.",
        "🟡 Demande plus tard.",
        "🟡 Mieux vaut ne pas te dire maintenant.",
        "🟡 Je ne peux pas prédire ça.",
        // Negatives (8)
        "🔴 N'y compte pas.",
        "🔴 Ma réponse est non.",
        "🔴 Mes sources disent non.",
        "🔴 Les perspectives ne sont pas bonnes.",
        "🔴 Très douteux.",
        "🔴 Non.",
        "🔴 Clairement pas.",
        "🔴 Absolument pas.",
    ];
    let idx = rand::thread_rng().gen_range(0..answers.len());
    Some(format!(
        "🎱 {} demande : \"{}\"\n{}",
        sender_name,
        truncate_str(question, 150),
        answers[idx]
    ))
}

/// Quote book command result.
pub enum QuoteAction {
    /// A response message to send back.
    Response(String),
}

/// Handle the `!quote` command with subcommands: add, list, count, del, or random.
///
/// Loads/saves quotes from `quotes_path` (JSON array of objects with text/author/date).
pub fn quote_command(args: &str, sender_name: &str, quotes_path: &str) -> QuoteAction {
    let mut quotes: Vec<serde_json::Value> = load_json(quotes_path);

    let response = if args.starts_with("add ") || args.starts_with("add\t") {
        let quote_text = args[4..].trim();
        if quote_text.is_empty() {
            "❌ Usage: !quote add <texte>".to_string()
        } else if quote_text.len() > 500 {
            "❌ Quote trop longue (max 500 caractères)".to_string()
        } else {
            let entry = serde_json::json!({
                "text": quote_text,
                "author": sender_name,
                "date": chrono::Utc::now().format("%Y-%m-%d %H:%M").to_string(),
            });
            quotes.push(entry);
            save_json(quotes_path, &quotes);
            format!("💬 Quote #{} sauvegardée !", quotes.len())
        }
    } else if args == "list" {
        if quotes.is_empty() {
            "📖 Aucune quote sauvegardée. Utilise [b]!quote add <texte>[/b]".to_string()
        } else {
            let start = if quotes.len() > 5 { quotes.len() - 5 } else { 0 };
            let mut lines = vec![format!("📖 Dernières quotes ({}/{}) :", quotes.len() - start, quotes.len())];
            for (i, q) in quotes[start..].iter().enumerate() {
                let num = start + i + 1;
                let text = q.get("text").and_then(|v| v.as_str()).unwrap_or("?");
                let author = q.get("author").and_then(|v| v.as_str()).unwrap_or("?");
                lines.push(format!("#{} — \"{}\" — {}", num, text, author));
            }
            lines.join("\n")
        }
    } else if args == "count" {
        format!("📖 {} quote(s) sauvegardée(s)", quotes.len())
    } else if args.starts_with("del ") || args.starts_with("delete ") {
        let num_str = args.split_whitespace().nth(1).unwrap_or("");
        if let Ok(num) = num_str.parse::<usize>() {
            if num >= 1 && num <= quotes.len() {
                let removed = quotes.remove(num - 1);
                save_json(quotes_path, &quotes);
                let text = removed.get("text").and_then(|v| v.as_str()).unwrap_or("?");
                format!("🗑️ Quote #{} supprimée : \"{}\"", num, text)
            } else {
                format!("❌ Numéro invalide (1-{})", quotes.len())
            }
        } else {
            "❌ Usage: !quote del <numéro>".to_string()
        }
    } else if args.is_empty() {
        // Random quote
        if quotes.is_empty() {
            "📖 Aucune quote sauvegardée. Utilise [b]!quote add <texte>[/b]".to_string()
        } else {
            let idx = rand::thread_rng().gen_range(0..quotes.len());
            let q = &quotes[idx];
            let text = q.get("text").and_then(|v| v.as_str()).unwrap_or("?");
            let author = q.get("author").and_then(|v| v.as_str()).unwrap_or("?");
            let date = q.get("date").and_then(|v| v.as_str()).unwrap_or("");
            format!("💬 #{}/{} — \"{}\" — {} ({})", idx + 1, quotes.len(), text, author, date)
        }
    } else {
        "❌ Usage: !quote [add <texte>|list|count|del <n>]".to_string()
    };

    QuoteAction::Response(response)
}

/// Handle the `!seen` command — search for last disconnect time of a user.
///
/// `seen_data` maps UID → (name, timestamp_string).
/// If `query` is empty, returns a count summary. Otherwise searches by partial name match.
pub fn seen_response(
    seen_data: &std::collections::HashMap<String, (String, String)>,
    query: &str,
) -> String {
    if query.is_empty() {
        return format!(
            "👁️ {} utilisateur(s) trackés — !seen <nom> pour chercher",
            seen_data.len()
        );
    }
    let query_lower = query.to_lowercase();
    let matches: Vec<_> = seen_data
        .values()
        .filter(|(name, _)| name.to_lowercase().contains(&query_lower))
        .collect();
    if matches.is_empty() {
        format!("❌ Aucun résultat pour \"{}\"", query)
    } else if matches.len() == 1 {
        let (name, ts) = &matches[0];
        format!("👁️ {} — dernière déconnexion : {}", name, ts)
    } else {
        let mut lines = vec![format!(
            "👁️ {} résultats pour \"{}\" :",
            matches.len(),
            query
        )];
        for (name, ts) in matches.iter().take(5) {
            lines.push(format!("• {} — {}", name, ts));
        }
        lines.join("\n")
    }
}

/// Handle the `!history` command — return formatted recent chat history.
///
/// Returns `None` if history is empty, `Some(formatted)` otherwise.
pub fn history_response(
    history: &std::collections::VecDeque<(String, String, String)>,
    args: &str,
) -> Option<String> {
    let count: usize = args.parse().unwrap_or(20).clamp(1, 50);

    if history.is_empty() {
        return None;
    }

    let start = if history.len() > count { history.len() - count } else { 0 };
    let mut lines = vec![format!("📜 Derniers {} message(s) :", history.len() - start)];
    for (ts, author, text) in history.iter().skip(start) {
        let truncated = if text.len() > 100 {
            format!("{}...", truncate_str(text, 100))
        } else {
            text.clone()
        };
        lines.push(format!("[{}] {} : {}", ts, author, truncated));
    }
    Some(lines.join("\n"))
}

// ---------------------------------------------------------------------------
// Notify
// ---------------------------------------------------------------------------

/// Type alias for the notify watchers map: target_name_lower → Vec<(watcher_name, watcher_uid)>
pub type NotifyWatchersMap = std::collections::HashMap<String, Vec<(String, String)>>;

/// Result of a `!notify` command.
pub enum NotifyResult {
    /// Response message + whether watchers were mutated (needs save).
    Response { message: String, changed: bool },
}

/// Handle the `!notify` command. Mutates `watchers` in place and returns the
/// response message plus whether persistence is needed.
pub fn notify_command(
    watchers: &mut NotifyWatchersMap,
    arg: &str,
    sender_uid: &str,
    sender_name: &str,
) -> NotifyResult {
    if arg.is_empty() {
        // Show current watches for this user
        let my_watches: Vec<String> = watchers
            .iter()
            .filter(|(_, v)| v.iter().any(|(_, uid)| uid == sender_uid))
            .map(|(target, _)| target.clone())
            .collect();
        if my_watches.is_empty() {
            NotifyResult::Response {
                message: "🔔 Aucune notification active.\n!notify <nom> — être notifié quand quelqu'un se connecte\n!notify clear — tout supprimer".to_string(),
                changed: false,
            }
        } else {
            let list = my_watches
                .iter()
                .map(|n| format!("• {}", n))
                .collect::<Vec<_>>()
                .join("\n");
            NotifyResult::Response {
                message: format!(
                    "🔔 Tes notifications actives :\n{}\n!notify clear pour tout supprimer",
                    list
                ),
                changed: false,
            }
        }
    } else if arg.eq_ignore_ascii_case("clear") {
        let mut removed = 0;
        watchers.retain(|_, v| {
            let before = v.len();
            v.retain(|(_, uid)| uid != sender_uid);
            removed += before - v.len();
            !v.is_empty()
        });
        NotifyResult::Response {
            message: format!("🔕 {} notification(s) supprimée(s)", removed),
            changed: removed > 0,
        }
    } else {
        let target_lower = arg.to_lowercase();
        let entry = watchers.entry(target_lower.clone()).or_default();
        if entry.iter().any(|(_, uid)| uid == sender_uid) {
            // Toggle off
            entry.retain(|(_, uid)| uid != sender_uid);
            if entry.is_empty() {
                watchers.remove(&target_lower);
            }
            NotifyResult::Response {
                message: format!("🔕 Notification pour \"{}\" désactivée", arg),
                changed: true,
            }
        } else {
            entry.push((sender_name.to_string(), sender_uid.to_string()));
            NotifyResult::Response {
                message: format!(
                    "🔔 Tu seras notifié quand \"{}\" se connecte ! (!notify {} pour annuler)",
                    arg, arg
                ),
                changed: true,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Poll / Vote
// ---------------------------------------------------------------------------

/// Result of showing a poll.
#[derive(Debug, PartialEq)]
pub enum PollResponse {
    /// No poll active — show help text.
    Help,
    /// Formatted poll display message.
    Message(String),
}

/// Show the current poll status, or help if no poll is active.
pub fn poll_show(poll: Option<&ActivePoll>) -> PollResponse {
    match poll {
        None => PollResponse::Help,
        Some(p) => {
            let total_votes: usize = p.votes.iter().map(|v| v.len()).sum();
            let mut lines = vec![format!(
                "📊 [b]{}[/b] (par {}, {} vote{})",
                p.question,
                p.creator,
                total_votes,
                if total_votes != 1 { "s" } else { "" }
            )];
            for (i, opt) in p.options.iter().enumerate() {
                let count = p.votes[i].len();
                let bar = "█".repeat(count.min(10));
                lines.push(format!("  [b]{}.[/b] {} {} ({})", i + 1, opt, bar, count));
            }
            lines.push("Vote : [b]!vote <n>[/b] — Fin : [b]!poll end[/b]".to_string());
            PollResponse::Message(lines.join("\n"))
        }
    }
}

/// End a poll and return the results message with winner(s) marked.
pub fn poll_end(poll: &ActivePoll) -> String {
    let total_votes: usize = poll.votes.iter().map(|v| v.len()).sum();
    let mut lines = vec![format!(
        "🏁 Sondage terminé : [b]{}[/b] ({} vote{})",
        poll.question,
        total_votes,
        if total_votes != 1 { "s" } else { "" }
    )];
    let max_votes = poll.votes.iter().map(|v| v.len()).max().unwrap_or(0);
    for (i, opt) in poll.options.iter().enumerate() {
        let count = poll.votes[i].len();
        let bar = "█".repeat(count.min(10));
        let winner = if count == max_votes && max_votes > 0 {
            " 👑"
        } else {
            ""
        };
        lines.push(format!(
            "  [b]{}.[/b] {} {} ({}){}", i + 1, opt, bar, count, winner
        ));
    }
    lines.join("\n")
}

/// Try to create a poll from a pipe-separated string. Returns `None` with error
/// message if validation fails, or `Some((poll, announcement))` on success.
pub fn poll_create(arg: &str, creator: &str) -> Option<(ActivePoll, String)> {
    let parts: Vec<&str> = arg.split('|').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if parts.len() < 3 {
        return None;
    }
    if parts.len() > 11 {
        return None;
    }
    let question = truncate_str(parts[0], 200).to_string();
    let options: Vec<String> = parts[1..].iter().map(|s| truncate_str(s, 100).to_string()).collect();
    let num_options = options.len();
    let poll = ActivePoll {
        question: question.clone(),
        options: options.clone(),
        votes: vec![std::collections::HashSet::new(); num_options],
        creator: creator.to_string(),
    };
    let mut lines = vec![format!(
        "📊 Nouveau sondage par [b]{}[/b] : [b]{}[/b]",
        creator, question
    )];
    for (i, opt) in options.iter().enumerate() {
        lines.push(format!("  [b]{}.[/b] {}", i + 1, opt));
    }
    lines.push("Vote avec [b]!vote <n>[/b]".to_string());
    Some((poll, lines.join("\n")))
}

/// Result of a vote attempt.
#[derive(Debug)]
pub enum VoteResult {
    /// Successfully voted — message to broadcast to channel.
    Voted(String),
    /// Error — message to reply to the voter.
    Error(String),
}

/// Process a vote on a poll. Handles vote change detection.
pub fn vote(poll: &mut ActivePoll, arg: &str, voter_uid: &str, voter_name: &str) -> VoteResult {
    if let Ok(n) = arg.parse::<usize>() {
        if n >= 1 && n <= poll.options.len() {
            let mut changed_from: Option<usize> = None;
            for (i, votes) in poll.votes.iter_mut().enumerate() {
                if votes.remove(voter_uid) {
                    changed_from = Some(i + 1);
                }
            }
            poll.votes[n - 1].insert(voter_uid.to_string());
            let msg = if let Some(old) = changed_from {
                if old == n {
                    format!(
                        "✅ {} a voté pour [b]{}. {}[/b]",
                        voter_name, n, poll.options[n - 1]
                    )
                } else {
                    format!(
                        "🔄 {} a changé son vote : {} → [b]{}. {}[/b]",
                        voter_name, old, n, poll.options[n - 1]
                    )
                }
            } else {
                format!(
                    "✅ {} a voté pour [b]{}. {}[/b]",
                    voter_name, n, poll.options[n - 1]
                )
            };
            VoteResult::Voted(msg)
        } else {
            VoteResult::Error(format!("❌ Choisis entre 1 et {}", poll.options.len()))
        }
    } else {
        VoteResult::Error("❌ Usage : [b]!vote <numéro>[/b]".to_string())
    }
}

// ---------------------------------------------------------------------------
// !remind / !rappel
// ---------------------------------------------------------------------------

/// Result of a `!remind` command.
#[derive(Debug)]
pub enum RemindResult {
    /// Simple text response (list, error, usage).
    Response(String),
    /// A new reminder should be added to the list and persisted.
    Add { message: String, reminder: Reminder },
    /// Clear user's reminders — caller should retain only non-matching and persist.
    Clear { message: String, removed: usize },
}

/// Pure handler for `!remind` / `!rappel`.
///
/// `reminders` is the current list (read-only), `now_ms` is the current epoch ms.
/// Returns a `RemindResult` that the caller uses to mutate state + send response.
pub fn remind_command(
    reminders: &[Reminder],
    arg: &str,
    uid: &str,
    name: &str,
    now_ms: u64,
) -> RemindResult {
    if arg.is_empty() || arg == "list" {
        let mine: Vec<&Reminder> = reminders.iter().filter(|r| r.uid == uid).collect();
        if mine.is_empty() {
            return RemindResult::Response(
                "⏰ Aucun rappel en cours.\nUsage : [b]!remind <durée> <message>[/b]\nEx: !remind 30m Checker le four".to_string(),
            );
        }
        let mut lines = vec![format!(
            "⏰ {} rappel{} en cours :",
            mine.len(),
            if mine.len() > 1 { "s" } else { "" }
        )];
        for (i, r) in mine.iter().enumerate() {
            let remaining = if r.due_ms > now_ms {
                format_duration_ms(r.due_ms - now_ms)
            } else {
                "imminent".to_string()
            };
            lines.push(format!("  [b]{}.[/b] dans {} — {}", i + 1, remaining, r.message));
        }
        return RemindResult::Response(lines.join("\n"));
    }

    if arg == "clear" || arg == "annuler" {
        let removed = reminders.iter().filter(|r| r.uid == uid).count();
        let msg = if removed > 0 {
            format!(
                "🗑️ {} rappel{} supprimé{}",
                removed,
                if removed > 1 { "s" } else { "" },
                if removed > 1 { "s" } else { "" }
            )
        } else {
            "ℹ️ Aucun rappel à supprimer.".to_string()
        };
        return RemindResult::Clear { message: msg, removed };
    }

    // Parse: <duration> <message>
    let parts: Vec<&str> = arg.splitn(2, ' ').collect();
    let duration_str = parts[0];
    let msg_text = parts.get(1).unwrap_or(&"").trim();

    let Some(dur_ms) = parse_duration_str(duration_str) else {
        return RemindResult::Response(
            "❌ Durée invalide. Formats : 30s, 5m, 1h, 2h30m, 1d, 1j\nEx: [b]!remind 30m Checker le four[/b]".to_string(),
        );
    };

    if msg_text.is_empty() {
        return RemindResult::Response(
            "❌ Il faut un message ! Ex: [b]!remind 30m Checker le four[/b]".to_string(),
        );
    }
    if dur_ms < 10_000 {
        return RemindResult::Response("❌ Durée trop courte (minimum 10s).".to_string());
    }
    if dur_ms > 7 * 86400 * 1000 {
        return RemindResult::Response("❌ Durée trop longue (maximum 7 jours).".to_string());
    }

    let user_count = reminders.iter().filter(|r| r.uid == uid).count();
    if user_count >= 10 {
        return RemindResult::Response(
            "❌ Maximum 10 rappels actifs. Utilise [b]!remind clear[/b] pour nettoyer.".to_string(),
        );
    }

    let reminder = Reminder {
        due_ms: now_ms + dur_ms,
        uid: uid.to_string(),
        name: name.to_string(),
        message: msg_text.to_string(),
        created_ms: now_ms,
    };
    RemindResult::Add {
        message: format!("✅ Rappel dans [b]{}[/b] : {}", format_duration_ms(dur_ms), msg_text),
        reminder,
    }
}

// --- AFK command ---

/// Result of the `!afk` command.
pub enum AfkResult {
    /// User wants to set AFK with a message.
    Set { message: String, afk_entry: (String, String) },
    /// User wants to clear AFK (empty arg, "off", or "clear").
    /// `was_afk` is always false here — caller checks actual state.
    Clear { message_if_was_afk: String, message_if_not_afk: String, was_afk: bool },
}

/// Pure logic for `!afk <arg>`. Returns what to do; caller handles state mutation.
pub fn afk_command(arg: &str, _uid: &str, name: &str) -> AfkResult {
    let arg = arg.trim();
    if arg.is_empty() || arg == "off" || arg == "clear" {
        AfkResult::Clear {
            message_if_was_afk: "✅ Tu n'es plus AFK".to_string(),
            message_if_not_afk: "ℹ️ Tu n'es pas AFK. Usage : [b]!afk <message>[/b]".to_string(),
            was_afk: false, // placeholder — caller determines this
        }
    } else {
        let afk_msg = truncate_str(arg, 200).to_string();
        AfkResult::Set {
            message: format!("💤 AFK activé : [b]{}[/b] — tape !afk pour revenir", afk_msg),
            afk_entry: (name.to_string(), afk_msg),
        }
    }
}

/// Check if a non-command message mentions any AFK user.
/// Returns the first matching (name, afk_message) or None.
pub fn afk_check_mentions(
    message: &str,
    sender_uid: &str,
    afk_map: &HashMap<String, (String, String)>,
) -> Option<(String, String)> {
    let msg_lower = message.to_lowercase();
    for (afk_uid, (afk_name, afk_msg)) in afk_map.iter() {
        if afk_uid == sender_uid { continue; }
        if msg_lower.contains(&afk_name.to_lowercase()) {
            return Some((afk_name.clone(), afk_msg.clone()));
        }
    }
    None
}

// --- Lang command ---

/// Result of the `!lang` command.
pub enum LangResult {
    /// Reset to auto-detection.
    Reset { message: String },
    /// Set a specific language.
    Set { message: String, code: String },
    /// Invalid language code.
    Invalid { message: String },
}

const VALID_LANGS: &[&str] = &[
    "fr", "en", "de", "es", "it", "pt", "nl", "ru", "ja", "ko", "zh", "ar",
    "pl", "cs", "sv", "da", "fi", "no", "tr", "uk", "ro", "hu", "el", "he",
    "th", "vi", "id", "ms", "hi", "bn",
];

/// Pure logic for `!lang <code>`. Returns what to do; caller handles state mutation.
pub fn lang_command(arg: &str) -> LangResult {
    let arg = arg.trim().to_lowercase();
    if arg.is_empty() || arg == "auto" {
        LangResult::Reset {
            message: "🌍 Langue : auto-détection".to_string(),
        }
    } else if VALID_LANGS.contains(&arg.as_str()) {
        LangResult::Set {
            message: format!("🌍 Langue forcée : [b]{}[/b]", arg),
            code: arg,
        }
    } else {
        LangResult::Invalid {
            message: format!("❌ Langue inconnue : {}. Ex: !lang fr, !lang en, !lang auto", arg),
        }
    }
}

// --- !greet command ---

/// Result of processing a `!greet` command.
pub enum GreetResult {
    /// Set greeting enabled/disabled state.
    SetEnabled { message: String, enabled: bool },
    /// Show current greet status.
    Status(String),
    /// Invalid argument.
    Invalid(String),
}

/// Process the `!greet` command. `arg` is the part after `!greet ` (lowercased).
/// `current_enabled` is the current greet state.
pub fn greet_command(arg: &str, current_enabled: bool) -> GreetResult {
    match arg.trim() {
        "on" => GreetResult::SetEnabled {
            message: "👋 Greetings activés — je saluerai les arrivants !".to_string(),
            enabled: true,
        },
        "off" => GreetResult::SetEnabled {
            message: "🔕 Greetings désactivés.".to_string(),
            enabled: false,
        },
        "" => {
            let status = if current_enabled { "activés ✅" } else { "désactivés ❌" };
            GreetResult::Status(format!(
                "👋 Greetings : {} — !greet on|off pour changer",
                status
            ))
        }
        _ => GreetResult::Invalid("❌ Usage: !greet on|off".to_string()),
    }
}

// --- !timeout command ---

/// Result of processing a `!timeout` command.
pub enum TimeoutResult {
    /// Set timeout to a new value (clamped to 500-10000).
    Set { message: String, value_ms: u64 },
    /// Show current timeout value.
    Show(String),
    /// Invalid argument.
    Invalid(String),
}

/// Process the `!timeout` command. `arg` is the part after `!timeout `.
/// `current_ms` is the current silence timeout.
pub fn timeout_command(arg: &str, current_ms: u64) -> TimeoutResult {
    let arg = arg.trim();
    if arg.is_empty() {
        return TimeoutResult::Show(format!(
            "⏱️ Silence timeout : {}ms — !timeout <ms> pour changer (500-10000)",
            current_ms
        ));
    }
    match arg.parse::<u64>() {
        Ok(ms) => {
            let clamped = ms.clamp(500, 10000);
            TimeoutResult::Set {
                message: format!("⏱️ Silence timeout : {}ms", clamped),
                value_ms: clamped,
            }
        }
        Err(_) => TimeoutResult::Invalid("❌ Usage: !timeout <ms> (500-10000)".to_string()),
    }
}

// --- Volume, Voice, Speed commands ---

/// Result of `!volume` command.
#[derive(Debug, PartialEq)]
pub enum VolumeResult {
    /// Show current volume.
    Show(String),
    /// Set volume to this value + response message.
    Set { message: String, value: u8 },
    /// Invalid input.
    Invalid(String),
}

/// Pure logic for `!volume [0-200]`.
/// `current_vol`: current volume percentage.
pub fn volume_command(arg: &str, current_vol: u8) -> VolumeResult {
    let arg = arg.trim();
    if arg.is_empty() {
        return VolumeResult::Show(format!("🔊 Volume actuel : {}%", current_vol));
    }
    match arg.trim_end_matches('%').parse::<u8>() {
        Ok(vol) if vol > 200 => VolumeResult::Invalid("❌ Volume entre 0 et 200 (100 = normal)".to_string()),
        Ok(vol) => {
            let emoji = if vol == 0 { "🔇" } else if vol < 50 { "🔈" } else if vol <= 100 { "🔉" } else { "🔊" };
            VolumeResult::Set {
                message: format!("{} Volume réglé à {}%", emoji, vol),
                value: vol,
            }
        }
        Err(_) => VolumeResult::Invalid("❌ Usage: !volume [0-200]".to_string()),
    }
}

/// Result of `!voice` command.
#[derive(Debug, PartialEq)]
pub enum VoiceResult {
    /// Show current voice + available voices.
    Show(String),
    /// Set voice to this value + response message.
    Set { message: String, voice: String },
    /// Invalid voice name.
    Invalid(String),
}

/// Pure logic for `!voice [name]`.
/// `current_voice`: current default voice, `valid_voices`: list of valid voice names.
pub fn voice_command(arg: &str, current_voice: &str, valid_voices: &[String]) -> VoiceResult {
    let arg = arg.trim();
    if arg.is_empty() {
        let voices_str = valid_voices.join(", ");
        return VoiceResult::Show(format!(
            "🎙️ Voix par défaut : [b]{}[/b]\nVoix disponibles : {}",
            current_voice, voices_str
        ));
    }
    let requested = arg.split_whitespace().next().unwrap_or("").to_lowercase();
    if valid_voices.contains(&requested) {
        VoiceResult::Set {
            message: format!("🎙️ Voix par défaut changée en [b]{}[/b]", requested),
            voice: requested,
        }
    } else {
        let voices_str = valid_voices.join(", ");
        VoiceResult::Invalid(format!(
            "❌ Voix inconnue : \"{}\"\nVoix disponibles : {}",
            arg.split_whitespace().next().unwrap_or(arg),
            voices_str
        ))
    }
}

/// Result of `!speed` command.
#[derive(Debug, PartialEq)]
pub enum SpeedResult {
    /// Show current speed.
    Show(String),
    /// Set speed to this value + response message.
    Set { message: String, value: f32 },
    /// Invalid input.
    Invalid(String),
}

/// Pure logic for `!speed [0.25-4.0]`.
/// `current_speed`: current TTS speed.
pub fn speed_command(arg: &str, current_speed: f32) -> SpeedResult {
    let arg = arg.trim();
    if arg.is_empty() {
        return SpeedResult::Show(format!(
            "🏎️ Vitesse TTS par défaut : [b]{:.2}x[/b]\nRange : 0.25 — 4.0 (1.0 = normal)",
            current_speed
        ));
    }
    match arg.split_whitespace().next().unwrap_or("").parse::<f32>() {
        Ok(s) if (0.25..=4.0).contains(&s) => SpeedResult::Set {
            message: format!("🏎️ Vitesse TTS changée en [b]{:.2}x[/b]", s),
            value: s,
        },
        _ => SpeedResult::Invalid(
            "❌ Vitesse invalide. Range : 0.25 — 4.0 (ex: !speed 1.0, !speed 1.3)".to_string(),
        ),
    }
}

// --- !roulette ---

pub enum RouletteResult {
    /// Player dies — message + they should be kicked
    Bang(String),
    /// Player survives — message only
    Survived(String),
}

/// Russian roulette: 1/6 chance of death. Returns the message and whether the player should be kicked.
pub fn roulette_command(name: &str) -> RouletteResult {
    use rand::Rng;
    let chamber = rand::thread_rng().gen_range(1..=6);
    if chamber == 1 {
        RouletteResult::Bang(format!(
            "🔫 {} appuie sur la gâchette...\n💀 BANG ! {} est mort(e) !",
            name, name
        ))
    } else {
        let messages = [
            format!(
                "🔫 {} appuie sur la gâchette...\n😮💨 *click* — Pas cette fois ! ({}/6 chances de survie)",
                name,
                6 - 1
            ),
            format!(
                "🔫 {} tente sa chance...\n😎 Le barillet était vide. Tu vis encore.",
                name
            ),
            format!("🔫 *click*\n🍀 {} a de la chance... pour l'instant.", name),
        ];
        let idx = rand::thread_rng().gen_range(0..messages.len());
        RouletteResult::Survived(messages[idx].clone())
    }
}

// --- !duel ---

pub enum DuelAcceptResult {
    NotYourDuel,
    Expired,
    /// (message, loser_clid) — None loser_clid means tie
    Resolved { message: String, loser_clid: Option<u16> },
}

pub enum DuelDeclineResult {
    NotYourDuel,
    NoDuel,
    Declined(String),
}

pub enum DuelStatusResult {
    Pending(String),
    ExpiredOrNone(String),
}

pub enum DuelChallengeResult {
    AlreadyActive(String),
    NoPendingDuel,
}

/// Handle `!duel accept` — resolve the duel with dice rolls
pub fn duel_accept(
    duel: &crate::models::state::ActiveDuel,
    sender_uid: &str,
) -> DuelAcceptResult {
    use rand::Rng;
    if duel.target_uid != sender_uid {
        return DuelAcceptResult::NotYourDuel;
    }
    if duel.created.elapsed().as_secs() > 30 {
        return DuelAcceptResult::Expired;
    }

    let roll1: u8 = rand::thread_rng().gen_range(1..=6) + rand::thread_rng().gen_range(1..=6);
    let roll2: u8 = rand::thread_rng().gen_range(1..=6) + rand::thread_rng().gen_range(1..=6);

    let (winner, loser, loser_clid) = if roll1 > roll2 {
        (&duel.challenger_name, &duel.target_name, Some(duel.target_clid))
    } else if roll2 > roll1 {
        (&duel.target_name, &duel.challenger_name, Some(duel.challenger_clid))
    } else {
        let msg = format!(
            "⚔️ DUEL : {} 🎲{} vs {} 🎲{}\n🤝 Égalité ! Personne ne meurt... cette fois.",
            duel.challenger_name, roll1, duel.target_name, roll2
        );
        return DuelAcceptResult::Resolved { message: msg, loser_clid: None };
    };

    let msg = format!(
        "⚔️ DUEL : {} 🎲{} vs {} 🎲{}\n🏆 {} gagne ! 💀 {} est éliminé(e) !",
        duel.challenger_name, roll1, duel.target_name, roll2, winner, loser
    );
    DuelAcceptResult::Resolved { message: msg, loser_clid }
}

/// Handle `!duel decline`
pub fn duel_decline(
    duel: &crate::models::state::ActiveDuel,
    sender_uid: &str,
) -> DuelDeclineResult {
    if duel.target_uid != sender_uid {
        DuelDeclineResult::NotYourDuel
    } else {
        DuelDeclineResult::Declined(format!(
            "🏳️ {} refuse le duel de {}. Lâche !",
            duel.target_name, duel.challenger_name
        ))
    }
}

/// Handle `!duel` with no args — show status
pub fn duel_status(duel: Option<&crate::models::state::ActiveDuel>) -> DuelStatusResult {
    let usage = "❌ Usage: !duel <nom> — défier quelqu'un en duel (2d6, perdant = kick)".to_string();
    match duel {
        Some(d) if d.created.elapsed().as_secs() <= 30 => {
            let remaining = 30 - d.created.elapsed().as_secs();
            DuelStatusResult::Pending(format!(
                "⚔️ Duel en attente : {} vs {} ({}s restantes)\n{} doit taper [b]!duel accept[/b] ou [b]!duel non[/b]",
                d.challenger_name, d.target_name, remaining, d.target_name
            ))
        }
        _ => DuelStatusResult::ExpiredOrNone(usage),
    }
}

/// Check if there's an active non-expired duel blocking a new challenge
pub fn duel_check_active(duel: Option<&crate::models::state::ActiveDuel>) -> Option<String> {
    match duel {
        Some(d) if d.created.elapsed().as_secs() <= 30 => Some(format!(
            "❌ Un duel est déjà en cours : {} vs {} ! Attends qu'il expire.",
            d.challenger_name, d.target_name
        )),
        _ => None,
    }
}

// --- TTS option parsing ---

/// Result of parsing `!tts` prefix options (voice:X speed:X).
#[derive(Debug, PartialEq)]
pub struct TtsParseResult {
    pub voice: Option<String>,
    pub speed: Option<f32>,
    pub text: String,
}

/// Parse `voice:XX` and `speed:XX` prefixes from a `!tts` command's raw text.
/// `valid_voices` should contain lowercase voice names.
/// Returns the extracted options and the remaining text.
pub fn tts_parse_options(raw_text: &str, valid_voices: &[String]) -> TtsParseResult {
    let mut voice: Option<String> = None;
    let mut speed: Option<f32> = None;
    let mut remaining = raw_text;

    loop {
        let trimmed = remaining.trim_start();
        if let Some(rest) = trimmed.strip_prefix("voice:") {
            let end = rest.find(' ').unwrap_or(rest.len());
            let v = &rest[..end];
            if valid_voices.iter().any(|valid| valid == &v.to_lowercase()) {
                voice = Some(v.to_lowercase());
                remaining = &rest[end..];
                continue;
            }
        }
        if let Some(rest) = trimmed.strip_prefix("speed:") {
            let end = rest.find(' ').unwrap_or(rest.len());
            if let Ok(s) = rest[..end].parse::<f32>() {
                if (0.25..=4.0).contains(&s) {
                    speed = Some(s);
                    remaining = &rest[end..];
                    continue;
                }
            }
        }
        break;
    }

    TtsParseResult {
        voice,
        speed,
        text: remaining.trim().to_string(),
    }
}

/// Validation result for parsed TTS input.
pub enum TtsValidation {
    Ok,
    Empty(String),
    TooLong(String),
}

/// Validate a parsed TTS result (non-empty, max 500 chars).
pub fn tts_validate(parsed: &TtsParseResult) -> TtsValidation {
    if parsed.text.is_empty() {
        TtsValidation::Empty(
            "❌ Usage: !tts [voice:nova] [speed:1.5] <texte>\nVoix: alloy, ash, ballad, coral, echo, fable, nova, onyx, sage, shimmer, verse".to_string()
        )
    } else if parsed.text.len() > 500 {
        TtsValidation::TooLong("❌ Texte trop long (max 500 caractères)".to_string())
    } else {
        TtsValidation::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_help_text_not_empty() {
        let text = help_text();
        assert!(text.contains("!help"));
        assert!(text.contains("!listen"));
        assert!(text.contains("!roll"));
    }

    #[test]
    fn test_ping_response() {
        let resp = ping_response(3661);
        assert!(resp.contains("Pong"));
        assert!(resp.contains("1h 1m"));
    }

    #[test]
    fn test_roll_dice_default() {
        let resp = roll_dice("");
        assert!(resp.starts_with("🎲"));
    }

    #[test]
    fn test_roll_dice_simple_number() {
        let resp = roll_dice("20");
        assert!(resp.contains("1-20"));
    }

    #[test]
    fn test_roll_dice_notation() {
        let resp = roll_dice("2d6");
        assert!(resp.contains("2d6"));
    }

    #[test]
    fn test_roll_dice_invalid() {
        let resp = roll_dice("abc");
        assert!(resp.starts_with("❌"));
    }

    #[test]
    fn test_roll_dice_modifier() {
        let resp = roll_dice("1d20+5");
        assert!(resp.contains("+5"));
    }

    #[test]
    fn test_eight_ball_empty() {
        assert!(eight_ball("test", "").is_none());
    }

    #[test]
    fn test_eight_ball_with_question() {
        let resp = eight_ball("Nicolas", "Will it rain?").unwrap();
        assert!(resp.contains("Nicolas"));
        assert!(resp.contains("Will it rain?"));
    }

    #[test]
    fn test_stats_response() {
        let stats = BotStats::default();
        let resp = stats_response(3600, &stats);
        assert!(resp.contains("Statistiques"));
        assert!(resp.contains("Messages reçus"));
    }

    #[test]
    fn test_quote_empty_usage() {
        let tmp = "/tmp/test_quotes_empty.json";
        let _ = std::fs::remove_file(tmp);
        let QuoteAction::Response(resp) = quote_command("", "tester", tmp);
        assert!(resp.contains("Aucune quote"));
    }

    #[test]
    fn test_quote_add_and_count() {
        let tmp = "/tmp/test_quotes_add.json";
        let _ = std::fs::remove_file(tmp);
        let QuoteAction::Response(resp) = quote_command("add Hello world", "tester", tmp);
        assert!(resp.contains("#1"));
        let QuoteAction::Response(resp) = quote_command("count", "tester", tmp);
        assert!(resp.contains("1 quote"));
        let _ = std::fs::remove_file(tmp);
    }

    #[test]
    fn test_quote_add_too_long() {
        let tmp = "/tmp/test_quotes_long.json";
        let _ = std::fs::remove_file(tmp);
        let long = "x".repeat(501);
        let QuoteAction::Response(resp) = quote_command(&format!("add {}", long), "tester", tmp);
        assert!(resp.contains("trop longue"));
        let _ = std::fs::remove_file(tmp);
    }

    #[test]
    fn test_quote_list_and_del() {
        let tmp = "/tmp/test_quotes_list.json";
        let _ = std::fs::remove_file(tmp);
        let _ = quote_command("add Quote one", "alice", tmp);
        let _ = quote_command("add Quote two", "bob", tmp);
        let QuoteAction::Response(resp) = quote_command("list", "alice", tmp);
        assert!(resp.contains("Quote one"));
        assert!(resp.contains("Quote two"));
        let QuoteAction::Response(resp) = quote_command("del 1", "alice", tmp);
        assert!(resp.contains("supprimée"));
        let QuoteAction::Response(resp) = quote_command("count", "alice", tmp);
        assert!(resp.contains("1 quote"));
        let _ = std::fs::remove_file(tmp);
    }

    #[test]
    fn test_quote_invalid_subcommand() {
        let tmp = "/tmp/test_quotes_invalid.json";
        let _ = std::fs::remove_file(tmp);
        let QuoteAction::Response(resp) = quote_command("blah", "tester", tmp);
        assert!(resp.contains("Usage"));
        let _ = std::fs::remove_file(tmp);
    }

    #[test]
    fn test_history_empty() {
        let hist = std::collections::VecDeque::new();
        assert!(history_response(&hist, "").is_none());
    }

    #[test]
    fn test_history_with_entries() {
        let mut hist = std::collections::VecDeque::new();
        hist.push_back(("12:00".to_string(), "alice".to_string(), "hello".to_string()));
        hist.push_back(("12:01".to_string(), "bob".to_string(), "world".to_string()));
        let resp = history_response(&hist, "").unwrap();
        assert!(resp.contains("alice"));
        assert!(resp.contains("bob"));
        assert!(resp.contains("2 message"));
    }

    #[test]
    fn test_seen_no_query_shows_count() {
        let seen = std::collections::HashMap::new();
        let r = seen_response(&seen, "");
        assert!(r.contains("0 utilisateur"));
    }

    #[test]
    fn test_seen_single_match() {
        let mut seen = std::collections::HashMap::new();
        seen.insert("uid1".to_string(), ("marlburrow".to_string(), "2026-02-11 22:00".to_string()));
        let r = seen_response(&seen, "marl");
        assert!(r.contains("marlburrow"));
        assert!(r.contains("2026-02-11 22:00"));
    }

    #[test]
    fn test_seen_no_match() {
        let mut seen = std::collections::HashMap::new();
        seen.insert("uid1".to_string(), ("marlburrow".to_string(), "2026-02-11 22:00".to_string()));
        let r = seen_response(&seen, "unknown");
        assert!(r.contains("Aucun résultat"));
    }

    #[test]
    fn test_seen_multiple_matches() {
        let mut seen = std::collections::HashMap::new();
        seen.insert("uid1".to_string(), ("marlburrow".to_string(), "2026-02-11 22:00".to_string()));
        seen.insert("uid2".to_string(), ("marlbot".to_string(), "2026-02-11 21:00".to_string()));
        let r = seen_response(&seen, "marl");
        assert!(r.contains("2 résultats"));
    }

    #[test]
    fn test_history_truncates_long_messages() {
        let mut hist = std::collections::VecDeque::new();
        let long_msg = "x".repeat(200);
        hist.push_back(("12:00".to_string(), "alice".to_string(), long_msg));
        let resp = history_response(&hist, "").unwrap();
        assert!(resp.contains("..."));
    }

    #[test]
    fn test_poll_show_no_poll() {
        assert_eq!(poll_show(None), PollResponse::Help);
    }

    #[test]
    fn test_poll_show_active() {
        let poll = ActivePoll {
            question: "Best lang?".to_string(),
            options: vec!["Rust".to_string(), "Go".to_string()],
            votes: vec![std::collections::HashSet::new(), std::collections::HashSet::new()],
            creator: "alice".to_string(),
        };
        if let PollResponse::Message(msg) = poll_show(Some(&poll)) {
            assert!(msg.contains("Best lang?"));
            assert!(msg.contains("alice"));
            assert!(msg.contains("Rust"));
        } else {
            panic!("Expected Message");
        }
    }

    #[test]
    fn test_poll_create_valid() {
        let result = poll_create("Best? | Rust | Go", "alice");
        assert!(result.is_some());
        let (poll, msg) = result.unwrap();
        assert_eq!(poll.question, "Best?");
        assert_eq!(poll.options.len(), 2);
        assert!(msg.contains("Nouveau sondage"));
    }

    #[test]
    fn test_poll_create_too_few_options() {
        assert!(poll_create("Just a question", "alice").is_none());
    }

    #[test]
    fn test_poll_end_with_votes() {
        let mut poll = ActivePoll {
            question: "Best?".to_string(),
            options: vec!["A".to_string(), "B".to_string()],
            votes: vec![{
                let mut s = std::collections::HashSet::new();
                s.insert("uid1".to_string());
                s.insert("uid2".to_string());
                s
            }, {
                let mut s = std::collections::HashSet::new();
                s.insert("uid3".to_string());
                s
            }],
            creator: "alice".to_string(),
        };
        let msg = poll_end(&mut poll);
        assert!(msg.contains("👑"));
        assert!(msg.contains("3 votes"));
    }

    #[test]
    fn test_vote_valid() {
        let mut poll = ActivePoll {
            question: "Q".to_string(),
            options: vec!["A".to_string(), "B".to_string()],
            votes: vec![std::collections::HashSet::new(), std::collections::HashSet::new()],
            creator: "x".to_string(),
        };
        let result = vote(&mut poll, "1", "uid1", "alice");
        if let VoteResult::Voted(msg) = result {
            assert!(msg.contains("alice"));
            assert!(msg.contains("A"));
        } else {
            panic!("Expected Voted");
        }
        assert!(poll.votes[0].contains("uid1"));
    }

    #[test]
    fn test_vote_change() {
        let mut poll = ActivePoll {
            question: "Q".to_string(),
            options: vec!["A".to_string(), "B".to_string()],
            votes: vec![{
                let mut s = std::collections::HashSet::new();
                s.insert("uid1".to_string());
                s
            }, std::collections::HashSet::new()],
            creator: "x".to_string(),
        };
        let result = vote(&mut poll, "2", "uid1", "alice");
        if let VoteResult::Voted(msg) = result {
            assert!(msg.contains("changé"));
        } else {
            panic!("Expected Voted with change");
        }
        assert!(!poll.votes[0].contains("uid1"));
        assert!(poll.votes[1].contains("uid1"));
    }

    #[test]
    fn test_vote_out_of_range() {
        let mut poll = ActivePoll {
            question: "Q".to_string(),
            options: vec!["A".to_string()],
            votes: vec![std::collections::HashSet::new()],
            creator: "x".to_string(),
        };
        assert!(matches!(vote(&mut poll, "5", "uid1", "alice"), VoteResult::Error(_)));
    }

    #[test]
    fn test_vote_invalid_number() {
        let mut poll = ActivePoll {
            question: "Q".to_string(),
            options: vec!["A".to_string()],
            votes: vec![std::collections::HashSet::new()],
            creator: "x".to_string(),
        };
        assert!(matches!(vote(&mut poll, "abc", "uid1", "alice"), VoteResult::Error(_)));
    }

    // --- Notify tests ---

    // --- Remind tests ---

    #[test]
    fn test_remind_empty_shows_usage() {
        let reminders = vec![];
        let r = remind_command(&reminders, "", "uid1", "alice", 0);
        match r {
            RemindResult::Response(msg) => assert!(msg.contains("Aucun rappel")),
            _ => panic!("Expected Response"),
        }
    }

    #[test]
    fn test_remind_list_shows_pending() {
        let reminders = vec![Reminder {
            due_ms: 999_999_999_999,
            uid: "uid1".to_string(),
            name: "alice".to_string(),
            message: "Check oven".to_string(),
            created_ms: 0,
        }];
        let r = remind_command(&reminders, "list", "uid1", "alice", 1000);
        match r {
            RemindResult::Response(msg) => {
                assert!(msg.contains("1 rappel"));
                assert!(msg.contains("Check oven"));
            }
            _ => panic!("Expected Response"),
        }
    }

    #[test]
    fn test_remind_clear() {
        let reminders = vec![Reminder {
            due_ms: 999_999_999_999,
            uid: "uid1".to_string(),
            name: "alice".to_string(),
            message: "Test".to_string(),
            created_ms: 0,
        }];
        let r = remind_command(&reminders, "clear", "uid1", "alice", 0);
        match r {
            RemindResult::Clear { message, removed } => {
                assert_eq!(removed, 1);
                assert!(message.contains("1 rappel"));
            }
            _ => panic!("Expected Clear"),
        }
    }

    #[test]
    fn test_remind_create_valid() {
        let reminders = vec![];
        let r = remind_command(&reminders, "30m Check the oven", "uid1", "alice", 1000);
        match r {
            RemindResult::Add { message, reminder } => {
                assert!(message.contains("Check the oven"));
                assert_eq!(reminder.message, "Check the oven");
                assert_eq!(reminder.uid, "uid1");
            }
            _ => panic!("Expected Add, got {:?}", r),
        }
    }

    #[test]
    fn test_remind_too_short() {
        let reminders = vec![];
        let r = remind_command(&reminders, "5s hello", "uid1", "alice", 0);
        match r {
            RemindResult::Response(msg) => assert!(msg.contains("trop courte")),
            _ => panic!("Expected Response"),
        }
    }

    #[test]
    fn test_remind_max_limit() {
        let reminders: Vec<Reminder> = (0..10)
            .map(|i| Reminder {
                due_ms: 999_999_999_999,
                uid: "uid1".to_string(),
                name: "alice".to_string(),
                message: format!("r{}", i),
                created_ms: 0,
            })
            .collect();
        let r = remind_command(&reminders, "30m another", "uid1", "alice", 1000);
        match r {
            RemindResult::Response(msg) => assert!(msg.contains("Maximum 10")),
            _ => panic!("Expected Response"),
        }
    }

    #[test]
    fn test_remind_invalid_duration() {
        let reminders = vec![];
        let r = remind_command(&reminders, "xyz hello", "uid1", "alice", 0);
        match r {
            RemindResult::Response(msg) => assert!(msg.contains("Durée invalide")),
            _ => panic!("Expected Response"),
        }
    }

    #[test]
    fn test_remind_no_message() {
        let reminders = vec![];
        let r = remind_command(&reminders, "30m", "uid1", "alice", 0);
        match r {
            RemindResult::Response(msg) => assert!(msg.contains("Il faut un message")),
            _ => panic!("Expected Response"),
        }
    }

    #[test]
    fn test_notify_empty_shows_help() {
        let mut watchers = NotifyWatchersMap::new();
        let r = notify_command(&mut watchers, "", "uid1", "alice");
        let NotifyResult::Response { message, changed } = r;
        assert!(message.contains("Aucune notification"));
        assert!(!changed);
    }

    #[test]
    fn test_notify_add_and_list() {
        let mut watchers = NotifyWatchersMap::new();
        let r = notify_command(&mut watchers, "bob", "uid1", "alice");
        let NotifyResult::Response { message, changed } = r;
        assert!(message.contains("Tu seras notifié"));
        assert!(changed);
        assert_eq!(watchers.len(), 1);

        // List
        let r2 = notify_command(&mut watchers, "", "uid1", "alice");
        let NotifyResult::Response { message: msg2, .. } = r2;
        assert!(msg2.contains("bob"));
    }

    #[test]
    fn test_notify_toggle_off() {
        let mut watchers = NotifyWatchersMap::new();
        notify_command(&mut watchers, "bob", "uid1", "alice");
        let r = notify_command(&mut watchers, "bob", "uid1", "alice");
        let NotifyResult::Response { message, changed } = r;
        assert!(message.contains("désactivée"));
        assert!(changed);
        assert!(watchers.is_empty());
    }

    #[test]
    fn test_notify_clear() {
        let mut watchers = NotifyWatchersMap::new();
        notify_command(&mut watchers, "bob", "uid1", "alice");
        notify_command(&mut watchers, "charlie", "uid1", "alice");
        let r = notify_command(&mut watchers, "clear", "uid1", "alice");
        let NotifyResult::Response { message, changed } = r;
        assert!(message.contains("2 notification"));
        assert!(changed);
        assert!(watchers.is_empty());
    }

    // --- AFK tests ---

    #[test]
    fn test_afk_set() {
        let r = afk_command("Going for lunch", "uid1", "alice");
        let AfkResult::Set { message, afk_entry } = r else { panic!("expected Set") };
        assert!(message.contains("AFK activé"));
        assert!(message.contains("Going for lunch"));
        assert_eq!(afk_entry.0, "alice");
    }

    #[test]
    fn test_afk_clear_when_not_afk() {
        let afk: HashMap<String, (String, String)> = HashMap::new();
        let r = afk_command("", "uid1", "alice");
        // Without existing AFK, this is a Clear variant
        let AfkResult::Clear { was_afk, .. } = r else { panic!("expected Clear") };
        assert!(!was_afk);
    }

    #[test]
    fn test_afk_clear_explicit() {
        let r = afk_command("off", "uid1", "alice");
        let AfkResult::Clear { was_afk, .. } = r else { panic!("expected Clear") };
        // was_afk is determined by caller — the pure function just signals intent
        assert!(!was_afk); // we can't check this without state, but the variant is correct
    }

    #[test]
    fn test_afk_mention_detection() {
        let mut afk: HashMap<String, (String, String)> = HashMap::new();
        afk.insert("uid2".to_string(), ("Bob".to_string(), "eating".to_string()));
        let result = afk_check_mentions("hey bob are you there?", "uid1", &afk);
        assert!(result.is_some());
        let (name, msg) = result.unwrap();
        assert_eq!(name, "Bob");
        assert_eq!(msg, "eating");
    }

    #[test]
    fn test_afk_mention_no_self() {
        let mut afk: HashMap<String, (String, String)> = HashMap::new();
        afk.insert("uid1".to_string(), ("Alice".to_string(), "brb".to_string()));
        let result = afk_check_mentions("alice says hi", "uid1", &afk);
        assert!(result.is_none()); // should not match own AFK
    }

    #[test]
    fn test_afk_mention_no_match() {
        let mut afk: HashMap<String, (String, String)> = HashMap::new();
        afk.insert("uid2".to_string(), ("Bob".to_string(), "eating".to_string()));
        let result = afk_check_mentions("hello everyone", "uid1", &afk);
        assert!(result.is_none());
    }

    // --- Lang tests ---

    #[test]
    fn test_lang_auto() {
        let r = lang_command("auto");
        let LangResult::Reset { message } = r else { panic!("expected Reset") };
        assert!(message.contains("auto"));
    }

    #[test]
    fn test_lang_valid() {
        let r = lang_command("fr");
        let LangResult::Set { message, code } = r else { panic!("expected Set") };
        assert_eq!(code, "fr");
        assert!(message.contains("fr"));
    }

    #[test]
    fn test_lang_invalid() {
        let r = lang_command("xx");
        let LangResult::Invalid { message } = r else { panic!("expected Invalid") };
        assert!(message.contains("xx"));
    }

    #[test]
    fn test_lang_empty() {
        let r = lang_command("");
        let LangResult::Reset { message } = r else { panic!("expected Reset") };
        assert!(message.contains("auto"));
    }

    // --- greet ---

    #[test]
    fn test_greet_on() {
        let GreetResult::SetEnabled { message, enabled } = greet_command("on", true) else { panic!("expected SetEnabled") };
        assert!(enabled);
        assert!(message.contains("activés"));
    }

    #[test]
    fn test_greet_off() {
        let GreetResult::SetEnabled { message, enabled } = greet_command("off", false) else { panic!("expected SetEnabled") };
        assert!(!enabled);
        assert!(message.contains("désactivés"));
    }

    #[test]
    fn test_greet_status() {
        let GreetResult::Status(msg) = greet_command("", true) else { panic!("expected Status") };
        assert!(msg.contains("activés ✅"));

        let GreetResult::Status(msg) = greet_command("", false) else { panic!("expected Status") };
        assert!(msg.contains("désactivés ❌"));
    }

    #[test]
    fn test_greet_invalid() {
        let GreetResult::Invalid(msg) = greet_command("maybe", false) else { panic!("expected Invalid") };
        assert!(msg.contains("Usage"));
    }

    // --- timeout ---

    #[test]
    fn test_timeout_set_valid() {
        let TimeoutResult::Set { message, value_ms } = timeout_command("2000", 1500) else { panic!("expected Set") };
        assert_eq!(value_ms, 2000);
        assert!(message.contains("2000ms"));
    }

    #[test]
    fn test_timeout_set_clamped() {
        let TimeoutResult::Set { value_ms, .. } = timeout_command("100", 1500) else { panic!("expected Set") };
        assert_eq!(value_ms, 500);

        let TimeoutResult::Set { value_ms, .. } = timeout_command("99999", 1500) else { panic!("expected Set") };
        assert_eq!(value_ms, 10000);
    }

    #[test]
    fn test_timeout_show() {
        let TimeoutResult::Show(msg) = timeout_command("", 1500) else { panic!("expected Show") };
        assert!(msg.contains("1500ms"));
    }

    #[test]
    fn test_timeout_invalid() {
        let TimeoutResult::Invalid(msg) = timeout_command("abc", 1500) else { panic!("expected Invalid") };
        assert!(msg.contains("Usage"));
    }

    // --- Volume tests ---

    #[test]
    fn test_volume_show() {
        let VolumeResult::Show(msg) = volume_command("", 75) else { panic!("expected Show") };
        assert!(msg.contains("75%"));
    }

    #[test]
    fn test_volume_set_valid() {
        let VolumeResult::Set { message, value } = volume_command("80", 50) else { panic!("expected Set") };
        assert_eq!(value, 80);
        assert!(message.contains("80%"));
    }

    #[test]
    fn test_volume_set_with_percent() {
        let VolumeResult::Set { value, .. } = volume_command("120%", 50) else { panic!("expected Set") };
        assert_eq!(value, 120);
    }

    #[test]
    fn test_volume_set_zero() {
        let VolumeResult::Set { message, value } = volume_command("0", 50) else { panic!("expected Set") };
        assert_eq!(value, 0);
        assert!(message.contains("🔇"));
    }

    #[test]
    fn test_volume_too_high() {
        let VolumeResult::Invalid(msg) = volume_command("250", 50) else { panic!("expected Invalid") };
        assert!(msg.contains("200"));
    }

    #[test]
    fn test_volume_invalid_text() {
        let VolumeResult::Invalid(msg) = volume_command("loud", 50) else { panic!("expected Invalid") };
        assert!(msg.contains("Usage"));
    }

    // --- Voice tests ---

    #[test]
    fn test_voice_show() {
        let voices = vec!["alloy".to_string(), "nova".to_string(), "onyx".to_string()];
        let VoiceResult::Show(msg) = voice_command("", "nova", &voices) else { panic!("expected Show") };
        assert!(msg.contains("nova"));
        assert!(msg.contains("alloy"));
    }

    #[test]
    fn test_voice_set_valid() {
        let voices = vec!["alloy".to_string(), "nova".to_string(), "onyx".to_string()];
        let VoiceResult::Set { voice, message } = voice_command("alloy", "nova", &voices) else { panic!("expected Set") };
        assert_eq!(voice, "alloy");
        assert!(message.contains("alloy"));
    }

    #[test]
    fn test_voice_invalid() {
        let voices = vec!["alloy".to_string(), "nova".to_string()];
        let VoiceResult::Invalid(msg) = voice_command("siri", "nova", &voices) else { panic!("expected Invalid") };
        assert!(msg.contains("siri"));
        assert!(msg.contains("alloy"));
    }

    // --- Speed tests ---

    #[test]
    fn test_speed_show() {
        let SpeedResult::Show(msg) = speed_command("", 1.15) else { panic!("expected Show") };
        assert!(msg.contains("1.15"));
    }

    #[test]
    fn test_speed_set_valid() {
        let SpeedResult::Set { value, message } = speed_command("1.5", 1.0) else { panic!("expected Set") };
        assert!((value - 1.5).abs() < 0.001);
        assert!(message.contains("1.50"));
    }

    #[test]
    fn test_speed_invalid_range() {
        let SpeedResult::Invalid(_) = speed_command("5.0", 1.0) else { panic!("expected Invalid") };
    }

    #[test]
    fn test_speed_invalid_text() {
        let SpeedResult::Invalid(_) = speed_command("fast", 1.0) else { panic!("expected Invalid") };
    }

    // --- roulette tests ---

    #[test]
    fn test_roulette_returns_result() {
        // Run multiple times to cover both branches probabilistically
        let mut saw_bang = false;
        let mut saw_survived = false;
        for _ in 0..100 {
            match roulette_command("TestUser") {
                RouletteResult::Bang(msg) => {
                    assert!(msg.contains("BANG"));
                    assert!(msg.contains("TestUser"));
                    saw_bang = true;
                }
                RouletteResult::Survived(msg) => {
                    assert!(msg.contains("TestUser"));
                    saw_survived = true;
                }
            }
        }
        assert!(saw_bang, "should have seen at least one Bang in 100 rolls");
        assert!(saw_survived, "should have seen at least one Survived in 100 rolls");
    }

    // --- duel tests ---

    #[test]
    fn test_duel_decline_not_your_duel() {
        let duel = crate::models::state::ActiveDuel {
            challenger_name: "Alice".to_string(),
            challenger_uid: "uid_a".to_string(),
            challenger_clid: 1,
            target_name: "Bob".to_string(),
            target_uid: "uid_b".to_string(),
            target_clid: 2,
            created: std::time::Instant::now(),
        };
        let DuelDeclineResult::NotYourDuel = duel_decline(&duel, "uid_c") else { panic!("expected NotYourDuel") };
    }

    #[test]
    fn test_duel_decline_ok() {
        let duel = crate::models::state::ActiveDuel {
            challenger_name: "Alice".to_string(),
            challenger_uid: "uid_a".to_string(),
            challenger_clid: 1,
            target_name: "Bob".to_string(),
            target_uid: "uid_b".to_string(),
            target_clid: 2,
            created: std::time::Instant::now(),
        };
        let DuelDeclineResult::Declined(msg) = duel_decline(&duel, "uid_b") else { panic!("expected Declined") };
        assert!(msg.contains("Bob"));
        assert!(msg.contains("Lâche"));
    }

    #[test]
    fn test_duel_status_none() {
        let DuelStatusResult::ExpiredOrNone(_) = duel_status(None) else { panic!("expected ExpiredOrNone") };
    }

    #[test]
    fn test_duel_status_pending() {
        let duel = crate::models::state::ActiveDuel {
            challenger_name: "Alice".to_string(),
            challenger_uid: "uid_a".to_string(),
            challenger_clid: 1,
            target_name: "Bob".to_string(),
            target_uid: "uid_b".to_string(),
            target_clid: 2,
            created: std::time::Instant::now(),
        };
        let DuelStatusResult::Pending(msg) = duel_status(Some(&duel)) else { panic!("expected Pending") };
        assert!(msg.contains("Alice"));
        assert!(msg.contains("Bob"));
    }

    #[test]
    fn test_duel_check_no_active() {
        assert!(duel_check_active(None).is_none());
    }

    // --- TTS parse tests ---

    #[test]
    fn test_tts_parse_no_options() {
        let voices = vec!["nova".to_string(), "onyx".to_string()];
        let result = tts_parse_options("hello world", &voices);
        assert_eq!(result.voice, None);
        assert_eq!(result.speed, None);
        assert_eq!(result.text, "hello world");
    }

    #[test]
    fn test_tts_parse_voice_only() {
        let voices = vec!["nova".to_string(), "onyx".to_string()];
        let result = tts_parse_options("voice:nova hello world", &voices);
        assert_eq!(result.voice, Some("nova".to_string()));
        assert_eq!(result.speed, None);
        assert_eq!(result.text, "hello world");
    }

    #[test]
    fn test_tts_parse_speed_only() {
        let voices = vec!["nova".to_string()];
        let result = tts_parse_options("speed:1.5 hello world", &voices);
        assert_eq!(result.voice, None);
        assert_eq!(result.speed, Some(1.5));
        assert_eq!(result.text, "hello world");
    }

    #[test]
    fn test_tts_parse_both() {
        let voices = vec!["nova".to_string(), "onyx".to_string()];
        let result = tts_parse_options("voice:onyx speed:2.0 salut", &voices);
        assert_eq!(result.voice, Some("onyx".to_string()));
        assert_eq!(result.speed, Some(2.0));
        assert_eq!(result.text, "salut");
    }

    #[test]
    fn test_tts_parse_invalid_voice() {
        let voices = vec!["nova".to_string()];
        let result = tts_parse_options("voice:unknown hello", &voices);
        assert_eq!(result.voice, None);
        assert_eq!(result.text, "voice:unknown hello");
    }

    #[test]
    fn test_tts_parse_speed_out_of_range() {
        let voices = vec!["nova".to_string()];
        let result = tts_parse_options("speed:10.0 hello", &voices);
        assert_eq!(result.speed, None);
        assert_eq!(result.text, "speed:10.0 hello");
    }

    #[test]
    fn test_tts_parse_empty_text_after_options() {
        let voices = vec!["nova".to_string()];
        let result = tts_parse_options("voice:nova", &voices);
        assert_eq!(result.voice, Some("nova".to_string()));
        assert_eq!(result.text, "");
    }

    #[test]
    fn test_tts_validate_ok() {
        let r = TtsParseResult { voice: Some("nova".to_string()), speed: Some(1.0), text: "hi".to_string() };
        assert!(matches!(tts_validate(&r), TtsValidation::Ok));
    }

    #[test]
    fn test_tts_validate_empty() {
        let r = TtsParseResult { voice: None, speed: None, text: "".to_string() };
        assert!(matches!(tts_validate(&r), TtsValidation::Empty(_)));
    }

    #[test]
    fn test_tts_validate_too_long() {
        let r = TtsParseResult { voice: None, speed: None, text: "a".repeat(501) };
        assert!(matches!(tts_validate(&r), TtsValidation::TooLong(_)));
    }
}
