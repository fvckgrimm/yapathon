//! Chat commands.
//!
//! `!yap extend 500` is how a yapathon runs before anyone has wired up OAuth,
//! and it is more fun than an automatic bonus: chat arguing about whether to
//! give 500 or 5000 words is the game.
//!
//! Parsing is separated from execution so every command can be tested without a
//! session, and so a malformed command produces a chat reply rather than a
//! warning in a log nobody reads.

use yap_core::goal::{Event, GoalMode, HardLimit};

/// What a chat message asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Show the current state.
    Status,
    /// Start a yapathon with the given goal.
    Start {
        words: u32,
    },
    /// Extend the goal by `words`.
    Extend {
        words: u32,
    },
    /// Move the goal to an absolute value.
    Set {
        words: u32,
    },
    /// Pause or resume counting.
    Pause,
    Resume,
    /// End the run.
    End,
    /// Void the last utterance.
    Undo,
    /// Ask chat for words: `!yap please 250`.
    Ask {
        words: u32,
    },
    /// Not a command.
    NotACommand,
    /// A command that could not be parsed, with the reason to say in chat.
    Invalid {
        reason: String,
    },
}

/// Parse one chat message.
///
/// Only messages starting with `!yap` are commands. `!` is Twitch's prefix and
/// `!` is what chat already types, so nothing else is intercepted.
pub fn parse(message: &str) -> Command {
    let text = message.trim();

    let Some(rest) = strip_prefix_ci(text, "!yap") else {
        return Command::NotACommand;
    };

    // `!yapathon` is a different word and should not be swallowed by `!yap`.
    if rest.starts_with(|c: char| c.is_alphanumeric()) {
        return Command::NotACommand;
    }

    let mut parts = rest.split_whitespace();
    let Some(first) = parts.next() else {
        return Command::Status;
    };

    // `!yap +500` is the form chat actually types: the verb is attached to the
    // count with no space, so splitting on whitespace first would leave `+500`
    // looking like an unknown verb.
    if let Some(attached) = first.strip_prefix('+')
        && !attached.is_empty()
    {
        return match parse_count(attached) {
            Some(words) => Command::Extend { words },
            None => Command::Invalid {
                reason: "usage: !yap +500".to_string(),
            },
        };
    }

    let verb = first;
    let argument = parts.next();

    match verb.to_lowercase().as_str() {
        "" | "status" => Command::Status,
        "start" | "go" => match argument.and_then(parse_count) {
            Some(words) => Command::Start { words },
            None => Command::Invalid {
                reason: "usage: !yap start 5000".to_string(),
            },
        },
        "extend" | "add" | "plus" => match argument.and_then(parse_count) {
            Some(words) => Command::Extend { words },
            None => Command::Invalid {
                reason: "usage: !yap extend 500".to_string(),
            },
        },
        "set" => match argument.and_then(parse_count) {
            Some(words) => Command::Set { words },
            None => Command::Invalid {
                reason: "usage: !yap set 7500".to_string(),
            },
        },
        "please" | "?" => match argument.and_then(parse_count) {
            Some(words) => Command::Ask { words },
            None => Command::Invalid {
                reason: "usage: !yap please 250".to_string(),
            },
        },
        "pause" => Command::Pause,
        "resume" => Command::Resume,
        "stop" | "end" => Command::End,
        "undo" | "oops" => Command::Undo,
        other => Command::Invalid {
            reason: format!("unknown command {other:?}"),
        },
    }
}

/// Parse a word count, accepting `5k` and commas.
///
/// Rejects zero: extending a goal by nothing is a command that did nothing,
/// which is worse than saying so.
fn parse_count(text: &str) -> Option<u32> {
    let cleaned = text.replace([',', '_'], "");
    let (digits, multiplier) = match cleaned.chars().last() {
        Some('k') | Some('K') => (&cleaned[..cleaned.len() - 1], 1_000u32),
        Some('m') | Some('M') => (&cleaned[..cleaned.len() - 1], 1_000_000),
        _ => (cleaned.as_str(), 1),
    };

    let value: u32 = digits.trim().parse().ok()?;
    if value == 0 {
        return None;
    }

    value.checked_mul(multiplier)
}

