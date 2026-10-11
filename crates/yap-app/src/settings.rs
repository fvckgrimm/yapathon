//! The settings schema.
//!
//! Every editable setting is described once here: key, label, kind, and where
//! it appears. Both the GPUI panel and the documentation read this, so a setting
//! cannot exist in one and be missing from the other.
//!
//! A `Field` has no value. It says what a setting *is*; the current value lives
//! in [`crate::settings_view::SettingsModel`], which is what lets the panel hold
//! unsaved edits without touching the file.

use gpui_kit::assets::IconName;

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
    /// The section's symbol.
    ///
    /// Eleven sections in a wrapping row of small buttons is a wall of text
    /// that has to be read rather than recognised, and the icons are what make
    /// it scannable — the goal one and the counting one are otherwise the same
    /// two words long.
    ///
    /// A name rather than an image, resolved through [`crate::icons::Assets`].
    /// Nothing about naming an icon guarantees its bytes exist, so the list in
    /// `icons.rs` has a test that checks every icon named here is served.
    pub icon: IconName,
    pub fields: &'static [Field],
}

const VOICE: &[Field] = &[
    Field {
        key: "input_device",
        label: "Microphone",
        kind: FieldKind::ChoiceFromDevices,
        options: &[],
        help: Some("Usually just leave this on System default"),
    },
    Field {
        key: "model_path",
        label: "Voice model",
        kind: FieldKind::Path,
        options: &[],
        help: Some("Pick one below — you don't need to type a path"),
    },
    Field {
        key: "engine",
        label: "Voice engine",
        kind: FieldKind::Text,
        options: &[],
        help: Some("Local is free and private. Groq needs internet + a key"),
    },
    Field {
        key: "engine_backend",
        label: "Speed",
        kind: FieldKind::Text,
        options: &[],
        help: Some("Auto is fine. Only change this if counting feels slow"),
    },
    Field {
        key: "groq_key",
        label: "Groq key (only if you use Groq)",
        kind: FieldKind::Secret,
        options: &[],
        help: None,
    },
];

const YAPATHON: &[Field] = &[
    Field {
        key: "default_mode",
        label: "How chat affects the goal",
        kind: FieldKind::Choice,
        options: &["extend", "fixed", "time"],
        help: Some("Extend = chat grows the goal. Fixed = goal never moves. Time = chat adds clock"),
    },
    Field {
        key: "default_hard_limit",
        label: "Biggest the goal can get",
        kind: FieldKind::Int,
        options: &[],
        help: Some("Safety cap so chat can't grow it forever. Empty = no cap"),
    },
    Field {
        key: "expand_contractions",
        label: "Count “don't” as 2 words",
        kind: FieldKind::Bool,
        options: &[],
        help: None,
    },
    Field {
        key: "spell_out_numerals",
        label: "Count “42” as “forty two”",
        kind: FieldKind::Bool,
        options: &[],
        help: None,
    },
    Field {
        key: "exclude_fillers",
        label: "Ignore ums and uhs",
        kind: FieldKind::Bool,
        options: &[],
        help: None,
    },
    Field {
        key: "count_bare_numerals",
        label: "Count lone numbers",
        kind: FieldKind::Bool,
        options: &[],
        help: Some("Only matters if spelling-out above is off"),
    },
    Field {
        key: "keep_transcript",
        label: "Keep what was said",
        kind: FieldKind::Bool,
        options: &[],
        help: Some("Lets you settle disputes later. Stored only on this PC"),
    },
];

const CHAT_SETUP: &[Field] = &[
    Field {
        key: "twitch_channel",
        label: "Your Twitch channel",
        kind: FieldKind::Text,
        options: &[],
        help: Some("Empty = chat can't grow the goal yet, talking still counts"),
    },
    Field {
        key: "twitch_client_id",
        label: "Twitch app ID",
        kind: FieldKind::Text,
        options: &[],
        help: Some("From dev.twitch.tv/console/apps — one-time copy-paste"),
    },
    Field {
        key: "twitch_token",
        label: "Twitch login",
        kind: FieldKind::Secret,
        options: &[],
        help: None,
    },
    Field {
        key: "astro_youtube_token",
        label: "YouTube connection",
        kind: FieldKind::Secret,
        options: &[],
        help: Some("Via StreamElements. Empty = YouTube off"),
    },
    Field {
        key: "astro_twitch_token",
        label: "Twitch via StreamElements",
        kind: FieldKind::Secret,
        options: &[],
        help: Some("Only needed if you stream to both sites at once"),
    },
];

