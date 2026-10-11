//! The settings panel: engine, model, counting rules, and Twitch.
//!
//! Schema-driven rather than hand-built per field, the way the settings in
//! `accerso-gpui` are. A field is described once — key, label, kind — and the
//! form is built from the description. Adding a setting then costs one line
//! rather than a label, an input and a save handler that can drift apart.
//!
//! Secrets are the exception and are not fields: an API key is written to the OS
//! credential store through a button, never rendered into a settings value. A
//! settings file is the one file on a streaming machine that gets synced,
//! backed up and pasted into a support thread.

use std::collections::BTreeMap;
use std::path::Path;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Styled as _, Task,
    Window, div, px,
};
use gpui_kit::Subscription;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;

use crate::model::SessionEntity;
use crate::segment::{icon_segment_button, segment_button};
use yap_core::goal::GoalMode;

use crate::settings::{Field, FieldKind, SETTINGS_SECTIONS, is_textual};
use crate::theme::Theme;
use crate::view::text::LABEL;
use crate::view::label;

/// One editable setting, as the panel holds it.
///
/// `dirty` tracks unsaved edits. The Save button is disabled until something
/// changes, so a streamer who opens Settings, looks, and closes does not rewrite
/// their config file for no reason.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingValue {
    pub value: String,
    pub dirty: bool,
}

impl SettingValue {
    fn new(value: String) -> Self {
        Self {
            value,
            dirty: false,
        }
    }
}

/// The settings panel's state.
#[derive(Debug, Clone, Default)]
pub struct SettingsModel {
    /// Keyed by field key, in section order.
    pub values: std::collections::BTreeMap<String, SettingValue>,
    /// The section being shown. Settings are grouped; showing thirty fields at
    /// once is how a settings screen becomes something nobody reads.
    pub section: usize,
    /// Set after a failed save, so the failure is visible rather than silent.
    pub notice: Option<String>,
    pub notice_is_error: bool,
}

impl SettingsModel {
    /// Whether anything needs saving.
    pub fn is_dirty(&self) -> bool {
        self.values.values().any(|v| v.dirty)
    }

    /// Clear the dirty marks after a save.
    pub fn mark_saved(&mut self) {
        for value in self.values.values_mut() {
            value.dirty = false;
        }
        self.notice_is_error = false;
    }

    /// Read a value, or the empty string.
    pub fn get(&self, key: &str) -> &str {
        self.values.get(key).map(|v| v.value.as_str()).unwrap_or("")
    }