/// Case-insensitive prefix strip.
fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    if text.len() < prefix.len() {
        return None;
    }
    let (head, tail) = text.split_at(prefix.len());
    head.eq_ignore_ascii_case(prefix).then_some(tail)
}

/// Turn a parsed command into the event that moves a goal.
///
/// Commands that are not goal changes produce `None`: pause, undo and status
/// are actions on the session, not events to price.
pub fn command_event(command: &Command, user: Option<String>) -> Option<Event> {
    match command {
        Command::Extend { words } => {
            let mut event = Event::granting("command", *words);
            event.user = user;
            event.note = Some("chat command".to_string());
            Some(event)
        }
        Command::Ask { words } => {
            let mut event = Event::granting("command", *words);
            event.user = user;
            event.note = Some("chat request".to_string());
            Some(event)
        }
        _ => None,
    }
}

/// The session config a `!yap start` produces.
///
/// Uses the same hard limit as the app's default, so a chat-started yapathon is
/// not an unbounded one — that is the whole point of the limit.
pub fn start_config(words: u32, default_limit: Option<u32>) -> yap_core::session::SessionConfig {
    let limit = default_limit.map(|max| HardLimit { max });
    yap_core::session::SessionConfig::new(GoalMode::Extend { base_words: words }, limit)
}

/// Whether a user may issue goal-changing commands.
///
/// A permission check belongs here rather than in the caller so every entry
/// point gets it. Off by default: most streamers would rather grant the bot
/// broad command access and not have a mod's `!yap stop` be ignored.
pub fn may_command(user: &str, allowed: &[String]) -> bool {
    if allowed.is_empty() {
        return true;
    }
    allowed.iter().any(|name| name.eq_ignore_ascii_case(user))
}

/// The reply to send back to chat.
///
/// Returns `None` when a command needs no public acknowledgement, so a
/// successful `!yap extend` does not spam chat.
pub fn reply(command: &Command, result: CommandOutcome) -> Option<String> {
    match (command, result) {
        (_, CommandOutcome::Error(message)) => Some(message),
        (Command::Status, CommandOutcome::Ok(status)) => Some(status),
        (Command::Ask { words }, CommandOutcome::Ok(_)) => {
            Some(format!("!yap please {words} — chat decide"))
        }
        _ => None,
    }
}