const LOOK: &[Field] = &[
    Field {
        key: "accent",
        label: "Color",
        kind: FieldKind::Choice,
        options: &["teal", "green", "pink", "purple", "amber", "blue", "mono"],
        help: Some("Just the highlight color. Teal is fine"),
    },
    Field {
        key: "appearance",
        label: "Light or dark",
        kind: FieldKind::Choice,
        options: &["auto", "dark", "light"],
        help: Some("Auto follows your PC"),
    },
    Field {
        key: "overlay_theme",
        label: "Overlay colors",
        kind: FieldKind::Choice,
        options: &[
            "card",
            "minimal",
            "cute",
            "neon",
            "mono",
            "catppuccin-mocha",
            "catppuccin-macchiato",
        ],
        help: Some("What chat sees on stream"),
    },
    Field {
        key: "overlay_style",
        label: "Overlay size",
        kind: FieldKind::Choice,
        options: &["corner", "slim", "strip", "big"],
        help: Some("Corner is the safe default"),
    },
];

const ADVANCED: &[Field] = &[
    Field {
        key: "twitch_credential",
        label: "Twitch login slot",
        kind: FieldKind::Text,
        options: &[],
        help: Some("Leave this alone unless you run two channels"),
    },
    Field {
        key: "vad_min_silence_ms",
        label: "Pause length (advanced)",
        kind: FieldKind::Int,
        options: &[],
        help: Some("Lower = splits sentences more. 500 is fine"),
    },
    Field {
        key: "vad_threshold",
        label: "Mic sensitivity (advanced)",
        kind: FieldKind::Text,
        options: &[],
        help: Some("0 to 1. Lower hears more room noise"),
    },
    Field {
        key: "bind_address",
        label: "Network (advanced)",
        kind: FieldKind::Text,
        options: &[],
        help: Some("Keep 127.0.0.1"),
    },
    Field {
        key: "port",
        label: "Port (advanced)",
        kind: FieldKind::Int,
        options: &[],
        help: Some("OBS points here. Only change if something else uses it"),
    },
];

/// An event price field.
///
/// A `const fn` rather than a spelled-out `Field` twenty times, because the
/// shape is identical for every event and the only things that differ are the
/// three strings.
const fn event_field(key: &'static str, label: &'static str, help: &'static str) -> Field {
    Field {
        key,
        label,
        kind: FieldKind::Text,
        options: &[],
        help: Some(help),
    }
}

/// All chat rewards in one place. Empty = off.
/// Presets at the top set sane defaults — most people never touch the rest.
const REWARDS: &[Field] = &[
    event_field("event_twitch_follow", "Twitch: new follower", "each time"),
    event_field("event_twitch_sub", "Twitch: new sub", "first month"),
    event_field("event_twitch_resub", "Twitch: resub", "every month after the first"),
    event_field("event_twitch_gift_sub", "Twitch: gifted sub (each)", "a 10-pack counts 10x"),
    event_field("event_twitch_cheer", "Twitch: cheer (per bit)", "1 per bit is normal"),
    event_field("event_twitch_raid", "Twitch: raid (per person)", "10 raiders = 10x"),
    event_field("event_twitch_host", "Twitch: host (per viewer)", "same as a raid"),
    event_field("event_twitch_redemption", "Twitch: channel point redeem", "each one"),
    event_field("event_twitch_tip", "Twitch: tip (per $1)", "blank if no tips"),
    event_field("event_twitch_merch", "Twitch: merch buy", "each order"),
    event_field("event_youtube_subscriber", "YouTube: new subscriber", "needs YouTube connection"),
    event_field("event_youtube_membership", "YouTube: new member", "paid, not Subscribe"),
    event_field("event_youtube_superchat", "YouTube: Super Chat (per $1)", "what they paid"),
    event_field("event_youtube_tip", "YouTube: tip (per $1)", "what they paid"),
    event_field("event_youtube_merch", "YouTube: merch buy", "each order"),
    event_field("event_kick_follow", "Kick: new follower", "each one"),
    event_field("event_kick_sub", "Kick: new sub", "each one"),
    event_field("event_kick_kicks", "Kick: KICKs gift (each)", "scales with gift"),
    event_field("event_kick_tip", "Kick: tip (per $1)", "what they paid"),
    event_field("event_chat_command", "!yap in chat", "each time typed — great for testing"),
];