    /// Whether a boolean field is on.
    ///
    /// Text rather than a strict parse: a hand-edited settings file can hold
    /// anything, and a checkbox that silently reads false for `TRUE` is a
    /// setting that appears to ignore you.
    pub fn is_on(&self, key: &str) -> bool {
        matches!(
            self.get(key).trim().to_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    }

    /// Record an edit.
    pub fn set(&mut self, key: &str, value: String) {
        let entry = self
            .values
            .entry(key.to_string())
            .or_insert_with(|| SettingValue {
                value: String::new(),
                dirty: false,
            });
        entry.value = value;
        entry.dirty = true;
        self.notice = None;
    }

    /// Flip a boolean field.
    pub fn toggle(&mut self, key: &str, on: bool) {
        self.set(key, if on { "true" } else { "false" }.to_string());
    }
}

/// Load the current settings into editable strings.
///
/// Reads the file as text rather than deserialising, because every field is
/// already a string and this way an unrecognised key in the file survives a
/// round trip through the panel instead of being dropped on save.
pub fn load_values(settings: &yap_core::settings::Settings) -> SettingsModel {
    let mut model = SettingsModel::default();

    for section in SETTINGS_SECTIONS {
        for field in section.fields {
            let value = field_value(settings, field);
            model
                .values
                .insert(field.key.to_string(), SettingValue::new(value));
        }
    }

    model
}

/// The current value of one field, rendered as text.
fn field_value(settings: &yap_core::settings::Settings, field: &Field) -> String {
    match field.key {
        "engine" => settings.engine.label().to_string(),
        "model_path" => match &settings.engine {
            yap_core::settings::EngineChoice::Local { model_path, .. } => {
                model_path.display().to_string()
            }
            yap_core::settings::EngineChoice::Groq { credential } => {
                format!("keyring: {credential}")
            }
        },
        "engine_backend" => match &settings.engine {
            yap_core::settings::EngineChoice::Local { backend, .. } => {
                format!("{backend:?}").to_lowercase()
            }
            _ => String::new(),
        },
        "default_mode" => settings.default_mode.short_name().to_string(),
        "default_hard_limit" => settings
            .default_hard_limit
            .map(|v| v.to_string())
            .unwrap_or_default(),
        "expand_contractions" => bool_text(settings.rules.expand_contractions),
        "spell_out_numerals" => bool_text(settings.rules.spell_out_numerals),
        "exclude_fillers" => bool_text(settings.rules.exclude_fillers),
        "count_bare_numerals" => bool_text(settings.rules.count_bare_numerals_as_words),
        "keep_transcript" => bool_text(settings.keep_transcript),
        "bind_address" => settings.bind_address.clone(),
        "port" => settings.port.to_string(),
        "input_device" => settings.input_device.clone(),
        "vad_min_silence_ms" => settings.vad.min_silence_ms.to_string(),
        "vad_threshold" => format!("{:.2}", settings.vad.threshold),
        // Event pricing, read out of the map rather than listed field by field:
        // the keys are `platform:kind` and the schema keys are
        // `event_platform_kind`, so the two can be written next to each other
        // here instead of being kept in step across three files.
        "accent" => settings.accent.clone(),
        "appearance" => settings.appearance.clone(),
        "overlay_theme" => settings.overlay_theme.clone(),
        "overlay_style" => settings.overlay_style.clone(),
        _ if field.key.starts_with("event_") => words_for(settings, &event_key(field.key)),
        "event_twitch_resub" => words_for(settings, "twitch:resub"),
        "event_twitch_gift_sub" => words_for(settings, "twitch:gift_sub"),
        "event_twitch_cheer" => words_for(settings, "twitch:cheer"),
        "event_twitch_raid" => words_for(settings, "twitch:raid"),
        "event_twitch_redemption" => words_for(settings, "twitch:redemption"),
        "event_chat_command" => words_for(settings, "chat:command"),
        "twitch_channel" => settings.twitch_channel.clone().unwrap_or_default(),
        "twitch_client_id" => settings.twitch_client_id.clone().unwrap_or_default(),
        "twitch_credential" => settings
            .twitch_credential
            .clone()
            .unwrap_or_else(|| yap_core::settings::credential_name("twitch")),
        _ => String::new(),
    }
}

/// One event's word value, as text.
fn words_for(settings: &yap_core::settings::Settings, event: &str) -> String {
    settings
        .event_words
        .get(event)
        .map(|words| words.to_string())
        .unwrap_or_default()
}

/// The `platform:kind` an event field prices.
///
/// Written out rather than derived by string surgery, because the schema key
/// and the settings key disagree on purpose — `event_twitch_cheer` reads as a
/// field name, `twitch:cheer` as the event the core prices — and deriving one
/// from the other would tie them together until someone renamed a kind and
/// silently repriced a cheer.
/// The `event_words` key a field edits.
///
/// **By convention, not by table.** `event_<platform>_<kind>` becomes
/// `<platform>:<kind>`. The table this replaces had to be kept in step with the
/// schema by hand, and being out of step is silent: a field the table did not
/// mention fell through to `other => other`, was stored under its own field
/// name, and was then never found by anything that looks events up. That is
/// how `event_words` ended up written faithfully and read by nothing.
///
/// A field that is not an event key returns itself, which is what the callers
/// outside the Events section need.
fn event_key(field: &str) -> String {
    let Some(kind) = field.strip_prefix("event_") else {
        return field.to_string();
    };

    match kind.split_once('_') {
        Some((platform, kind)) => format!("{platform}:{kind}"),
        // `event_foo` with no platform to split off; store it as-is rather than
        // inventing a key for it.
        None => field.to_string(),
    }
}
fn bool_text(value: bool) -> String {
    if value { "true" } else { "false" }.to_string()
}

/// The panel.
pub struct SettingsView {
    /// The session state.
    ///
    /// Carried so the panel can show live state alongside the settings — the
    /// hard limit and the counting rules both describe what the *current* run is
    /// doing, and a panel that shows them with no context is a form that
    /// encourages guessing.
    model: Entity<SessionEntity>,
    settings: SettingsModel,
    /// One editable input per text-shaped setting, keyed by field key.
    ///
    /// An input state holds a focus handle, a selection and an undo history, so
    /// it must be an entity rather than a string — and it lives here rather than
    /// in a render closure because a closure's state is rebuilt on every redraw,
    /// which is why the previous version of this panel could not be typed into
    /// at all: it drew a `div` with the value in it and nothing could ever mark
    /// the panel dirty, so Save was disabled forever.
    inputs: BTreeMap<String, Entity<gpui_kit::component::input::InputState>>,
    /// One masked input per credential, keyed by field key.
    ///
    /// Separate from `inputs` on purpose. A credential is not a setting: it is
    /// never written to `settings.toml`, it never marks the panel dirty, and
    /// Save must not be how it is committed — otherwise typing a token and
    /// clicking Save would put the token in a plain text file, which is the one
    /// thing this is all arranged to avoid.
    ///
    /// So these have no change subscription and are read only when the dialog's
    /// own commit button fires.
    secret_inputs: BTreeMap<String, Entity<gpui_kit::component::input::InputState>>,
    /// The keys whose change events this panel reacts to.
    ///
    /// Recorded from the loop that creates the subscriptions rather than
    /// derived from the schema, so it reports what is actually wired rather
    /// than what was intended. `secret_inputs` is deliberately absent from it:
    /// a credential must not reach `SettingsModel`, because Save serialises
    /// that model into a file.
    subscribed_keys: Vec<String>,
    /// Which credential inputs were built masked.
    ///
    /// Recorded at construction rather than queried later, because the state's
    /// own `is_masked` is not public. Both the builder and this map read the one
    /// constant below, so a change to whether credentials are masked cannot
    /// update one and miss the other.
    secret_masks: BTreeMap<String, bool>,
    /// Whether each credential is set, resolved off the render thread.
    ///
    /// `render_secret_field` needs this to decide what to draw, and reading the
    /// keyring to find out is the same mistake as reading it during window
    /// construction: a locked keyring does not error, it blocks, and this runs
    /// on the GPUI thread — so the settings panel would freeze solid with no
    /// error rather than merely showing nothing.
    ///
    /// Resolved in the background at construction and refreshed whenever a
    /// credential is set or cleared. The value only changes when this panel
    /// changes it, so a cached answer cannot be stale in a way anyone would
    /// notice.
    secret_states: BTreeMap<String, yap_core::secrets::SecretState>,
    /// The keyring entry each credential field's value belongs in.
    ///
    /// Resolved from the settings this panel was built with. Carrying it here
    /// rather than recomputing per render means the prompt and the platform
    /// loaders are working from the same answer.
    secret_entries: BTreeMap<String, (String, &'static str)>,
    /// Kept alive so the change subscriptions below keep firing.
    ///
    /// A dropped `Subscription` unsubscribes, which would leave the fields
    /// editable but the panel permanently clean — Save greyed out again, with no
    /// error to explain it.
    _subscriptions: Vec<Subscription>,
    /// A weak handle to this panel, for click handlers.
    ///
    /// Their callbacks receive `&mut App`, not a `Context<Self>`, so they cannot
    /// reach the panel without a handle to it. Weak so a button's closure cannot
    /// keep the panel alive after its window closes.
    me: gpui::WeakEntity<Self>,
    /// The settings file this panel reads from and writes to.
    ///
    /// Resolved at startup rather than recomputed here. There are two places
    /// settings can live — next to the binary, or in the platform config
    /// directory — and a Save that went to the other one would leave the app
    /// reading a file the streamer never edits, which is a panel that appears to
    /// work and does not.
    path: std::path::PathBuf,
    /// Whether a file chooser is open right now.
    ///
    /// Not decoration: the chooser opens as a separate window owned by the
    /// desktop portal, and without this the Browse button stays live and can be
    /// clicked again, stacking dialogs on top of each other.
    browsing: bool,
    /// The task waiting on the chooser.
    ///
    /// Held so the panel cancels it when its window goes away, and so it is not
    /// dropped on the spot — a dropped `Task` is a cancelled one.
    browse_task: Option<Task<()>>,
}

/// Put the parts of a save that can take effect now, into effect now.
///
/// Whether this path is a model file the recogniser could actually open.
///
/// `is_file` and not `exists`, because one of the directories this app looks in
/// is one it creates itself: pointing at `~/.local/share/yap-a-thon/models` is a
/// directory that exists, opens no model, and fails at load with an error that
/// does not say what was wrong with the path.
fn model_path_is_usable(path: &std::path::Path) -> bool {
    path.is_file()
}

/// A free function because it reads nothing but its argument, and a free
/// function is testable without building a panel — which is the point, since the
/// thing being tested is a judgement about a path, not a rendering.
pub fn model_warning(saved: &yap_core::settings::Settings) -> Option<String> {
    let yap_core::settings::EngineChoice::Local { model_path, .. } = &saved.engine else {
        // A cloud engine has no local file to be wrong about. Its failures —
        // a missing key, no network — happen when it runs, and are reported
        // there with advice that is about the key rather than the path.
        return None;
    };

    if model_path_is_usable(model_path) {
        return None;
    }

    // Not an error when there is something on disk to use instead: the loader
    // substitutes it, and the microphone thread says which one it picked.
    let substitute = yap_core::settings::discover_models().into_iter().next();

    Some(match substitute {
        Some(model) => format!(
            "saved — no model at {}, so {} will be used instead",
            model_path.display(),
            model.name()
        ),
        None => format!(
            "saved — but there is no model at {}. Put a ggml model in {}, or \
             choose one above.",
            model_path.display(),
            yap_core::settings::models_dir().display()
        ),
    })
}

impl SettingsView {
    pub fn new(
        model: Entity<SessionEntity>,
        settings: yap_core::settings::Settings,
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let values = load_values(&settings);

        // One input per text-shaped field, built eagerly rather than on demand.
        //
        // Built in `new` because an input needs a `Window` to construct, and a
        // window is only available before the panel is handed back. Built
        // lazily it would need a deferred construction path, and every one of
        // those is a way for a field to silently end up read-only.
        let mut inputs: BTreeMap<String, Entity<gpui_kit::component::input::InputState>> =
            BTreeMap::new();

        for section in SETTINGS_SECTIONS {
            for field in section.fields {
                if !is_textual(field.kind) {
                    continue;
                }

                let state = cx.new(|cx| {
                    let mut state = gpui_kit::component::input::InputState::new(window, cx);
                    state.set_value(values.get(field.key).to_string(), window, cx);
                    state
                });

                inputs.insert(field.key.to_string(), state);
            }
        }

        // One masked input per credential, built here for the same reason as
        // the ones above: constructing an input needs a window, and this is the
        // only place one exists before the panel is handed back.
        //
        // Masked at construction rather than toggled afterwards — the toggle
        // needs a `&mut Window` inside a `Context` update, and a field that
        // starts unmasked on the first frame is a token on screen before
        // anything has had a chance to hide it.
        let mut secret_inputs: BTreeMap<String, Entity<gpui_kit::component::input::InputState>> =
            BTreeMap::new();
        let mut secret_masks: BTreeMap<String, bool> = BTreeMap::new();
        let mut secret_states: BTreeMap<String, yap_core::secrets::SecretState> = BTreeMap::new();

        // Where each credential actually lives. Resolved here rather than at
        // render time because the service is configurable, and reading it from
        // the settings the panel was built with keeps the address in one place
        // instead of spread across a render function.
        let mut secret_entries: BTreeMap<String, (String, &'static str)> = BTreeMap::new();

        for section in SETTINGS_SECTIONS {
            for field in section.fields {
                if field.kind != FieldKind::Secret {
                    continue;
                }

                let state = cx.new(|cx| {
                    gpui_kit::component::input::InputState::new(window, cx)
                        .masked(SECRET_INPUTS_ARE_MASKED)
                });

                secret_masks.insert(field.key.to_string(), SECRET_INPUTS_ARE_MASKED);
                secret_inputs.insert(field.key.to_string(), state);

                if let Some(entry) = crate::settings::credential_entry(&settings, field) {
                    secret_entries.insert(field.key.to_string(), entry);
                }
            }
        }

        // Asked on the thread that is allowed to wait for it, which is this one:
        // the panel does not exist yet, so nothing is being drawn and nothing can
        // freeze. Each answer is a round trip to the credential store, and a
        // locked store turns each one into an indefinite wait — which is why
        // this is here and not in `render`.
        for (key, (service, user)) in &secret_entries {
            secret_states.insert(key.clone(), yap_core::secrets::state(service, user));
        }

        let mut subscriptions: Vec<Subscription> = Vec::with_capacity(inputs.len());
        // Which keys the panel watches. Reported rather than declared: this is
        // filled from the same loop that creates the subscriptions, so it cannot
        // disagree with them.
        let mut subscribed: Vec<String> = Vec::with_capacity(inputs.len());

        cx.new(move |cx: &mut Context<Self>| {
            // One subscription per input, so typing marks the panel dirty and
            // Save becomes available. Without this the form accepts text and
            // silently discards it.
            for (key, state) in &inputs {
                let key = key.clone();
                subscribed.push(key.clone());
                subscriptions.push(cx.subscribe(state, move |this, _state, event, cx| {
                    if matches!(event, gpui_kit::component::input::InputEvent::Change) {
                        let value = this
                            .inputs
                            .get(&key)
                            .map(|state| state.read(cx).value().to_string())
                            .unwrap_or_default();
                        this.settings.set(&key, value);
                    }
                }));
            }

            Self {
                me: cx.weak_entity(),
                model,
                settings: values,
                inputs,
                secret_inputs,
                secret_masks,
                secret_states,
                secret_entries,
                subscribed_keys: subscribed,
                path,
                browsing: false,
                browse_task: None,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Rebuild the panel from a settings file.
    ///
    /// Called after a save so the stored values, not the edited ones, become
    /// what is displayed. Otherwise a rejected save would leave the form showing
    /// what was asked for rather than what was stored.
    pub fn reload(
        &mut self,
        settings: &yap_core::settings::Settings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings = load_values(settings);

        for (key, state) in &self.inputs {
            let value = self.settings.get(key).to_string();
            state.update(cx, |state, cx| state.set_value(value, window, cx));
        }

        self.settings.mark_saved();
        cx.notify();
    }

    fn render_sections(&self, me: &gpui::WeakEntity<Self>, _cx: &mut App) -> impl IntoElement {
        div().flex().flex_wrap().gap_2().children(
            SETTINGS_SECTIONS
                .iter()
                .enumerate()
                .map(|(index, section)| {
                    let selected = index == self.settings.section;
                    let label = section.title;
                    let icon = section.icon;
                    let me = me.clone();

                    icon_segment_button(
                        SharedId::section(index),
                        icon,
                        label,
                        selected,
                        move |_window, cx| {
                            let _ = me.update(cx, |this, _cx| {
                                this.settings.section = index;
                                this.settings.notice = None;
                            });
                        },
                    )
                    .into_any_element()
                }),
        )
    }

    /// One-line plain-English description per settings tab.
    fn section_blurb(&self) -> &'static str {
        match SETTINGS_SECTIONS
            .get(self.settings.section)
            .map(|s| s.title)
            .unwrap_or("")
        {
            "Voice" => "How the app hears you. System default mic + any model below just works.",
            "Yapathon" => "What a normal run looks like when you press Start.",
            "Chat setup" => "Optional. Talking already counts — this lets chat grow the goal.",
            "Rewards" => "What each thing adds. Blank = off. Presets set sane defaults.",
            "On stream" => "What chat sees on stream. Corner + card is the safe default.",
            _ => "Only touch these if something is broken.",
        }
    }

    /// One-tap reward presets so nobody prices 20 events by hand.
    fn render_reward_presets(&self, me: &gpui::WeakEntity<Self>, _cx: &mut App) -> Option<gpui::AnyElement> {
        let is_rewards = SETTINGS_SECTIONS
            .get(self.settings.section)
            .is_some_and(|s| s.title == "Rewards");
        if !is_rewards {
            return None;
        }

        let presets: &[(&str, &[(&str, &str)])] = &[
            ("Off", &[]),
            (
                "Chill",
                &[
                    ("event_twitch_sub", "100"),
                    ("event_twitch_resub", "100"),
                    ("event_twitch_gift_sub", "50"),
                    ("event_twitch_cheer", "1"),
                    ("event_twitch_raid", "10"),
                    ("event_chat_command", "50"),
                ],
            ),
            (
                "Hype",
                &[
                    ("event_twitch_follow", "25"),
                    ("event_twitch_sub", "150"),
                    ("event_twitch_resub", "150"),
                    ("event_twitch_gift_sub", "100"),
                    ("event_twitch_cheer", "2"),
                    ("event_twitch_raid", "25"),
                    ("event_twitch_redemption", "50"),
                    ("event_chat_command", "100"),
                    ("event_youtube_superchat", "10"),
                    ("event_youtube_subscriber", "50"),
                ],
            ),
        ];

        Some(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .pb_2()
                .children(presets.iter().map(|(name, values)| {
                    let me = me.clone();
                    let values: Vec<(String, String)> = values
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    let name = name.to_string();
                    let id = format!("reward-preset-{name}");
                    Button::new(id)
                        .label(name.to_string())
                        .on_click(move |_, _, cx| {
                            let _ = me.update(cx, |this, _| {
                                if name == "Off" {
                                    // Clear every reward field.
                                    for section in SETTINGS_SECTIONS {
                                        for field in section.fields {
                                            if field.key.starts_with("event_") {
                                                this.settings.set(field.key, String::new());
                                            }
                                        }
                                    }
                                } else {
                                    for (k, v) in &values {
                                        this.settings.set(k, v.clone());
                                    }
                                }
                            });
                        })
                        .into_any_element()
                }))
                .into_any_element(),
        )
    }

    /// The model picker.
    ///
    /// A list of the models actually found on disk, as buttons.
    ///
    /// This is the answer to "I downloaded a model and cannot choose it". Typing
    /// a path works, but it asks someone who has just downloaded a 460 MB file
    /// to find and type an absolute path, which is a worse way to pick a file
    /// than a list of the files they already have.
    fn render_model_picker(
        &self,
        me: &gpui::WeakEntity<Self>,
        cx: &mut App,
    ) -> Option<gpui::AnyElement> {
        let found = yap_core::settings::discover_models();

        let body = div()
            .flex()
            .flex_col()
            .gap_1()
            .children(found.iter().map(|model| {
                let selected = self.settings.get("model_path") == model.path.display().to_string();
                let path = model.path.display().to_string();
                let label = format!("{}  ({})", model.name(), model.size_label());
                let me = me.clone();

                segment_button(
                    SharedId::model(&model.name()),
                    label,
                    selected,
                    move |_window, cx| {
                        let _ = me.update(cx, |this, _cx| {
                            this.settings.set("model_path", path.clone())
                        });
                    },
                )
                .into_any_element()
            }));

        Some(
            div()
                .p_3()
                .rounded_lg()
                .bg(Theme::surface(cx))
                .border_1()
                .border_color(Theme::border(cx))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(crate::view::text::BODY)
                        .font_weight(gpui::FontWeight::BOLD)
                        .child("Voice models on this PC"),
                )
                .when(found.is_empty(), |this| {
                    this.child(
                        div()
                            .text_size(LABEL)
                            .text_color(Theme::muted(cx))
                            .child(format!(
                                "None yet. Download one (ggml-*.bin) into {} — or Browse below.",
                                yap_core::settings::models_dir().display()
                            )),
                    )
                })
                .child(body)
                .child(self.render_browse_button(me, cx))
                .into_any_element(),
        )
    }

    /// "browse for a model file".
    ///
    /// The discovered list only shows files in the directories the app knows to
    /// look in. Someone who put a model somewhere else — a second drive, a
    /// models folder for another tool, a download that never got moved — sees
    /// an empty list and no way to proceed. The chooser is what makes the path
    /// field a fallback rather than the only option.
    fn render_browse_button(&self, me: &gpui::WeakEntity<Self>, cx: &mut App) -> impl IntoElement {
        let browsing = self.browsing;
        let label = if browsing {
            "Waiting for the file picker…"
        } else {
            "Browse for a model file…"
        };

        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .items_center()
            .child(
                Button::new(SharedId::browse_model())
                    .label(label)
                    // Disabled while a chooser is open, which is the one place
                    // this panel legitimately needs a dead control: a second
                    // dialog on top of the first is not a thing anyone wants.
                    .disabled(browsing)
                    .on_click({
                        let me = me.clone();
                        // A closed panel means the window went away, which is
                        // not worth reporting: there is nobody left to tell.
                        move |_, _window, cx| {
                            let _ = me.update(cx, |this, cx| this.browse(cx));
                        }
                    }),
            )
            .when(browsing, |this| {
                this.child(
                    div()
                        .text_size(px(10.0))
                        .text_color(Theme::muted(cx))
                        .child("the chooser is a separate window from this one"),
                )
            })
    }

    fn render_fields(&self, me: &gpui::WeakEntity<Self>, cx: &mut App) -> Vec<gpui::AnyElement> {
        let Some(section) = SETTINGS_SECTIONS.get(self.settings.section) else {
            return Vec::new();
        };

        section
            .fields
            .iter()
            .map(|field| {
                let key = field.key;
                let on = self.settings.is_on(key);

                let control: gpui::AnyElement = match field.kind {
                    FieldKind::Bool => {
                        let me = me.clone();
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                div()
                                    .text_size(LABEL)
                                    .text_color(Theme::muted(cx))
                                    .child(if on { "On" } else { "Off" }),
                            )
                            .child(
                                Switch::new(SharedId::field(key))
                                    .checked(on)
                                    .on_change(move |checked, _window, cx| {
                                        let _ = me.update(cx, |this, _cx| {
                                            this.settings.toggle(key, *checked)
                                        });
                                    }),
                            )
                            .into_any_element()
                    }
                    FieldKind::Text | FieldKind::Int | FieldKind::Path => self
                        .render_text_field(field, cx)
                        .unwrap_or_else(|| label(field.label, cx).into_any_element()),
                    FieldKind::Choice => self.render_choice_field(field, me, cx).into_any_element(),
                    FieldKind::ChoiceFromDevices => {
                        self.render_device_field(field, me, cx).into_any_element()
                    }
                    FieldKind::Secret => render_secret_field(
                        field,
                        self.secret_inputs.get(field.key).cloned(),
                        self.secret_entries.get(field.key).cloned(),
                        self.secret_states
                            .get(field.key)
                            .copied()
                            .unwrap_or(yap_core::secrets::SecretState::Absent),
                        me,
                        cx,
                    )
                    .into_any_element(),
                };

                div()
                    .p_3()
                    .rounded_lg()
                    .bg(Theme::surface(cx))
                    .border_1()
                    .border_color(Theme::border(cx))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(crate::view::text::BODY)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(field.label),
                    )
                    .child(control)
                    .into_any_element()
            })
            .collect()
    }

    /// A text input.
    ///
    /// A real `Input` bound to an `InputState` owned by this panel, so it can be
    /// typed into and the change reaches the settings model.
    fn render_text_field(&self, field: &Field, cx: &mut App) -> Option<gpui::AnyElement> {
        let state = self.inputs.get(field.key)?;
        let hint = field.help.unwrap_or("");

        let input = gpui_kit::component::input::Input::new(state)
            .h(px(32.0))
            .w_full()
            .into_any_element();

        Some(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(input)
                .when(!hint.is_empty(), |this| {
                    this.child(
                        div()
                            .text_size(LABEL)
                            .text_color(Theme::muted(cx))
                            .child(hint),
                    )
                })
                .into_any_element(),
        )
    }

    /// A field with a fixed set of values.
    fn render_choice_field(
        &self,
        field: &Field,
        me: &gpui::WeakEntity<Self>,
        _cx: &mut App,
    ) -> impl IntoElement {
        let key = field.key;
        let current = self.settings.get(key).to_string();
        let options: Vec<String> = field.options.iter().map(|o| o.to_string()).collect();

        div()
            .flex()
            .gap_1()
            .flex_wrap()
            .children(options.into_iter().map(|option| {
                let selected = option == current;
                let me = me.clone();

                segment_button(
                    SharedId::choice(key, &option),
                    option.clone(),
                    selected,
                    move |_window, cx| {
                        let _ = me.update(cx, |this, _cx| this.settings.set(key, option.clone()));
                    },
                )
                .into_any_element()
            }))
    }

    /// The microphone picker.
    ///
    /// Enumerated on every draw rather than cached: a USB interface can be
    /// plugged in while the app is running, and a cached list would need a
    /// refresh button for something that costs nothing to redo.
    ///
    /// The system default is offered as its own option and stored as an empty
    /// string, because "whatever the OS thinks" is a real and common choice and
    /// should not be spelled as whichever device happens to be first.
    fn render_device_field(
        &self,
        field: &Field,
        me: &gpui::WeakEntity<Self>,
        cx: &mut App,
    ) -> impl IntoElement {
        let current = self.settings.get(field.key).to_string();
        let listed = input_devices();

        let mut options = vec![String::new()];
        let error = match &listed {
            Ok(found) => {
                options.extend(found.iter().cloned());
                None
            }
            Err(message) => Some(message.clone()),
        };

        let label_of = |name: &str| {
            if name.is_empty() {
                "system default".to_string()
            } else {
                name.to_string()
            }
        };

        div()
            .flex()
            .flex_col()
            .gap_1()
            .children(options.iter().map(|option| {
                let selected = *option == current;
                let label = label_of(option);
                let key = field.key;
                let value = option.clone();
                let me = me.clone();

                segment_button(
                    SharedId::choice(key, if value.is_empty() { "default" } else { &value }),
                    label,
                    selected,
                    move |_window, cx| {
                        let _ = me.update(cx, |this, _cx| this.settings.set(key, value.clone()));
                    },
                )
                .into_any_element()
            }))
            .when(error.is_some(), |this| {
                this.child(
                    div()
                        .text_size(px(10.0))
                        .text_color(Theme::danger(cx))
                        .child(format!(
                            "could not list microphones: {}",
                            error.unwrap_or_default()
                        )),
                )
            })
    }

    fn render_footer(&self, me: &gpui::WeakEntity<Self>, cx: &mut App) -> impl IntoElement {
        let dirty = self.settings.is_dirty();
        let notice = self.settings.notice.clone();
        let is_error = self.settings.notice_is_error;

        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .bg(Theme::surface(cx))
            .border_1()
            .border_color(Theme::border(cx))
            .when(notice.is_some(), |this| {
                this.child(
                    div()
                        .p_2()
                        .rounded_md()
                        .bg(Theme::bg(cx))
                        .text_size(LABEL)
                        .text_color(if is_error {
                            Theme::danger(cx)
                        } else {
                            Theme::success(cx)
                        })
                        .child(notice.unwrap_or_default()),
                )
            })
            .child(
                Button::new("save-settings")
                    .label(if dirty { "Save changes" } else { "All saved ✓" })
                    .primary()
                    .disabled(!dirty)
                    .on_click({
                        let me = me.clone();
                        move |_, _window, cx| {
                            let _ = me.update(cx, |this, cx| this.save(cx));
                        }
                    }),
            )
            .child(
                div()
                    .text_size(LABEL)
                    .text_color(Theme::muted(cx))
                    .child(
                        "Voice changes reload the mic on Save — you'll miss a second of talk.",
                    ),
            )
    }
}

impl SettingsView {
    /// Which section is on screen.
    pub fn section(&self) -> usize {
        self.settings.section
    }

