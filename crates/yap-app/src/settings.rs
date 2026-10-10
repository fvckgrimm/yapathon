//! The settings schema.
//!
//! Every editable setting is described once here: key, label, kind, and where
//! it appears. Both the GPUI panel and the documentation read this, so a setting
//! cannot exist in one and be missing from the other.
//!
//! A `Field` has no value. It says what a setting *is*; the current value lives
//! in [`crate::settings_view::SettingsModel`], which is what lets the panel hold
//! unsaved edits without touching the file.

/// What kind of control a setting needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// A toggle.
    Bool,
    /// Free text.
    Text,
    /// A whole number.
    Int,
    /// A filesystem path.
    Path,
    /// One of a fixed set.
    Choice,
    /// One of a fixed set the machine supplies at runtime.
    ///
    /// Separate from [`FieldKind::Choice`] because the options are not known
    /// until the app is running: a microphone can be plugged in, unplugged, or
    /// renamed between launches, and a list baked into a `const` would be a list
    /// of last year's hardware. Nothing in the schema can enumerate them, so
    /// this is the one kind whose options come from somewhere else.
    ChoiceFromDevices,
    /// A credential. Never rendered as a value.
    Secret,
}

impl FieldKind {
    /// Whether this kind is edited by typing into an input.
    ///
    /// The panel builds one input per textual field, and only these kinds get
    /// one. Keeping the predicate here rather than in the view means a new kind
    /// cannot be added without deciding how it is edited.
    pub fn is_textual(self) -> bool {
        matches!(self, Self::Text | Self::Int | Self::Path)
    }

    /// What the input shows when the field is empty.
    pub fn placeholder(self) -> &'static str {
        match self {
            Self::Path => "path to a model file",
            Self::Int => "number",
            _ => "",
        }
    }
}

/// Whether a kind is edited by typing.
pub fn is_textual(kind: FieldKind) -> bool {
    kind.is_textual()
}

/// Whether a kind's options come from a fixed list in the schema.
pub fn has_fixed_options(kind: FieldKind) -> bool {
    matches!(kind, FieldKind::Choice)
}

/// One setting.
#[derive(Debug, Clone, Copy)]
pub struct Field {
    /// The settings.toml key. Also the element id, so a field needs no id of its
    /// own.
    pub key: &'static str,
    /// What the panel calls it.
    pub label: &'static str,
    pub kind: FieldKind,
    /// For [`FieldKind::Choice`]: the permitted values, first one the default.
    pub options: &'static [&'static str],
    /// A sentence explaining a setting whose effect is not obvious.
    pub help: Option<&'static str>,
}

/// A group of settings.
#[derive(Debug, Clone, Copy)]
pub struct Section {
    pub title: &'static str,
    pub fields: &'static [Field],
}

const ENGINE: &[Field] = &[
    Field {
        key: "engine",
        label: "Speech recognition",
        kind: FieldKind::Text,
        options: &[],
        help: Some("local whisper.cpp, or groq for a hosted one"),
    },
    Field {
        key: "model_path",
        label: "Model file",
        kind: FieldKind::Path,
        options: &[],
        help: Some("a ggml model, e.g. ggml-small.en.bin"),
    },
    Field {
        key: "engine_backend",
        label: "Compute",
        kind: FieldKind::Text,
        options: &[],
        help: Some("needs a GPU-enabled build; CPU works everywhere"),
    },
    Field {
        key: "groq_key",
        label: "Groq API key",
        kind: FieldKind::Secret,
        options: &[],
        help: None,
    },
];

const GOAL: &[Field] = &[
    Field {
        key: "default_mode",
        label: "Default mode",
        kind: FieldKind::Choice,
        options: &["extend", "fixed", "time"],
        help: Some("whether chat can move the finish line"),
    },
    Field {
        key: "default_hard_limit",
        label: "Hard limit",
        kind: FieldKind::Int,
        options: &[],
        help: Some("the most the goal can climb to; empty for no limit"),
    },
];