/// What happened when a command ran.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandOutcome {
    /// Done. The string is a status line where one is useful.
    Ok(String),
    /// Refused. The string is what to say in chat.
    Error(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_yap_is_status() {
        assert_eq!(parse("!yap"), Command::Status);
        assert_eq!(parse("!yap   "), Command::Status);
    }

    #[test]
    fn status_variants() {
        assert_eq!(parse("!yap status"), Command::Status);
        assert_eq!(parse("!YAP STATUS"), Command::Status);
    }

    #[test]
    fn extend_parses_a_count() {
        assert_eq!(parse("!yap extend 500"), Command::Extend { words: 500 });
        assert_eq!(parse("!yap add 500"), Command::Extend { words: 500 });
        assert_eq!(parse("!yap +500"), Command::Extend { words: 500 });
    }

    #[test]
    fn counts_accept_k_and_commas() {
        assert_eq!(parse("!yap extend 5k"), Command::Extend { words: 5_000 });
        assert_eq!(parse("!yap extend 5K"), Command::Extend { words: 5_000 });
        assert_eq!(
            parse("!yap extend 2m"),
            Command::Extend { words: 2_000_000 }
        );
        assert_eq!(parse("!yap extend 1,500"), Command::Extend { words: 1_500 });
    }

    #[test]
    fn a_longer_word_is_not_a_command() {
        // `!yapathon` is somebody's username, not a yapathon command.
        assert_eq!(parse("!yapathon"), Command::NotACommand);
        assert_eq!(parse("!yapper"), Command::NotACommand);
    }

    #[test]
    fn unrelated_messages_are_untouched() {
        assert_eq!(parse("hello chat"), Command::NotACommand);
        assert_eq!(parse("!up"), Command::NotACommand);
        assert_eq!(parse(""), Command::NotACommand);
    }

    #[test]
    fn a_missing_count_is_an_invalid_command_with_advice() {
        let Command::Invalid { reason } = parse("!yap extend") else {
            panic!("expected invalid");
        };
        assert!(reason.contains("!yap extend 500"), "got {reason}");
    }

    #[test]
    fn zero_and_garbage_counts_are_rejected() {
        // Extending by zero is a command that does nothing.
        assert!(matches!(parse("!yap extend 0"), Command::Invalid { .. }));
        assert!(matches!(
            parse("!yap extend banana"),
            Command::Invalid { .. }
        ));
        assert!(matches!(parse("!yap extend -5"), Command::Invalid { .. }));
    }

    #[test]
    fn an_absurd_count_does_not_overflow() {
        // `checked_mul` returning None must not wrap into a small goal.
        assert!(matches!(
            parse("!yap extend 9999999999m"),
            Command::Invalid { .. }
        ));
    }

    #[test]
    fn lifecycle_commands() {
        assert_eq!(parse("!yap pause"), Command::Pause);
        assert_eq!(parse("!yap resume"), Command::Resume);
        assert_eq!(parse("!yap stop"), Command::End);
        assert_eq!(parse("!yap undo"), Command::Undo);
        assert_eq!(parse("!yap oops"), Command::Undo);
    }

    #[test]
    fn an_unknown_verb_says_so() {
        let Command::Invalid { reason } = parse("!yap teleport") else {
            panic!("expected invalid");
        };
        assert!(reason.contains("teleport"), "got {reason}");
    }

    #[test]
    fn extend_becomes_a_priced_event() {
        let event = command_event(&Command::Extend { words: 500 }, Some("chatmod".to_string()))
            .expect("event");

        assert_eq!(event.words, Some(500));
        assert_eq!(event.platform, "chat");
        assert_eq!(event.user.as_deref(), Some("chatmod"));
    }

    #[test]
    fn non_goal_commands_produce_no_event() {
        // Pause and undo act on the session, not the goal. Emitting events for
        // them would pay out words for a pause.
        for command in [
            Command::Pause,
            Command::Resume,
            Command::End,
            Command::Undo,
            Command::Status,
        ] {
            assert_eq!(command_event(&command, None), None);
        }
    }

    #[test]
    fn a_chat_started_yapathon_still_has_a_limit() {
        // An unbounded chat-started run could never end on a busy channel.
        let config = start_config(5_000, Some(10_000));
        assert_eq!(config.mode, GoalMode::Extend { base_words: 5_000 });
        assert_eq!(config.hard_limit.map(|l| l.max), Some(10_000));
    }

    #[test]
    fn an_empty_allowlist_permits_everyone() {
        assert!(may_command("anyone", &[]));
    }

    #[test]
    fn an_allowlist_is_case_insensitive() {
        let allowed = vec!["ModName".to_string()];
        assert!(may_command("modname", &allowed));
        assert!(!may_command("someone", &allowed));
    }

    #[test]
    fn only_ask_and_status_get_a_reply() {
        // A successful extend should not spam chat; a refusal always should.
        assert_eq!(
            reply(
                &Command::Ask { words: 250 },
                CommandOutcome::Ok(String::new())
            ),
            Some("!yap please 250 — chat decide".to_string())
        );

        assert_eq!(
            reply(
                &Command::Extend { words: 500 },
                CommandOutcome::Ok(String::new())
            ),
            None
        );

        assert_eq!(
            reply(
                &Command::Extend { words: 500 },
                CommandOutcome::Error("no yapathon running".to_string())
            ),
            Some("no yapathon running".to_string())
        );
    }
}