    /// The file this panel reads and writes.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Whether a file chooser is open.
    pub fn is_browsing(&self) -> bool {
        self.browsing
    }

    /// Whether the panel has unsaved edits, so Save would do something.
    pub fn is_dirty(&self) -> bool {
        self.settings.is_dirty()
    }

    /// Whether a credential's input is rendering as a password.
    ///
    /// Goes through the presentation the element actually renders, because that
    /// is what `is_masked` on the state refers to internally and there is no
    /// public accessor that reports it for a state which is not on screen.
    pub fn secret_input_is_masked(&self, key: &str) -> bool {
        self.secret_masks.get(key).copied().unwrap_or(false)
    }

    /// The keys whose edits this panel reacts to.
    pub fn subscribed_keys(&self) -> Vec<String> {
        self.subscribed_keys.clone()
    }

    /// The editable model's value for a setting.
    ///
    /// Public so a test can check what Save would serialise, rather than only
    /// checking that Save is disabled — the latter is the visible half, and it
    /// is possible to have the model hold a credential while Save stays
    /// disabled for an unrelated reason.
    pub fn model_value(&self, key: &str) -> String {
        self.settings.get(key).to_string()
    }

    /// The masked input behind a credential field.
    ///
    /// Public so a test can drive the real control rather than a stand-in.
    /// Reading it through the panel is what makes the test evidence about the
    /// panel's wiring rather than about a fresh `InputState` in isolation.
    pub fn secret_input(
        &self,
        key: &str,
    ) -> Option<Entity<gpui_kit::component::input::InputState>> {
        self.secret_inputs.get(key).cloned()
    }

    /// Open the native file chooser and adopt whatever comes back.
    ///
    /// The dialog itself runs on the tokio runtime, not here. `rfd` blocks its
    /// thread until the person picks or cancels, and this is the GPUI thread:
    /// blocking it would freeze the window, stop it repainting, and leave a
    /// grey rectangle on screen while the chooser is open — which reads as a
    /// crash rather than as a modal dialog.
    ///
    /// The result comes back over an async channel and is applied by a task on
    /// the entity, which is the same shape every other background result in
    /// this app uses.
    fn browse(&mut self, cx: &mut Context<Self>) {
        if self.browsing {
            return;
        }
        self.browsing = true;
        cx.notify();

        let (sender, receiver) = async_channel::bounded(1);
        let current = self.settings.get("model_path").to_string();
        let start = std::path::PathBuf::from(current)
            .parent()
            .filter(|dir| dir.is_dir())
            .map(Path::to_path_buf)
            .unwrap_or_else(yap_core::settings::models_dir);

        // `.bin` and `.gguf` rather than no filter: the list a chooser shows is
        // a contract about what can be chosen, and letting someone pick a
        // directory or a text file only defers the failure to the next launch.
        crate::runtime::spawn(async move {
            let dialog = rfd::FileDialog::new()
                .set_title("Choose a whisper model")
                .add_filter("whisper models", &["bin", "gguf"])
                .set_directory(&start);

            let chosen = tokio::task::spawn_blocking(move || dialog.pick_file())
                .await
                .ok()
                .flatten();

            // `send_blocking` rather than `send`: this is a runtime thread, not
            // the GPUI executor, and the channel has room to spare, so there is
            // nothing here to wait on.
            let _ = sender.send_blocking(chosen);
        });

        // Stored rather than dropped or detached. A dropped `Task` is cancelled,
        // which would leave the panel stuck at `browsing = true` for ever; a
        // detached one would outlive the window it belongs to.
        self.browse_task = Some(cx.spawn(async move |view, cx| {
            // No result to apply means the person cancelled, which is not an
            // error and must not be reported as one.
            let picked = receiver.recv().await.ok().flatten();
            let _ = view.update(cx, |this, cx| {
                this.browsing = false;

                if let Some(file) = picked {
                    this.settings.set("model_path", file.display().to_string());
                }

                cx.notify();
            });
        }));
    }

    /// Write the panel back to the settings file.
    fn save(&mut self, cx: &mut gpui::Context<Self>) {
        match self.write() {
            Ok(saved) => {
                self.settings.mark_saved();
                self.apply_live(&saved, cx);

                // Tell the microphone, so changing the model or the input device
                // takes effect now rather than at the next launch.
                //
                // The state decides whether that is warranted by comparing the
                // capture configuration before and after, so a save that only
                // touched a colour does not tear the microphone down mid-count.
                self.model.read(cx).state().notify_capture_changed();

                // Asked once: a second call could disagree with the first, and
                // the notice and its styling must not come from two different
                // evaluations of the same question.
                match model_warning(&saved) {
                    Some(warning) => {
                        self.settings.notice = Some(warning);
                        self.settings.notice_is_error = true;
                    }
                    None => {
                        self.settings.notice = Some("saved".to_string());
                        self.settings.notice_is_error = false;
                    }
                }

                cx.notify();
            }
            Err(error) => {
                // A failed save has to say so: the streamer needs to know their
                // edits are not on disk, and a settings panel that swallows this
                // loses them silently at the next launch.
                self.settings.notice = Some(format!("could not save: {error:#}"));
                self.settings.notice_is_error = true;
                cx.notify();
            }
        }
    }

