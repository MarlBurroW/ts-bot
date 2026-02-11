//! Chat command response generators.
//!
//! Pure functions that take command arguments and return response strings.
//! Extracted from main.rs to reduce its size and improve testability.

use rand::Rng;

use crate::models::BotStats;
use crate::utils::{format_uptime, truncate_str};

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
}