const COUNTING: &[Field] = &[
    Field {
        key: "expand_contractions",
        label: "Expand contractions",
        kind: FieldKind::Bool,
        options: &[],
        help: Some("count \"don't\" as two words, because it is said as two"),
    },
    Field {
        key: "spell_out_numerals",
        label: "Spell out numbers",
        kind: FieldKind::Bool,
        options: &[],
        help: Some("count \"42\" as \"forty two\", which is what was said"),
    },
    Field {
        key: "exclude_fillers",
        label: "Exclude fillers",
        kind: FieldKind::Bool,
        options: &[],
        help: Some("um, uh and the like do not count toward the goal"),
    },
    Field {
        key: "count_bare_numerals",
        label: "Count bare numerals",
        kind: FieldKind::Bool,
        options: &[],
        help: Some("only applies when spelling out is off"),
    },
    Field {
        key: "keep_transcript",
        label: "Keep transcript",
        kind: FieldKind::Bool,
        options: &[],
        help: Some("a local record of what was said; off makes counts disputable"),
    },
];

const VOICE: &[Field] = &[
    Field {
        key: "input_device",
        label: "Microphone",
        kind: FieldKind::ChoiceFromDevices,
        options: &[],
        help: Some(
            "which input to listen to; system default is fine unless you have more than one",
        ),
    },
    Field {
        key: "vad_min_silence_ms",
        label: "Pause that ends a thought",
        kind: FieldKind::Int,
        options: &[],
        help: Some("milliseconds; lower splits sentences more often"),
    },
    Field {
        key: "vad_threshold",
        label: "Speech threshold",
        kind: FieldKind::Text,
        options: &[],
        help: Some("0.0 to 1.0; lower hears more of the room"),
    },
];

const TWITCH: &[Field] = &[
    Field {
        key: "twitch_channel",
        label: "Channel",
        kind: FieldKind::Text,
        options: &[],
        help: Some("empty leaves chat commands as the only way to extend"),
    },
    Field {
        key: "twitch_client_id",
        label: "Application id",
        kind: FieldKind::Text,
        options: &[],
        help: Some("from dev.twitch.tv/console/apps"),
    },
    Field {
        key: "twitch_credential",
        label: "Token location",
        kind: FieldKind::Text,
        options: &[],
        help: Some("a keyring entry name, not the token itself"),
    },
    Field {
        key: "twitch_token",
        label: "User token",
        kind: FieldKind::Secret,
        options: &[],
        help: None,
    },
];

const SERVER: &[Field] = &[
    Field {
        key: "accent",
        label: "Accent colour",
        kind: FieldKind::Choice,
        options: &["teal", "green", "pink", "purple", "amber", "blue", "mono"],
        help: Some("tints the count and the highlights; applies on next launch"),
    },
    Field {
        key: "appearance",
        label: "Light or dark",
        kind: FieldKind::Choice,
        options: &["auto", "dark", "light"],
        help: Some("dark unless you ask otherwise"),
    },
    Field {
        key: "overlay_theme",
        label: "Overlay look",
        kind: FieldKind::Choice,
        options: &["card", "minimal", "cute", "neon", "mono"],
        help: Some(
            "what OBS shows; preview one at a time by adding ?theme=name to the \
             overlay URL in a browser",
        ),
    },
    Field {
        key: "bind_address",
        label: "Bind address",
        kind: FieldKind::Text,
        options: &[],
        help: Some("keep this on 127.0.0.1 unless you know why not"),
    },
    Field {
        key: "port",
        label: "Port",
        kind: FieldKind::Int,
        options: &[],
        help: Some("OBS points at this"),
    },
];