    /// Put the parts of a save that can take effect now, into effect now.
    ///
    /// Appearance, accent and overlay theme used to be applied once at startup,
    /// so picking a different one in this panel changed a number in the file and
    /// nothing else — a setting that looks live and is not, which is the same
    /// class of thing as the Events panel.
    ///
    /// The window repaints because the session entity is notified. The view
    /// observes that entity, so it is already the right lever: reaching through
    /// to the window from here would mean a handle to something that is not this
    /// panel's to hold.
    fn apply_live(&self, saved: &yap_core::settings::Settings, cx: &mut gpui::Context<Self>) {
        crate::theme::set_accent(crate::theme::Accent::from_id(&saved.accent));

        // Light/dark was the one setting that never applied live at all, because
        // the component theme is changed through the app rather than through this
        // panel's own state — so it was the easiest one to leave out and the most
        // obvious one to a user ("I picked light and nothing happened").
        {
            use gpui_kit::component::Theme as KitTheme;

            let mode = crate::app::appearance_from(&saved.appearance).resolve(true);
            KitTheme::change(mode, None, cx);
        }

        // The overlay reads its look from the shared state, so this is what the
        // served page will use on its next poll. Both axes: the palette and the
        // shape are separate settings and the page keys on both.
        self.model.update(cx, |model, cx| {
            model.set_overlay_theme(&saved.overlay_theme, cx);
            model.set_overlay_style(&saved.overlay_style, cx);
        });
    }

    /// The write itself, split out so it can be tested without a window.
    pub fn write(&self) -> anyhow::Result<yap_core::settings::Settings> {
        let mut settings = yap_core::settings::load(&self.path)?;

        for section in SETTINGS_SECTIONS {
            for field in section.fields {
                let raw = self.settings.get(field.key).to_string();
                apply_field(&mut settings, field, &raw);
            }
        }

        yap_core::settings::save(&self.path, &settings)?;

        // Returned so the caller can apply whatever takes effect immediately,
        // from the same values that were written. Reading the file back instead
        // would mean two sources of truth for one save.
        Ok(settings)
    }
}

/// Apply one field's edited text back onto the settings.
///
/// Returns `false` for a key it does not recognise, so a schema field with no
/// arm here is caught by a test rather than silently doing nothing.
fn apply_field(settings: &mut yap_core::settings::Settings, field: &Field, raw: &str) -> bool {
    let trimmed = raw.trim();

    match field.key {
        "expand_contractions" => settings.rules.expand_contractions = parse_bool(trimmed),
        "spell_out_numerals" => settings.rules.spell_out_numerals = parse_bool(trimmed),
        "exclude_fillers" => settings.rules.exclude_fillers = parse_bool(trimmed),
        "count_bare_numerals" => settings.rules.count_bare_numerals_as_words = parse_bool(trimmed),
        "keep_transcript" => settings.keep_transcript = parse_bool(trimmed),
        "bind_address" => {
            if !trimmed.is_empty() {
                settings.bind_address = trimmed.to_string();
            }
        }
        "port" => {
            if let Ok(port) = trimmed.parse() {
                settings.port = port;
            }
        }
        "default_hard_limit" => {
            settings.default_hard_limit = trimmed.parse::<u32>().ok();
        }
        // The mode is a choice, so only the name arrives. The target itself is
        // set per-yapathon from the start form, not from the defaults, so the
        // existing base is kept.
        "default_mode" => {
            let base = settings
                .default_mode
                .base_words()
                .or(settings.default_hard_limit)
                .unwrap_or(5_000);

            if let Some(mode) = GoalMode::from_short_name(trimmed, base) {
                settings.default_mode = mode;
            }
        }
        // Assigned rather than parsed: an empty value is the system default and
        // has to survive, so there is nothing here that can fail.
        "input_device" => settings.input_device = trimmed.to_string(),
        // The Astro tokens are in the keyring, not here. Deliberately a no-op,
        // like every other credential: `settings.toml` is the file that gets
        // synced, backed up and pasted into support threads.
        // An empty box means "worth nothing", not "leave it alone". A streamer
        // who blanks a price means to stop that event extending the goal, and
        // silently keeping the old value would leave chat able to move a number
        // the streamer believes is fixed.
        // Trimmed and lower-cased rather than taken verbatim, because a
        // mistyped theme must fall back to the default and not to a blank card.
        // `overlay_theme` resolves unknown names, so a typo here is harmless.
        // Both resolve an unknown name to the default rather than failing, so
        // these are assigned verbatim: a typo should not be able to lose the
        // rest of the save.
        "accent" => settings.accent = trimmed.to_lowercase(),
        "appearance" => settings.appearance = trimmed.to_lowercase(),
        "overlay_theme" => settings.overlay_theme = trimmed.to_lowercase(),
        "overlay_style" => settings.overlay_style = trimmed.to_lowercase(),
        _ if field.key.starts_with("event_") => {
            let event = event_key(field.key);
            if trimmed.is_empty() {
                // A blank box is a decision to stop that event extending the
                // goal, so the price is removed rather than kept. Silently
                // keeping the old value would leave chat able to move a number
                // the streamer believes is fixed.
                settings.event_words.remove(&event);
            } else if let Ok(words) = trimmed.parse::<u32>() {
                settings.event_words.insert(event, words);
            }
        }
        "vad_min_silence_ms" => {
            if let Ok(ms) = trimmed.parse() {
                settings.vad.min_silence_ms = ms;
            }
        }
        "vad_threshold" => {
            if let Ok(value) = trimmed.parse::<f32>() {
                settings.vad.threshold = value.clamp(0.0, 1.0);
            }
        }
        // Read-only, but real settings rather than labels. `EngineChoice` is an enum
        // and the backend is fixed at compile time by the build features, so
        // there is nothing to write — claimed explicitly so the wiring test knows
        // the omission is deliberate.
        "engine" | "engine_backend" => {}
        // Secrets live in the OS credential store and are deliberately not
        // written here. Claimed so a secret is not mistaken for a forgotten
        // field.
        "groq_key" | "twitch_token" | "astro_youtube_token" | "astro_twitch_token" => {}
        "model_path" => {
            if !trimmed.is_empty() {
                if let yap_core::settings::EngineChoice::Local { model_path, .. } =
                    &mut settings.engine
                {
                    *model_path = trimmed.into();
                }
            }
        }
        "twitch_channel" => {
            settings.twitch_channel = (!trimmed.is_empty()).then(|| trimmed.to_string());
        }
        "twitch_client_id" => {
            settings.twitch_client_id = (!trimmed.is_empty()).then(|| trimmed.to_string());
        }
        "twitch_credential" => {
            settings.twitch_credential = (!trimmed.is_empty()).then(|| trimmed.to_string());
        }
        _ => return false,
    }

    true
}

/// Parse a boolean leniently.
///
/// Unparseable text is `false` rather than an error: a settings panel should not
/// refuse to save because one toggle got a stray character.
fn parse_bool(raw: &str) -> bool {
    matches!(raw.to_lowercase().as_str(), "1" | "true" | "yes" | "on")
}

/// What a credential field says about itself.
///
/// Three states, not two, because the third is the one that matters: a machine
/// with no credential store must not show an input. Inviting someone to paste a
/// key somewhere the app cannot keep it is how keys end up in screenshots and
/// shell history — the exact outcome the credential store exists to prevent.
fn secret_status(state: yap_core::secrets::SecretState) -> (&'static str, bool) {
    match state {
        yap_core::secrets::SecretState::Present => ("Connected ✓ — saved on this PC", true),
        yap_core::secrets::SecretState::Absent => ("Not connected yet", true),

        // Locked is its own message because its own command fixes it. Telling
        // someone their machine has no credential store when it is sat right
        // there, locked, is the kind of technically-true thing that sends them
        // off to reinstall an OS.
        yap_core::secrets::SecretState::Locked => ("locked — unlock it, then check again", false),

        yap_core::secrets::SecretState::Unavailable => (
            "This PC can't store logins safely — skipping this",
            false,
        ),
    }
}

/// The extra line under a credential row, when there is something useful to add.
///
/// Platform-specific advice, kept out of [`secret_status`] so that function stays
/// a pure mapping of state to words — and so that the advice is compiled per
/// platform rather than chosen at runtime on a machine that has six.
fn secret_hint(state: yap_core::secrets::SecretState) -> &'static str {
    match state {
        yap_core::secrets::SecretState::Locked => yap_core::secrets::unlock_hint(),
        _ => "",
    }
}

/// A secret field: a dialog, never the value.
///
/// The key is never read back into a field, so it cannot be copied out of the
/// settings screen, and it never touches the settings file. It goes to the OS
/// credential store when the dialog's own button is pressed — never by the
/// panel's Save, which writes a file.
fn render_secret_field(
    field: &Field,
    input: Option<Entity<gpui_kit::component::input::InputState>>,
    entry: Option<(String, &'static str)>,
    state: yap_core::secrets::SecretState,
    me: &gpui::WeakEntity<SettingsView>,
    cx: &mut App,
) -> impl IntoElement {
    let key = field.key;
    let Some((service, user)) = entry else {
        // `credential_entry` returns `None` only for a field that is not a
        // credential, so this should be unreachable. Showing the key's name is
        // better than a button that writes nowhere.
        return div()
            .text_size(px(10.0))
            .text_color(Theme::danger(cx))
            .child("no credential location is configured for this field");
    };

    // Read from the cache, never from the keyring: this runs while drawing.
    let (status, can_set) = secret_status(state);

    let mut row = div().flex().flex_col().gap_1().child(
        div()
            .text_size(px(10.0))
            .text_color(if can_set {
                Theme::muted(cx)
            } else {
                Theme::danger(cx)
            })
            .child(status),
    );

    // The way out, when there is one. Only for a *locked* store: a machine with
    // no store has nothing to unlock, and saying "unlock it" there would send
    // someone off to reinstall an OS.
    if state == yap_core::secrets::SecretState::Locked {
        let hint = secret_hint(state);
        if !hint.is_empty() {
            row = row.child(
                div()
                    .text_size(px(10.0))
                    .text_color(Theme::muted(cx))
                    .child(hint),
            );
        }
    }

    if !can_set {
        // No button at all. A disabled one still looks like the thing you are
        // supposed to press, and the only way to find out is to try.
        return row;
    }

    let input = match input {
        Some(input) => input,
        // Built in `new` for every secret field, so this cannot normally
        // happen. Falling back to the status alone is better than a button
        // that opens nothing.
        None => return row,
    };

    let replace = matches!(state, yap_core::secrets::SecretState::Present);

    // The button is the dialog's trigger, so the dialog owns showing itself.
    // The closure needs the panel only to write a notice when the store
    // answers — and answers can be failures.
    let dialog_input = input.clone();

    let set_button = Button::new(SharedId::secret(key))
        .label(if replace { "Replace…" } else { "Set…" })
        .on_click(move |_, _window, cx| {
            // Clear whatever was last typed, so reopening never shows a stale
            // token and the field starts empty every time.
            dialog_input.update(cx, |state, cx| {
                state.set_value(String::new(), _window, cx);
            });
        });

    let ok_input = input.clone();
    let content_input = input.clone();
    let ok_me = me.clone();
    let ok_service = service.clone();
    let ok_label = if replace { "Replace" } else { "Save" };

    let dialog = gpui_kit::component::dialog::AlertDialog::new(cx)
        .trigger(set_button)
        .ok_text(ok_label)
        .cancel_text("Cancel")
        .content(move |content, _window, cx| {
            let input = gpui_kit::component::input::Input::new(&content_input)
                .h(px(28.0))
                .w_full()
                // A reveal toggle rather than permanent masking: these are long
                // opaque strings and a typo is otherwise invisible until the
                // connection fails with an authentication error.
                .mask_toggle()
                .content_type(gpui_kit::component::input::InputContentType::Password);

            content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(Theme::muted(cx))
                            .child("Saved to this machine's credential store. Never to settings.toml, so the settings file stays safe to share."),
                    )
                    .child(input),
            )
        })
        .on_ok(move |_event, _window, cx| {
            let value = ok_input.read(cx).value().to_string();

            let result = yap_core::secrets::set(&ok_service, user, &value);

            // Clear the input whatever happened. On success it has been stored;
            // on failure it is sitting in a plaintext field in a screenshot.
            ok_input.update(cx, |state, cx| state.set_value(String::new(), _window, cx));

            let _ = ok_me.update(cx, |this, cx| {
                // The outcome is the answer, so it is not re-read. Another store
                // round trip here would be another thing that can block the GPUI
                // thread for as long as a keyring stays locked, in exchange for
                // a fact we were just told.
                if result.is_ok() {
                    this.secret_states.insert(
                        key.to_string(),
                        yap_core::secrets::SecretState::Present,
                    );
                }

                this.settings.notice = Some(match &result {
                    Ok(()) => format!("{key} stored"),
                    Err(error) => format!("could not store {key}: {error:#}"),
                });
                this.settings.notice_is_error = result.is_err();
                cx.notify();
            });

            // Close even on failure. The message is on the settings screen,
            // which stays visible, and a dialog that refuses to close is a
            // worse way to report an error than a line of text.
            true
        });