/// Every setting, grouped for normal humans: 6 tabs, not 11.
pub const SETTINGS_SECTIONS: &[Section] = &[
    Section {
        title: "Voice",
        icon: IconName::Mic,
        fields: VOICE,
    },
    Section {
        title: "Yapathon",
        icon: IconName::Target,
        fields: YAPATHON,
    },
    Section {
        title: "Chat setup",
        icon: IconName::Radio,
        fields: CHAT_SETUP,
    },
    Section {
        title: "Rewards",
        icon: IconName::Gift,
        fields: REWARDS,
    },
    Section {
        title: "On stream",
        icon: IconName::Palette,
        fields: LOOK,
    },
    Section {
        title: "Advanced",
        icon: IconName::Settings,
        fields: ADVANCED,
    },
];

/// The keyring entry a credential field's value belongs in.
///
/// `(service, user)` — the two halves the store files an entry under, either of
/// which is enough to address nothing. `user` is fixed by what the credential
/// *is*; `service` comes from settings, which is what lets one machine hold two
/// Twitch channels.
///
/// Returns `None` for a field that is not a credential, so a caller cannot
/// accidentally prompt for a setting and store it under `user-token`.
///
/// # Why this exists
///
/// The credential prompt and the platform loaders originally used different
/// addresses — the prompt wrote under a service derived from the field name, the
/// loaders read under `twitch_credential`. The prompt reported success and the
/// platform found nothing, which is exactly the shape of every other bug this
/// project has had: everything said it worked.
///
/// One function, so a field cannot be promptable at one address and read at
/// another. The test for that is in `settings_view`, and it goes through the
/// real loader rather than through this mapping.
pub fn credential_entry(
    settings: &yap_core::settings::Settings,
    field: &Field,
) -> Option<(String, &'static str)> {
    if field.kind != FieldKind::Secret {
        return None;
    }

    let service = match field.key {
        "twitch_token" => settings
            .twitch_credential
            .clone()
            .unwrap_or_else(|| yap_core::settings::credential_name("twitch")),
        "astro_youtube_token" | "astro_twitch_token" => {
            yap_core::settings::credential_name("astro")
        }
        "groq_key" => match &settings.engine {
            yap_core::settings::EngineChoice::Groq { credential } => credential.clone(),
            // The engine is local, but the key is still worth keeping — a
            // streamer may switch engines and would otherwise have to find it
            // again. The default address is the one the cloud engine uses.
            _ => yap_core::settings::credential_name("groq"),
        },
        // An unrecognised credential field gets the conventional address rather
        // than no prompt at all. A secret the app cannot place is worse than
        // one placed somewhere a future loader can be told to look.
        _ => yap_core::settings::credential_name(field.key),
    };

    Some((
        service,
        match field.key {
            "groq_key" => "api-key",
            // One entry per platform, so clearing one account's token leaves
            // the other's alone — and so the panel can show whether each is set.
            "astro_youtube_token" => "youtube",
            "astro_twitch_token" => "twitch",
            _ => "user-token",
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AssetSource;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn every_section_icon_is_served_and_no_two_sections_share_one() {
        // Two properties of the icons, checked together because they fail the
        // same way: a settings section whose icon never draws looks exactly like
        // one that does, until someone notices a gap where a symbol should be.
        //
        // *Served* because `IconName` is a name and nothing about it says the
        // bytes exist — that half is the gap `crate::icons` documents.
        //
        // *Distinct* because an icon repeated across sections spends the one
        // thing it was added for. Events · Twitch and Twitch are already close
        // enough in words; giving them the same glyph would make them harder to
        // tell apart, not easier.
        let mut seen: HashMap<IconName, &'static str> = HashMap::new();

        for section in SETTINGS_SECTIONS {
            let path = section.icon.path();

            assert!(
                crate::icons::Assets
                    .load(&path)
                    .unwrap_or_else(|error| panic!("{}: {error:#}", section.title))
                    .is_some(),
                "{}: {:?} is named but not served — add it to the icon_assets! \
                 list in icons.rs",
                section.title,
                section.icon,
            );

            if let Some(previous) = seen.insert(section.icon, section.title) {
                panic!(
                    "{} and {previous} share {:?}, which spends the icon on both",
                    section.title, section.icon,
                );
            }
        }
    }

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