const EVENTS: &[Field] = &[
    Field {
        key: "event_twitch_sub",
        label: "New subscriber",
        kind: FieldKind::Int,
        options: &[],
        help: Some("words a first-time sub adds to the goal"),
    },
    Field {
        key: "event_twitch_resub",
        label: "Resubscribe",
        kind: FieldKind::Int,
        options: &[],
        help: Some("words someone renewing their sub adds"),
    },
    Field {
        key: "event_twitch_gift_sub",
        label: "Gifted sub (each)",
        kind: FieldKind::Int,
        options: &[],
        help: Some(
            "per sub gifted, so a hundred gifted subs are worth a hundred of these — \
             Twitch sends a gift bomb as one event carrying a quantity",
        ),
    },
    Field {
        key: "event_twitch_cheer",
        label: "Cheer (per bit)",
        kind: FieldKind::Int,
        options: &[],
        help: Some("100 bits is a dollar, so this is usually 1"),
    },
    Field {
        key: "event_twitch_raid",
        label: "Raider",
        kind: FieldKind::Int,
        options: &[],
        help: None,
    },
    Field {
        key: "event_twitch_redemption",
        label: "Channel redemption",
        kind: FieldKind::Int,
        options: &[],
        help: None,
    },
    Field {
        key: "event_chat_command",
        label: "Chat !command",
        kind: FieldKind::Int,
        options: &[],
        help: Some("what typing a command in chat is worth"),
    },
];

/// Every setting, grouped.
pub const SETTINGS_SECTIONS: &[Section] = &[
    Section {
        title: "Engine",
        fields: ENGINE,
    },
    Section {
        title: "Events",
        fields: EVENTS,
    },
    Section {
        title: "Goal",
        fields: GOAL,
    },
    Section {
        title: "Counting",
        fields: COUNTING,
    },
    Section {
        title: "Voice",
        fields: VOICE,
    },
    Section {
        title: "Twitch",
        fields: TWITCH,
    },
    Section {
        title: "Server",
        fields: SERVER,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_key_is_unique() {
        // Two fields with one key means the panel shows the same value twice and
        // one of them is unreachable.
        let mut seen = HashSet::new();
        for section in SETTINGS_SECTIONS {
            for field in section.fields {
                assert!(
                    seen.insert(field.key),
                    "duplicate settings key: {}",
                    field.key
                );
            }
        }
    }

    #[test]
    fn every_section_has_at_least_one_field() {
        for section in SETTINGS_SECTIONS {
            assert!(
                !section.fields.is_empty(),
                "section {} is empty",
                section.title
            );
        }
    }

    #[test]
    fn every_choice_has_options_and_other_kinds_do_not() {
        for section in SETTINGS_SECTIONS {
            for field in section.fields {
                match field.kind {
                    FieldKind::Choice => assert!(
                        !field.options.is_empty(),
                        "{} is a choice with no options",
                        field.key
                    ),
                    FieldKind::ChoiceFromDevices => assert!(
                        field.options.is_empty(),
                        "{} declares options; a device list comes from the machine",
                        field.key
                    ),
                    _ => assert!(
                        field.options.is_empty(),
                        "{} declares options but is not a choice",
                        field.key
                    ),
                }
            }
        }
    }

    #[test]
    fn a_secret_field_carries_no_value() {
        // A secret that carried a value would end up in settings.toml, which is
        // the one file on a streaming machine that gets synced and pasted into
        // support threads. The check is that no field's key reads like somewhere
        // to put the value — `Field` has nowhere to put one, which is the point.
        for section in SETTINGS_SECTIONS {
            for field in section.fields {
                if field.kind == FieldKind::Secret {
                    assert!(
                        !field.key.ends_with("_value"),
                        "{} looks like it stores the secret inline",
                        field.key
                    );
                }
            }
        }
    }

    #[test]
    fn secrets_exist_so_they_are_reachable_at_all() {
        // A settings screen with no way to enter a token is a settings screen
        // that cannot be completed.
        let secrets: Vec<&str> = SETTINGS_SECTIONS
            .iter()
            .flat_map(|s| s.fields.iter())
            .filter(|f| f.kind == FieldKind::Secret)
            .map(|f| f.key)
            .collect();

        assert!(
            secrets.contains(&"twitch_token"),
            "there must be a way to set the Twitch token"
        );
        assert!(
            secrets.contains(&"groq_key"),
            "there must be a way to set the cloud engine's key"
        );
    }
}