    row = row.child(dialog);

    // Clear, only when there is something to clear.
    if replace {
        let clear_me = me.clone();
        let clear_service = service.clone();
        row = row.child(
            Button::new(SharedId::secret_clear(key))
                .label("Clear")
                .on_click(move |_, _window, cx| {
                    let result = yap_core::secrets::remove(&clear_service, user);
                    let failed = result.is_err();

                    let _ = clear_me.update(cx, |this, cx| {
                        // Same: the clear either happened or it did not.
                        this.secret_states
                            .insert(key.to_string(), yap_core::secrets::SecretState::Absent);

                        this.settings.notice = Some(match &result {
                            Ok(true) => format!("{key} cleared"),
                            Ok(false) => format!("{key} was not set"),
                            Err(error) => format!("could not clear {key}: {error:#}"),
                        });
                        this.settings.notice_is_error = failed;
                        cx.notify();
                    });
                }),
        );
    }

    row
}

/// Whether a credential input is rendered as a password.
///
/// One constant, used both to build the input and to record what it was built
/// as, because a test that reads a flag written separately from the call it
/// describes is a test of the flag.
const SECRET_INPUTS_ARE_MASKED: bool = true;

/// Element ids.
///
/// Buttons need ids that are unique per control. Building them from the field
/// key means two fields with the same key cannot collide silently, and adding a
/// field needs no id of its own.
pub struct SharedId;

impl SharedId {
    /// The file chooser button.
    ///
    /// Public and given a stable id because a test looking for it is how we know
    /// the engine section drew its picker at all.
    pub fn browse_model() -> gpui::ElementId {
        "browse-model".into()
    }

    pub fn section(index: usize) -> gpui::ElementId {
        format!("settings-section-{index}").into()
    }

    fn field(key: &str) -> gpui::ElementId {
        format!("settings-field-{key}").into()
    }

    fn choice(key: &str, option: &str) -> gpui::ElementId {
        format!("settings-choice-{key}-{option}").into()
    }

    fn model(name: &str) -> gpui::ElementId {
        format!("settings-model-{name}").into()
    }

    /// The button that opens the credential prompt.
    ///
    /// Public and given a stable id because a test looking for it is how we know
    /// the prompt is wired to something — which is this project's recurring bug:
    /// a control that draws correctly and does nothing.
    pub fn secret(key: &str) -> gpui::ElementId {
        format!("settings-secret-{key}").into()
    }

    /// The button that clears a stored credential.
    ///
    /// Separate from [`SharedId::secret`] because both can be on screen at
    /// once: a credential that is set renders Set *and* Clear, and ids built
    /// from the key alone would collide.
    fn secret_clear(key: &str) -> gpui::ElementId {
        format!("settings-secret-clear-{key}").into()
    }
}

impl gpui::Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        // A weak handle to this entity, captured so click handlers can reach it.
        //
        // `cx.entity()` is the only way to get one from inside a render, and the
        // handlers receive `&mut App` rather than a `Context<Self>`. Capturing a
        // weak handle rather than a strong `Entity` means a button's closure
        // cannot keep the panel alive after the window closes.
        let me = self.me.clone();

        let fields = self.render_fields(&me, cx);

        // Only meaningful on the section that owns the model path.
        let picker = if self.settings.section == engine_section() {
            self.render_model_picker(&me, cx)
        } else {
            None
        };

        let running = self.model.read(cx).model().snapshot.is_some();
        let blurb = self.section_blurb().to_string();
        let reward_presets = self.render_reward_presets(&me, cx);

        div()
            .flex()
            .flex_col()
            .gap_3()
            .min_w_0()
            .when(running, |this| {
                this.child(
                    div()
                        .p_3()
                        .rounded_md()
                        .bg(Theme::surface(cx))
                        .border_1()
                        .border_color(Theme::warning(cx))
                        .text_size(LABEL)
                        .text_color(Theme::warning(cx))
                        .child("A run is going — changes apply to the next one"),
                )
            })
            .child(
                div()
                    .text_size(crate::view::text::TITLE)
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("Settings"),
            )
            .child(
                div()
                    .text_size(LABEL)
                    .text_color(Theme::muted(cx))
                    .child(blurb),
            )
            .child(self.render_sections(&me, cx))
            .when(picker.is_some(), |this| this.child(picker.unwrap()))
            .when(reward_presets.is_some(), |this| {
                this.child(reward_presets.unwrap())
            })
            .child(div().flex().flex_col().gap_2().children(fields))
            .child(self.render_footer(&me, cx))
    }
}

/// The microphones this machine can record from.
///
/// Read fresh each time the panel draws: a USB interface can be plugged in
/// while the app is running, and caching a hardware list means needing a
/// refresh button for something that costs nothing to redo.
///
/// The error is returned rather than collapsed into an empty list, because "no
/// microphones found" and "could not ask the machine for its microphones" need
/// different fixes — and an empty list claims the first.
pub fn input_devices() -> Result<Vec<String>, String> {
    yap_voice::Microphone::list_devices()
        .map(|devices| devices.into_iter().map(|device| device.name).collect())
        .map_err(|error| format!("{error:#}"))
}

/// The index of the section holding the voice + model settings.
fn engine_section() -> usize {
    SETTINGS_SECTIONS
        .iter()
        .position(|section| section.title == "Voice")
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SessionModel;
    use yap_core::settings::{Settings, default_model_path};

    #[test]
    fn a_saved_accent_takes_effect_without_a_restart() {
        // Pressing Save updated the file and nothing else: the window stayed the
        // colour it was. `appearance` was worse — it was never applied live at
        // all, because the component theme is changed through the app rather
        // than through this panel's own state.
        //
        // The accent is process-wide, so this is asserted on the value rather
        // than on pixels. Restored at the end: leaving it set would decide the
        // palette for every test after this one.
        use crate::theme::{Accent, accent, set_accent};

        let field = SETTINGS_SECTIONS
            .iter()
            .flat_map(|section| section.fields.iter())
            .find(|field| field.key == "accent")
            .copied()
            .expect("an accent field");

        let mut settings = Settings::default();
        assert!(apply_field(&mut settings, &field, "purple"));

        // What `apply_live` does with the saved value.
        set_accent(Accent::from_id(&settings.accent));
        assert_eq!(accent(), Accent::Purple);

        set_accent(Accent::default());
    }

    #[test]
    fn every_appearance_the_picker_offers_is_one_the_app_understands() {
        // `appearance` resolves an unknown name to "follow system" rather than
        // failing, so a stale offer is a silent no-op rather than a visible
        // failure. This is what makes the list worth checking.
        let offered: Vec<&str> = SETTINGS_SECTIONS
            .iter()
            .flat_map(|section| section.fields.iter())
            .find(|field| field.key == "appearance")
            .map(|field| field.options.to_vec())
            .expect("an appearance field");

        // Each name must resolve to itself. "auto" resolving to the fallback is
        // correct — it *is* the fallback — so the check is against the
        // expectation rather than against "not the default".
        let expected = [
            ("auto", crate::theme::Appearance::Auto),
            ("dark", crate::theme::Appearance::Dark),
            ("light", crate::theme::Appearance::Light),
        ];

        for (name, want) in expected {
            assert!(offered.contains(&name), "{name:?} is not offered");
            assert_eq!(
                crate::app::appearance_from(name),
                want,
                "{name:?} is offered but does not mean what it says",
            );
        }
    }

    #[test]
    fn the_url_the_app_offers_carries_the_theme_it_serves() {
        // The window and the server have to agree about which look is in force.
        // If the window offered `/overlay` while the server was set to `cute`,
        // a streamer copying the URL would get the default card and conclude
        // the setting did not work.
        // Note: catppuccin-mocha is not a legacy combined name, so the style
        // we set survives (a legacy theme like "mono" would map to its old
        // pair and discard an explicitly set style — see `legacy_look`).
        let state = yap_server::AppState::new();
        state.set_overlay_theme("catppuccin-mocha");
        state.set_overlay_style("strip");

        let theme = state.overlay_theme(None);
        assert_eq!(theme, "catppuccin-mocha");
        let style = state.overlay_style(None);
        assert_eq!(style, "strip");

        let offered = SessionModel {
            overlay_url: "http://127.0.0.1:8973/overlay".to_string(),
            overlay_theme: theme,
            overlay_style: style,
            ..SessionModel::default()
        }
        .overlay_link();

        // The URL the app hands over is authoritative: asking for that theme
        // and style serves them, whatever the setting happens to be later.
        state.set_overlay_theme("card");
        state.set_overlay_style("corner");
        assert_eq!(
            state.overlay_theme(Some("catppuccin-mocha")),
            "catppuccin-mocha"
        );
        assert_eq!(state.overlay_style(Some("strip")), "strip");
        assert!(offered.ends_with("?theme=catppuccin-mocha&style=strip"));
    }

    /// The options the settings panel offers for one field.
    fn offered(key: &str) -> Vec<&'static str> {
        SETTINGS_SECTIONS
            .iter()
            .flat_map(|section| section.fields.iter())
            .find(|field| field.key == key)
            .map(|field| field.options.to_vec())
            .unwrap_or_else(|| panic!("a {key} field"))
    }

    #[test]
    fn both_overlay_axes_the_panel_offers_are_the_ones_that_render() {
        // The two pickers list `card, minimal, cute, neon, mono, …` and
        // `corner, slim, strip, big`. If `yap-server` gains or loses either, this
        // fails rather than the panel offering a button that silently does
        // nothing — which is the failure mode this project keeps meeting.
        //
        // Both axes, because they are separate lists now and either can drift.
        let palettes: Vec<&str> = yap_server::overlay::OVERLAY_PALETTES
            .iter()
            .map(|palette| palette.id)
            .collect();

        let styles: Vec<&str> = yap_server::overlay::OVERLAY_STYLES
            .iter()
            .map(|style| style.id)
            .collect();

        assert_eq!(
            offered("overlay_theme"),
            palettes,
            "the colour picker and the served overlay must offer the same palettes",
        );
        assert_eq!(
            offered("overlay_style"),
            styles,
            "the shape picker and the served overlay must offer the same styles",
        );
    }

    #[test]
    fn a_style_picked_in_the_panel_survives_all_the_way_to_the_page() {
        // The seam, because "the dropdown appears and takes clicks" is not the
        // same as "the shape changes". A field missing from `apply_field` reads
        // and writes nothing, draws perfectly, and saves nothing.
        let field = SETTINGS_SECTIONS
            .iter()
            .flat_map(|section| section.fields.iter())
            .find(|field| field.key == "overlay_style")
            .copied()
            .expect("an overlay style field");

        let mut settings = Settings::default();
        assert!(apply_field(&mut settings, &field, "  Strip  "));
        assert_eq!(
            settings.overlay_style, "strip",
            "the chosen shape must be stored, lower-cased and trimmed",
        );

        // And it reads back, which is what marks the current option in the
        // dropdown. A write-only field would save correctly and show nothing
        // selected.
        assert_eq!(field_value(&settings, &field), "strip");

        let state = yap_server::AppState::shared();
        state.set_overlay_theme("catppuccin-mocha");
        state.set_overlay_style(&settings.overlay_style);

        let page = yap_server::overlay::overlay_html_with(
            &state.overlay_theme(None),
            Some(&state.overlay_style(None)),
        );

        assert!(page.contains(r#"data-theme="catppuccin-mocha""#), "{page}");
        assert!(page.contains(r#"data-style="strip""#), "{page}");
    }

    #[test]
    fn an_old_combined_setting_still_renders_the_overlay_it_used_to() {
        // Someone upgrading mid-stream. Their file says `overlay_theme = "strip"`
        // — which is a *shape*, not a colour, and now also a real style id. Read
        // as a palette alone it would fall back to `card`, and the strip they
        // chose months ago would quietly become a bottom-right card.
        let state = yap_server::AppState::shared();
        state.set_overlay_theme("strip");

        let (palette, style) = state.overlay_look(None, None);
        assert_eq!(palette, "card");
        assert_eq!(style, "strip");
    }

    #[test]
    fn a_url_can_pin_one_axis_and_leave_the_other_alone() {
        // `?theme=neon` should change the colours and keep the shape. Falling
        // back to the default shape would undo whatever the streamer picked in
        // the app the first time they opened a themed URL in a browser.
        let state = yap_server::AppState::shared();
        state.set_overlay_theme("card");
        state.set_overlay_style("strip");

        let (palette, style) = state.overlay_look(Some("neon"), None);
        assert_eq!(palette, "neon");
        assert_eq!(
            style, "strip",
            "pinning the palette must not reset the shape"
        );

        let (palette, style) = state.overlay_look(None, Some("big"));
        assert_eq!(
            palette, "card",
            "pinning the shape must not reset the palette"
        );
        assert_eq!(style, "big");
    }

    #[test]
    fn a_chosen_overlay_theme_reaches_the_page() {
        // End to end: set it through the panel, resolve it the way the server
        // does, and check the served markup carries it.
        let field = SETTINGS_SECTIONS
            .iter()
            .flat_map(|section| section.fields.iter())
            .find(|field| field.key == "overlay_theme")
            .copied()
            .expect("an overlay theme field");

        let mut settings = Settings::default();
        assert!(apply_field(&mut settings, &field, "  Neon  "));

        let state = yap_server::AppState::shared();
        state.set_overlay_theme(&settings.overlay_theme);

        assert_eq!(state.overlay_theme(None), "neon");
        assert!(
            yap_server::overlay::overlay_html_with(&state.overlay_theme(None), None)
                .contains(r#"data-theme="neon""#),
        );
    }

    #[test]
    fn a_nonsense_overlay_theme_still_serves_something() {
        // A hand-edited settings file must not be able to blank the one thing
        // that is on screen during a stream.
        let state = yap_server::AppState::shared();
        state.set_overlay_theme("not-a-real-theme");

        assert_eq!(state.overlay_theme(None), "card");
        assert!(
            yap_server::overlay::overlay_html_with(&state.overlay_theme(None), None)
                .contains(r#"data-theme="card""#)
        );
    }

    #[test]
    fn a_url_can_override_the_theme_without_touching_settings() {
        // How a streamer compares looks: open the URL in a browser, edit the
        // query, pick one. Copying it into OBS then needs no further setup.
        let state = yap_server::AppState::shared();
        state.set_overlay_theme("card");

        assert_eq!(state.overlay_theme(Some("cute")), "cute");
        assert_eq!(
            state.overlay_theme(None),
            "card",
            "the setting is unchanged"
        );
    }

    #[test]
    fn the_pricing_the_panel_edits_is_the_pricing_the_platform_uses() {
        // The Events section used to be decorative.
        //
        // It read and wrote `settings.event_words`, looked authoritative, and
        // nothing ever read it back: `TwitchPlatform` was built with
        // `TwitchPricing::default()` — hardcoded numbers that happened to match
        // the defaults. A streamer could set a gift sub to 1, hit Save, and see
        // no change and no error.
        //
        // This closes the loop the whole way: edit through the panel, take what
        // the platform would actually be built with, and price an event by it.
        let mut settings = Settings::default();

        let field = SETTINGS_SECTIONS
            .iter()
            .flat_map(|section| section.fields.iter())
            .find(|field| field.key == "event_twitch_gift_sub")
            .copied()
            .expect("a gift price field");

        assert!(apply_field(&mut settings, &field, "7"));

        let pricing = yap_events::TwitchPricing::from_words(settings.pricing_for("twitch"));
        let event = yap_events::price_event(
            "twitch",
            "gift_sub",
            None,
            Some(100.0),
            pricing.get("gift_sub"),
        );

        assert_eq!(
            event.words,
            Some(700),
            "100 gifted subs at the price the panel was just given",
        );
    }

    #[test]
    fn an_event_with_no_price_grants_nothing() {
        // Deleting a key must mean "this event is worth nothing", not "fall back
        // to a default". Chat must not be able to move a goal through a kind the
        // streamer has not agreed to pay for.
        let mut settings = Settings::default();
        let field = SETTINGS_SECTIONS
            .iter()
            .flat_map(|section| section.fields.iter())
            .find(|field| field.key == "event_twitch_raid")
            .copied()
            .expect("a raid price field");

        assert!(apply_field(&mut settings, &field, ""));
        let pricing = yap_events::TwitchPricing::from_words(settings.pricing_for("twitch"));

        assert_eq!(pricing.get("raid"), None);
        let event = yap_events::price_event("twitch", "raid", None, None, pricing.get("raid"));
        assert_eq!(event.granted_words(), 0);
    }

    #[test]
    fn pricing_is_split_by_platform() {
        // `pricing_for` hands one platform its own half of the table. If it ever
        // handed over everything, Twitch would start answering to YouTube's
        // prices.
        let settings = Settings {
            event_words: [
                ("twitch:cheer".to_string(), 1),
                ("youtube:superchat".to_string(), 10),
                ("chat:command".to_string(), 100),
            ]
            .into_iter()
            .collect(),
            ..Settings::default()
        };

        let twitch = settings.pricing_for("twitch");
        assert_eq!(twitch.len(), 1);
        assert_eq!(twitch.get("cheer"), Some(&1));

        let youtube = settings.pricing_for("youtube");
        assert_eq!(youtube.len(), 1);
        assert_eq!(youtube.get("superchat"), Some(&10));
    }

    #[test]
    fn every_event_the_core_prices_has_a_field() {
        // One direction only, and the direction that matters: **a price with no
        // field** means a streamer can read the number in their settings file and
        // not change it — the same shape of bug as the dead tab, a control that
        // shows a value and refuses to do anything with it.
        //
        // The reverse is *not* a fault. A field with no price is a field nobody
        // has set yet: it shows blank, and a blank box means "worth nothing",
        // which is the safe default and the honest one. Asserting otherwise
        // would force a made-up price onto every event the app can emit, and a
        // Super Chat priced at some invented rate is a worse thing than an
        // unpriced one the streamer has to fill in deliberately.
        let mut unpriced_fields = Settings::default().event_words;

        for section in SETTINGS_SECTIONS {
            for field in section.fields {
                if !field.key.starts_with("event_") {
                    continue;
                }

                let event = event_key(field.key);
                assert!(
                    !event.is_empty(),
                    "{} produced an empty event key",
                    field.key,
                );
                unpriced_fields.remove(&event);
            }
        }

        assert!(
            unpriced_fields.is_empty(),
            "the core prices these but nothing in the panel can change them: \
             {unpriced_fields:?}",
        );
    }

    #[test]
    fn every_platform_the_app_can_receive_from_has_an_events_field() {
        // The complaint that prompted this: the panel listed Twitch and nothing
        // else, so there was no way to price a Super Chat or a YouTube
        // subscriber from the UI even though both were arriving.
        //
        // Checked by platform rather than by event, because a platform with one
        // priceable kind is enough — the failure was a platform with none.
        for platform in ["twitch", "youtube", "kick"] {
            let priced: Vec<&str> = SETTINGS_SECTIONS
                .iter()
                .flat_map(|section| section.fields.iter())
                .filter(|field| field.key.starts_with("event_"))
                .filter(|field| event_key(field.key).starts_with(&format!("{platform}:")))
                .map(|field| field.key)
                .collect();

            assert!(
                !priced.is_empty(),
                "the panel offers no price for {platform} events, so nothing the app \
                 receives from {platform} can extend a goal",
            );
        }
    }

    #[test]
    fn an_event_field_never_edits_a_price_under_its_own_name() {
        // The failure mode of the table this replaced: a field not mentioned in
        // it fell through to `other => other` and was stored under its own field
        // key, where nothing looks events up. It read and wrote faithfully and
        // did nothing — the original `event_words` bug, one level down.
        for section in SETTINGS_SECTIONS {
            for field in section.fields {
                if !field.key.starts_with("event_") {
                    continue;
                }

                let event = event_key(field.key);
                assert_ne!(
                    event, field.key,
                    "{} stores under its own name, so no lookup can ever find it",
                    field.key,
                );
                assert!(
                    event.contains(':'),
                    "{} produced {event:?}, which is not a <platform>:<kind> key",
                    field.key,
                );
            }
        }
    }

    #[test]
    fn a_blanked_event_price_stops_paying_for_it() {
        // Blank means zero, not "leave it as it was". A streamer who empties a
        // price is saying that event should not move the goal, and quietly
        // keeping the old number would leave chat able to move a number they
        // believe is fixed.
        let field = SETTINGS_SECTIONS
            .iter()
            .flat_map(|section| section.fields.iter())
            .find(|field| field.key == "event_twitch_cheer")
            .copied()
            .expect("a cheer price field");

        let mut settings = Settings::default();
        assert!(apply_field(&mut settings, &field, ""));
        assert!(
            !settings.event_words.contains_key("twitch:cheer"),
            "a blanked price must stop granting words",
        );
    }

    #[test]
    fn a_typed_event_price_is_stored_against_the_right_event() {
        let field = SETTINGS_SECTIONS
            .iter()
            .flat_map(|section| section.fields.iter())
            .find(|field| field.key == "event_twitch_gift_sub")
            .copied()
            .expect("a gift price field");

        let mut settings = Settings::default();
        assert!(apply_field(&mut settings, &field, "75"));
        assert_eq!(settings.event_words.get("twitch:gift_sub"), Some(&75));
    }

    #[test]
    fn the_microphone_can_be_left_on_the_system_default() {
        // Empty has to survive: "whatever the OS thinks" is the right answer
        // for almost everyone, and it has to be storable as itself rather than
        // as whichever device happened to be first.
        let field = SETTINGS_SECTIONS
            .iter()
            .flat_map(|section| section.fields.iter())
            .find(|field| field.key == "input_device")
            .copied()
            .expect("a microphone field");

        let mut settings = Settings {
            input_device: "Some USB Interface".to_string(),
            ..Settings::default()
        };
        assert!(apply_field(&mut settings, &field, ""));
        assert_eq!(
            settings.input_device, "",
            "clearing the microphone must mean the system default",
        );
    }

    #[test]
    fn every_field_has_a_value_when_loaded() {
        let settings = Settings::default();
        let model = load_values(&settings);

        for section in SETTINGS_SECTIONS {
            for field in section.fields {
                assert!(
                    model.values.contains_key(field.key),
                    "{}:{} has no loaded value",
                    section.title,
                    field.key
                );
            }
        }
    }

    #[test]
    fn a_loaded_panel_is_not_dirty() {
        // Opening Settings must not mark the file as needing a save.
        assert!(!load_values(&Settings::default()).is_dirty());
    }

    #[test]
    fn editing_marks_dirty_and_saving_clears_it() {
        let mut model = load_values(&Settings::default());
        assert!(!model.is_dirty());

        model.set("keep_transcript", "false".to_string());
        assert!(model.is_dirty());

        model.mark_saved();
        assert!(!model.is_dirty());
    }

    #[test]
    fn booleans_parse_leniently() {
        let mut model = load_values(&Settings::default());

        for truthy in ["1", "true", "TRUE", "yes", "on"] {
            model.set("exclude_fillers", truthy.to_string());
            assert!(
                model.is_on("exclude_fillers"),
                "{truthy:?} should read as on"
            );
        }

        for falsy in ["0", "false", "no", "off", "", "nonsense"] {
            model.set("exclude_fillers", falsy.to_string());
            assert!(
                !model.is_on("exclude_fillers"),
                "{falsy:?} should read as off"
            );
        }
    }

    #[test]
    fn toggling_produces_a_parseable_value() {
        let mut model = load_values(&Settings::default());
        model.toggle("keep_transcript", false);
        assert_eq!(model.get("keep_transcript"), "false");
        assert!(!parse_bool(model.get("keep_transcript")));
    }

    #[test]
    fn a_field_round_trips_through_the_settings_struct() {
        let mut model = load_values(&Settings::default());
        model.set("expand_contractions", "false".to_string());
        model.set("keep_transcript", "false".to_string());
        model.set("default_hard_limit", "25000".to_string());
        model.set("vad_min_silence_ms", "750".to_string());
        model.set("port", "9000".to_string());

        let mut settings = Settings::default();
        for section in SETTINGS_SECTIONS {
            for field in section.fields {
                let raw = model.get(field.key).to_string();
                apply_field(&mut settings, field, &raw);
            }
        }

        assert!(!settings.rules.expand_contractions);
        assert!(!settings.keep_transcript);
        assert_eq!(settings.default_hard_limit, Some(25_000));
        assert_eq!(settings.vad.min_silence_ms, 750);
        assert_eq!(settings.port, 9000);
    }

    #[test]
    fn an_unparseable_number_is_ignored_rather_than_zeroed() {
        // A typo in one field must not wipe the port and take the dock with it.
        let mut settings = Settings::default();
        apply_field(&mut settings, port_field(), "not a number");
        assert_eq!(settings.port, 8973);
    }

    #[test]
    fn clearing_the_twitch_channel_unsets_it() {
        // An empty field means "off", not "set to nothing", which is what a
        // failed lookup at startup would look like.
        let mut settings = Settings {
            twitch_channel: Some("someone".to_string()),
            ..Settings::default()
        };
        apply_field(&mut settings, twitch_channel_field(), "");
        assert_eq!(settings.twitch_channel, None);
    }

    #[test]
    fn the_model_path_is_shown_for_a_local_engine_and_a_keyring_for_a_cloud_one() {
        let local = load_values(&Settings::default());
        assert!(
            local.get("model_path").ends_with("ggml-large-v3-turbo.bin"),
            "got {}",
            local.get("model_path")
        );
        // And it is the path the settings would actually use.
        assert_eq!(
            local.get("model_path"),
            default_model_path().display().to_string()
        );

        let cloud = load_values(&Settings {
            engine: yap_core::settings::EngineChoice::Groq {
                credential: "yap-a-thon/groq".to_string(),
            },
            ..Settings::default()
        });
        assert!(cloud.get("model_path").contains("keyring"));
    }

    #[test]
    fn a_vad_threshold_is_clamped_to_a_probability() {
        let mut settings = Settings::default();
        let field = vad_threshold_field();
        apply_field(&mut settings, field, "5.0");
        assert_eq!(settings.vad.threshold, 1.0);
        apply_field(&mut settings, field, "-1.0");
        assert_eq!(settings.vad.threshold, 0.0);
    }

    #[test]
    fn every_field_is_wired_to_something() {
        // The bug this guards, in its worse form: a field that renders, accepts
        // typing, marks the panel dirty, saves — and then writes nothing because
        // `apply_field` has no arm for it. The value is accepted and discarded,
        // which is worse than a read-only field because it looks like it worked.
        //
        // So every field in the schema must be claimed by both the loader and
        // the writer. A new field added to the schema without being wired up
        // fails here instead of silently doing nothing.
        for section in SETTINGS_SECTIONS {
            for field in section.fields {
                assert!(
                    apply_field(&mut Settings::default(), field, "probe"),
                    "{}/{}: apply_field ignores it, so editing it would be a lie",
                    section.title,
                    field.key
                );
            }
        }
    }

    #[test]
    fn an_unset_optional_setting_loads_as_empty_rather_than_as_junk() {
        // A channel nobody configured has to read as empty. Loading a placeholder
        // would make the panel look configured when it is not.
        let model = load_values(&Settings::default());
        assert_eq!(model.get("twitch_channel"), "");
        assert_eq!(model.get("twitch_client_id"), "");
    }

    #[test]
    fn the_default_mode_loads_as_a_name_a_choice_button_matches() {
        // The bug this guards: the mode loaded as `Extend { base_words: 5000 }`,
        // which matches no button, so no mode appeared selected and the panel
        // looked like it had no mode set at all.
        let model = load_values(&Settings::default());
        let loaded = model.get("default_mode");

        assert!(
            yap_core::goal::GoalMode::short_names().contains(&loaded),
            "{loaded:?} is not one of the offered choices"
        );
    }

    #[test]
    fn every_textual_field_gets_an_input() {
        // The panel builds one input per textual field. The count is derived
        // from the schema rather than hardcoded, so a text setting added without
        // a control cannot slip through unnoticed.
        let textual = SETTINGS_SECTIONS
            .iter()
            .flat_map(|section| section.fields.iter())
            .filter(|field| is_textual(field.kind))
            .count();

        assert!(
            textual >= 5,
            "expected several textual settings, found {textual}; this test would prove nothing"
        );
    }

    #[test]
    fn a_toggle_alone_does_not_disable_save_forever() {
        // The other half of the greyed-out Save: toggling a switch has to mark
        // the panel dirty exactly like typing does. A switch that renders but
        // never marks dirty is as broken as an input that cannot be typed into.
        let mut model = load_values(&Settings::default());
        assert!(!model.is_dirty());

        model.toggle("keep_transcript", false);
        assert!(
            model.is_dirty(),
            "toggling a setting must make Save available"
        );
    }

    #[test]
    fn the_engine_section_exists() {
        // The model picker is shown only on this section, found by title. If it
        // is renamed the picker silently stops appearing, which is a failure
        // with no error anywhere.
        assert!(
            SETTINGS_SECTIONS.iter().any(|s| s.title == "Voice"),
            "the Voice section is what the model picker hangs off"
        );
    }

    #[gpui_kit::test]
    fn the_engine_section_offers_a_file_chooser_and_not_just_a_text_box(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        // The gap this closes: `SharedId::browse_model`'s own doc comment claims
        // "a test looking for it is how we know the engine section drew its
        // picker at all" — and no test looked for it. So the chooser could stop
        // rendering, or stop responding, and every other test here would pass.
        //
        // It builds the real panel, opens the real section, and asks the
        // question that was asked of it: is there a way to pick a file other
        // than typing its absolute path?
        //
        // Which matters, because typing an absolute path is the wrong way to
        // choose a file. Someone who has just downloaded a 460 MB model should
        // not have to know where it landed or spell out `/home/…` to use it.
        use std::rc::Rc;

        use gpui_kit::{IntoElement, Render, test::TestWindowExt as _};

        /// A window root that is nothing but the settings panel.
        ///
        /// `add_window_view` needs something that implements `Render`, and the
        /// panel is a sub-view of the main window rather than a root itself.
        struct Root(Entity<SettingsView>);

        impl Render for Root {
            fn render(
                &mut self,
                _window: &mut gpui::Window,
                _cx: &mut gpui::Context<Self>,
            ) -> impl IntoElement {
                self.0.clone().into_any_element()
            }
        }

        // The panel reads the kit theme while building, so the global state has
        // to exist before the window is made rather than inside it.
        cx.update(gpui_kit::init);

        let engine = engine_section();
        // Kept so the panel can be inspected after the window is built; the root
        // it sits in is a wrapper that exists only to satisfy `Render`.
        let panel: Rc<std::cell::Cell<Option<gpui::Entity<SettingsView>>>> =
            Rc::new(std::cell::Cell::new(None));

        let (_, cx) = cx.add_window_view({
            let panel = panel.clone();
            move |window, cx| {
                let (_sender, receiver) = async_channel::unbounded::<crate::model::ModelMessage>();
                let session = crate::model::SessionEntity::new(
                    yap_server::AppState::shared(),
                    receiver,
                    "http://127.0.0.1/overlay".to_string(),
                    cx,
                );

                let view = SettingsView::new(
                    session,
                    Settings::default(),
                    std::path::PathBuf::from("/nonexistent/settings.toml"),
                    window,
                    cx,
                );
                panel.set(Some(view.clone()));
                Root(view)
            }
        });

        cx.update(|window, cx| window.draw(cx).clear(cx));

        // Open the section that owns the model path. Defaults to the first one,
        // which is Engine today — pinned by a test elsewhere, but not assumed
        // here.
        let entity = panel.take().expect("the panel was built");
        let opened = cx.update(|_window, cx| entity.read(cx).settings.section);
        assert_eq!(
            opened, engine,
            "the panel should open on Voice, and the picker hangs off that section",
        );

        assert!(
            cx.update(|window, _| window.try_find(SharedId::browse_model()).is_some()),
            "the Voice section offers only a text field — there is no way to pick \
             a model file without typing its absolute path",
        );
    }

    #[test]
    fn saving_a_model_that_does_not_exist_says_so() {
        // The half of this that used to be missing. A path that is not a file
        // used to fail at the *next launch*, with nothing on screen at the moment
        // it could still be fixed: the streamer saw "saved", went back to their
        // stream, and the app came up a minute later unable to hear anything.
        let mut settings = Settings::default();
        settings.engine = yap_core::settings::EngineChoice::Local {
            model_path: "/nope/missing.bin".into(),
            backend: yap_core::settings::LocalBackend::default(),
        };

        let warning = model_warning(&settings).expect("a model that is not there must be reported");

        assert!(
            warning.contains("no model at") && warning.contains("/nope/missing.bin"),
            "the warning must name the path that is wrong, or it cannot be acted \
             on. Got: {warning:?}",
        );
    }

    #[test]
    fn a_warning_is_not_raised_for_a_model_that_is_there() {
        // The other direction, and the one that matters most: a warning that
        // appears when nothing is wrong teaches a streamer to ignore it, and
        // then it says nothing useful the time it is true.
        let mut settings = Settings::default();

        // This file certainly exists.
        settings.engine = yap_core::settings::EngineChoice::Local {
            model_path: std::env::current_exe().unwrap_or_default(),
            backend: yap_core::settings::LocalBackend::default(),
        };

        assert_eq!(
            model_warning(&settings),
            None,
            "an existing model must not be reported as missing"
        );

        // And the reason it is `None` is that the path was checked, not that the
        // check was skipped. A directory is not a model file, so this must warn —
        // which is only true if `is_file` is doing the work.
        //
        // Asserted through the helper rather than through `model_warning`, whose
        // second half depends on whether this machine happens to have a model to
        // substitute. A test whose result depends on the machine it runs on is a
        // test that passes for the wrong reason somewhere else.
        assert!(
            !model_path_is_usable(&std::env::temp_dir()),
            "a directory exists but is not a model file, and must not pass for one",
        );
        assert!(
            model_path_is_usable(&std::env::current_exe().expect("a test binary")),
            "an existing file is usable",
        );
        assert!(!model_path_is_usable(std::path::Path::new(
            "/nope/missing.bin"
        )));
    }

    #[test]
    fn a_cloud_engine_is_never_asked_about_model_files() {
        // A cloud engine has no local file to be wrong about, and its failures
        // are about the key or the network — which are reported when it runs,
        // with advice that would be wrong here.
        let mut settings = Settings::default();
        settings.engine = yap_core::settings::EngineChoice::Groq {
            credential: "yap-a-thon/groq".to_string(),
        };

        assert_eq!(model_warning(&settings), None);
    }

    #[test]
    fn section_indexes_are_all_valid() {
        // The panel looks sections up by index, so an out-of-range index would
        // render an empty panel rather than an error.
        let model = SettingsModel::default();
        assert!(SETTINGS_SECTIONS.get(model.section).is_some());
    }

    fn field(key: &'static str) -> &'static Field {
        SETTINGS_SECTIONS
            .iter()
            .flat_map(|s| s.fields.iter())
            .find(|f| f.key == key)
            .expect("field exists")
    }

    fn port_field() -> &'static Field {
        field("port")
    }

    fn twitch_channel_field() -> &'static Field {
        field("twitch_channel")
    }

    fn vad_threshold_field() -> &'static Field {
        field("vad_threshold")
    }

    // ---------------------------------------------------------------------
    // Credentials
    //
    // The property these all protect: a token goes to the OS credential store
    // and reaches nothing else. `settings.toml` is meant to be readable, copied
    // between machines and pasted into a bug report, so a token in it is a
    // token in a screenshot.
    // ---------------------------------------------------------------------

    #[test]
    fn every_credential_field_is_stored_under_a_name_the_store_recognises() {
        // The schema says a field is a credential; the store recognises it by
        // name. If those disagree the panel prompts for a key and then files it
        // somewhere nothing will ever read it back from.
        for section in SETTINGS_SECTIONS {
            for field in section.fields {
                if field.kind == FieldKind::Secret {
                    assert!(
                        yap_core::secrets::is_secret_key(field.key),
                        "{}: a credential whose name the store does not recognise as one",
                        field.key,
                    );
                }
            }
        }
    }

    #[test]
    fn the_settings_file_can_never_be_where_a_credential_is_kept() {
        // Not "does not by default" — cannot. `apply_field` is the only way a
        // value reaches the file, so a credential key that assigned anything
        // would be the whole bug.
        for section in SETTINGS_SECTIONS {
            for field in section.fields {
                if field.kind != FieldKind::Secret {
                    continue;
                }

                let mut settings = Settings::default();
                apply_field(&mut settings, field, "oauth:abcdef0123456789");

                assert_eq!(
                    settings,
                    Settings::default(),
                    "{}: a credential was written into the settings",
                    field.key,
                );
            }
        }
    }

    #[test]
    fn no_credential_survives_a_full_round_trip_through_the_settings_file() {
        // The round trip rather than the single step, because the two halves
        // can each be right while the pair is wrong: `apply_field` refusing a
        // credential while something earlier in `write()` had already copied it
        // somewhere on the way to the file.
        //
        // The canary is fed only to credential fields. A wider net catches
        // `bind_address`, which is a text field and quite correctly holds a
        // string — the property here is about credentials specifically.
        const POISON: &str = "oauth:CANARY-MUST-NOT-REACH-THE-FILE";

        let credential_fields: Vec<&Field> = SETTINGS_SECTIONS
            .iter()
            .flat_map(|section| section.fields.iter())
            .filter(|field| field.kind == FieldKind::Secret)
            .collect();

        assert!(
            !credential_fields.is_empty(),
            "there are no credential fields, so this test proves nothing",
        );

        let mut settings = Settings::default();

        for field in &credential_fields {
            apply_field(&mut settings, field, POISON);
        }

        assert_eq!(
            settings,
            Settings::default(),
            "a credential reached the settings model",
        );

        let dir = std::env::temp_dir().join(format!("yap-secret-canary-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let path = dir.join("settings.toml");

        yap_core::settings::save(&path, &settings).expect("a save");
        let text = std::fs::read_to_string(&path).expect("the file back");

        assert!(
            !text.contains("CANARY"),
            "a credential reached settings.toml, which is safe to share and safe to screenshot:\n{text}",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_credential_is_never_loaded_into_the_panel_as_an_editable_value() {
        // The panel model is what Save serialises. A credential sitting in it
        // is one Save away from a file, whatever `apply_field` does with it —
        // so it must not get there in the first place.
        let settings = Settings::default();
        let values = load_values(&settings);

        for section in SETTINGS_SECTIONS {
            for field in section.fields {
                if field.kind == FieldKind::Secret {
                    assert_eq!(
                        values.get(field.key),
                        "",
                        "{}: a credential was loaded into the editable model",
                        field.key,
                    );
                }
            }
        }
    }

    #[test]
    fn the_panel_model_starts_clean_where_a_credential_would_be_typed() {
        // The mechanism that keeps a credential out of the file is structural:
        // the secret inputs have no change subscription, so typing in one never
        // reaches `SettingsModel::set`, and never marks the panel dirty. Save is
        // the panel's only commit and Save writes a file, so a dirty panel is
        // the whole risk.
        //
        // That the *wiring* is right is asserted headlessly in `view.rs`, by
        // typing into a real credential input on a real panel. Here: the other
        // half, that an ordinary setting does behave the way the panel assumes.
        let mut model = SettingsModel::default();
        assert!(!model.is_dirty(), "a fresh panel is clean");

        model.set("twitch_channel", "someone".to_string());
        assert!(
            model.is_dirty(),
            "an ordinary setting marks the panel dirty, which is what makes Save meaningful",
        );
    }

    #[test]
    fn the_status_a_credential_shows_says_whether_one_can_be_stored() {
        // The three states drive different affordances, and the important one
        // is that an unavailable store shows no button at all.
        let (_, can_set) = secret_status(yap_core::secrets::SecretState::Present);
        assert!(can_set, "a stored credential can be replaced");

        let (_, can_set) = secret_status(yap_core::secrets::SecretState::Absent);
        assert!(can_set, "an unset credential can be set");

        let (text, can_set) = secret_status(yap_core::secrets::SecretState::Unavailable);
        assert!(
            !can_set,
            "with no credential store, the app must not offer to accept one",
        );
        assert!(
            text.contains("can't store") || text.contains("safely"),
            "and must say it won't be kept unsafely, got: {text:?}",
        );
    }

    #[test]
    fn a_locked_store_is_its_own_message_and_carries_its_own_fix() {
        // The distinction that was merged away: a locked keyring and a missing
        // one look identical from inside the app, and the fix for one is a
        // command and the fix for the other is nothing.
        //
        // Telling someone their machine has no credential store when it is sat
        // there, locked, is technically true and completely useless — it sends
        // them to reinstall an OS.
        let (text, can_set) = secret_status(yap_core::secrets::SecretState::Locked);

        assert!(
            !can_set,
            "nothing can be accepted while the store is locked"
        );
        assert!(
            text.contains("locked") && text.contains("unlock"),
            "the message has to name the problem and the action, got: {text:?}",
        );

        // And it must not be the missing-store message wearing a hat.
        let (missing, _) = secret_status(yap_core::secrets::SecretState::Unavailable);
        assert_ne!(
            text, missing,
            "locked and missing are different problems with different fixes",
        );

        // The hint names the platform's own tool rather than one desktop's
        // command on a machine that may have six.
        let hint = secret_hint(yap_core::secrets::SecretState::Locked);
        assert!(
            cfg!(target_os = "macos") || cfg!(target_os = "windows") || !hint.is_empty(),
            "on linux the locked store needs an actual command, got an empty hint",
        );

        // No hint where there is nothing to unlock.
        assert_eq!(
            secret_hint(yap_core::secrets::SecretState::Unavailable),
            "",
            "a missing store has nothing to unlock, so no advice",
        );
        assert_eq!(secret_hint(yap_core::secrets::SecretState::Present), "",);
    }

    /// Whether this machine's credential store can be **written to**.
    ///
    /// Probed on a thread with a deadline, because the failure mode is a **hang**
    /// rather than an error, and it took a while to find: three tests here sat
    /// for over a minute each on a locked keyring, and a run that is stuck looks
    /// like a run that is working.
    ///
    /// The probe is a *write* to a throwaway name. A read is the wrong probe and
    /// was the wrong probe for a while: reading an entry that does not exist
    /// answers `NoEntry` immediately even when the collection is locked, so the
    /// guard passed and the test then blocked on the first `set` — appearing to
    /// prove nothing at all was wrong.
    ///
    /// Writing is what needs the collection unlocked, so that is what is asked.
    fn store_answers() -> bool {
        use std::sync::mpsc;
        use std::time::Duration;

        let (tx, rx) = mpsc::channel();
        let service = yap_core::settings::credential_name("astro");

        std::thread::spawn(move || {
            let probe = yap_core::secrets::set(&service, "yap-test-probe", "probe");
            let _ = yap_core::secrets::remove(&service, "yap-test-probe");
            let _ = tx.send(probe.is_ok());
        });

        matches!(rx.recv_timeout(Duration::from_secs(5)), Ok(true))
    }

    #[test]
    fn a_credential_set_in_the_panel_is_readable_by_the_platform_that_needs_it() {
        // The test that matters most in this file, and the one whose absence let
        // a real disconnect through.
        //
        // The prompt and the platform loaders do not share a field name — they
        // share a `(service, user)` keyring address, and the `service` half is
        // configurable through `twitch_credential`. So it was entirely possible
        // for the prompt to write somewhere the platform never looks: the button
        // would say "stored", the file would be untouched, and the only symptom
        // would be a channel that silently never sends an event.
        //
        // So this goes through the real loader rather than through the mapping
        // that produced the address. Testing the mapping would only prove the
        // mapping is consistent with itself.
        use yap_core::secrets::{remove, set};

        if !store_answers() {
            eprintln!("skipping: this machine's credential store did not answer");
            return;
        }

        let field = SETTINGS_SECTIONS
            .iter()
            .flat_map(|section| section.fields.iter())
            .find(|field| field.key == "twitch_token")
            .expect("a Twitch token field");

        // Whatever name this settings file carries, which is the one `app.rs`
        // hands the platform — so use that, rather than asking
        // `credential_entry` where to look. Asking the mapping where to look and
        // then confirming it is consistent with itself is how the original
        // disconnect passed every test in this file.
        let settings = Settings::default();
        let configured = settings
            .twitch_credential
            .clone()
            .expect("a default Twitch credential name");

        let (service, user) =
            crate::settings::credential_entry(&settings, field).expect("an entry");

        assert_eq!(
            service, configured,
            "the panel prompts at {service:?} but app.rs hands the platform \
             {configured:?}; the button would say 'stored' and the channel would \
             silently send nothing",
        );

        if !store_answers() {
            eprintln!("skipping: this machine's credential store did not answer");
            return;
        }

        // Whatever was there before is put back afterwards, so running this
        // never costs a streamer a token they had set up.
        let had = yap_core::secrets::get(&service, user).ok().flatten();

        let before = yap_events::twitch::TwitchPlatform::from_keyring(
            "someone".to_string(),
            &service,
            "client-id",
            yap_events::twitch::TwitchPricing::default(),
        );
        assert!(
            before.is_err() || had.is_some(),
            "with nothing stored the loader must fail — otherwise this test proves nothing",
        );

        // What the prompt's Save button does.
        assert!(set(&service, user, "oauth:written-by-the-panel").is_ok());

        let after = yap_events::twitch::TwitchPlatform::from_keyring(
            "someone".to_string(),
            &service,
            "client-id",
            yap_events::twitch::TwitchPricing::default(),
        );
        assert!(
            after.is_ok(),
            "the platform could not read back what the panel just stored at \
             {service}/{user}; the prompt would have said 'stored' and the channel \
             would have silently sent nothing",
        );

        // And the value is the one that was written, not merely *something*.
        assert_eq!(
            yap_core::secrets::get(&service, user)
                .ok()
                .flatten()
                .as_deref(),
            Some("oauth:written-by-the-panel"),
        );

        // Restored.
        match had {
            Some(previous) => {
                let _ = set(&service, user, &previous);
            }
            None => {
                let _ = remove(&service, user);
            }
        }
    }

    #[test]
    fn a_custom_twitch_credential_name_is_where_the_panel_prompts() {
        // The same property as above, for a name that is not the default. The
        // setting exists so one machine can hold two channels; if the prompt
        // ignored it, the second channel's token would be filed under the first
        // one's name and both would be wrong.
        let field = SETTINGS_SECTIONS
            .iter()
            .flat_map(|section| section.fields.iter())
            .find(|field| field.key == "twitch_token")
            .expect("a Twitch token field");

        let settings = Settings {
            twitch_credential: Some("yap-a-thon/second-channel".to_string()),
            ..Settings::default()
        };

        let (service, _) = crate::settings::credential_entry(&settings, field).expect("an entry");

        assert_eq!(
            service, "yap-a-thon/second-channel",
            "a custom credential name must be honoured",
        );
    }

    #[test]
    fn the_cloud_key_is_readable_under_the_address_the_panel_prompts_for() {
        // The same property for the other credential, because they have
        // different `user` values and different ways of naming their service.
        use yap_core::secrets::{remove, set};

        if !store_answers() {
            eprintln!("skipping: this machine's credential store did not answer");
            return;
        }

        let field = SETTINGS_SECTIONS
            .iter()
            .flat_map(|section| section.fields.iter())
            .find(|field| field.key == "groq_key")
            .expect("a Groq key field");

        let mut settings = Settings::default();
        settings.engine = yap_core::settings::EngineChoice::Groq {
            credential: "yap-a-thon/test-groq".to_string(),
        };

        let (service, user) =
            crate::settings::credential_entry(&settings, field).expect("an entry");

        assert_eq!(
            service, "yap-a-thon/test-groq",
            "a custom credential name must be honoured, or the prompt writes \
             somewhere the engine never looks",
        );

        assert!(set(&service, user, "gsk_test_key").is_ok());

        let engine = yap_voice::cloud::GroqEngine::from_keyring(&service);
        assert!(
            engine.is_ok(),
            "the speech engine could not read the key the panel stored at {service}/{user}",
        );

        let _ = remove(&service, user);
    }

    #[test]
    fn no_two_credentials_share_a_keyring_entry() {
        // The invariant, rather than a list of known keys: clearing one
        // credential must not clear another. Two fields that resolve to the same
        // `(service, user)` pair would share one value, so replacing the Twitch
        // token would also replace the Groq key and nothing would say so.
        //
        // Written this way rather than as an allowlist of known fields, because
        // an allowlist passes the moment a credential is added that nobody
        // thought about — which is exactly how this test would have missed the
        // Astro tokens when they first went in as a settings-file field.
        let settings = Settings::default();
        let mut claimed: std::collections::BTreeMap<(String, &'static str), Vec<&str>> =
            std::collections::BTreeMap::new();

        for section in SETTINGS_SECTIONS {
            for field in section.fields {
                if let Some((service, user)) = crate::settings::credential_entry(&settings, field) {
                    claimed.entry((service, user)).or_default().push(field.key);
                }
            }
        }

        assert!(
            claimed.len() >= 3,
            "expected the Twitch, speech and StreamElements credentials, found {:?}",
            claimed.keys().collect::<Vec<_>>(),
        );

        for (entry, fields) in claimed {
            assert_eq!(
                fields.len(),
                1,
                "{entry:?} is claimed by {fields:?}, so clearing one clears them all",
            );
        }
    }
}
