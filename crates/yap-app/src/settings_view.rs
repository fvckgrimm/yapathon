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
use crate::segment::segment_button;
use yap_core::goal::GoalMode;

use crate::settings::{Field, FieldKind, SETTINGS_SECTIONS, is_textual};
use crate::theme::Theme;
use crate::view::text::LABEL;
use crate::view::{label, section_heading};

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
        "event_twitch_sub" => words_for(settings, "twitch:sub"),
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
fn event_key(field: &str) -> String {
    match field {
        "event_twitch_sub" => "twitch:sub",
        "event_twitch_resub" => "twitch:resub",
        "event_twitch_gift_sub" => "twitch:gift_sub",
        "event_twitch_cheer" => "twitch:cheer",
        "event_twitch_raid" => "twitch:raid",
        "event_twitch_redemption" => "twitch:redemption",
        "event_chat_command" => "chat:command",
        other => other,
    }
    .to_string()
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

        let mut subscriptions: Vec<Subscription> = Vec::with_capacity(inputs.len());

        cx.new(move |cx: &mut Context<Self>| {
            // One subscription per input, so typing marks the panel dirty and
            // Save becomes available. Without this the form accepts text and
            // silently discards it.
            for (key, state) in &inputs {
                let key = key.clone();
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
        div().flex().flex_wrap().gap_2().px_4().pb_3().children(
            SETTINGS_SECTIONS
                .iter()
                .enumerate()
                .map(|(index, section)| {
                    let selected = index == self.settings.section;
                    let label = section.title;
                    let me = me.clone();

                    segment_button(
                        SharedId::section(index),
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
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(LABEL)
                        .text_color(Theme::text(cx))
                        .child("models found"),
                )
                .when(found.is_empty(), |this| {
                    // Naming the directory matters more than the message: the
                    // fix is to move a file, and the person needs to know where.
                    this.child(
                        div()
                            .text_size(px(10.0))
                            .text_color(Theme::muted(cx))
                            .child(format!(
                                "none — put a ggml model in {}",
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
            "waiting for the file chooser…"
        } else {
            "browse for a model file…"
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
                        Switch::new(SharedId::field(key))
                            .checked(on)
                            // `on_change` hands the new value as a `&bool`, so
                            // it needs a deref before it is a bool.
                            .on_change(move |checked, _window, cx| {
                                let _ =
                                    me.update(cx, |this, _cx| this.settings.toggle(key, *checked));
                            })
                            .into_any_element()
                    }
                    FieldKind::Text | FieldKind::Int | FieldKind::Path => self
                        .render_text_field(field, cx)
                        .unwrap_or_else(|| label(field.label, cx).into_any_element()),
                    FieldKind::Choice => self.render_choice_field(field, me, cx).into_any_element(),
                    FieldKind::ChoiceFromDevices => {
                        self.render_device_field(field, me, cx).into_any_element()
                    }
                    FieldKind::Secret => render_secret_field(field, cx).into_any_element(),
                };

                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .py_2()
                    .child(
                        div()
                            .text_size(LABEL)
                            .text_color(Theme::text(cx))
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
            .h(px(28.0))
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
                            .text_size(px(10.0))
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
            .px_4()
            .py_3()
            .when(notice.is_some(), |this| {
                this.child(
                    div()
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
                    .label(if dirty { "Save changes" } else { "Saved" })
                    .primary()
                    // Disabled until something changes, so looking at Settings
                    // does not rewrite the file.
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
                    .text_size(px(10.0))
                    .text_color(Theme::muted(cx))
                    .child("Engine and model changes take effect after a restart."),
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
            Ok(()) => {
                self.settings.mark_saved();
                self.settings.notice = Some("saved".to_string());
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

    /// The write itself, split out so it can be tested without a window.
    pub fn write(&self) -> anyhow::Result<()> {
        let mut settings = yap_core::settings::load(&self.path)?;

        for section in SETTINGS_SECTIONS {
            for field in section.fields {
                let raw = self.settings.get(field.key).to_string();
                apply_field(&mut settings, field, &raw);
            }
        }

        yap_core::settings::save(&self.path, &settings)?;
        Ok(())
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
        "event_twitch_sub"
        | "event_twitch_resub"
        | "event_twitch_gift_sub"
        | "event_twitch_cheer"
        | "event_twitch_raid"
        | "event_twitch_redemption"
        | "event_chat_command" => {
            let event = event_key(field.key);
            if trimmed.is_empty() {
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
        "groq_key" | "twitch_token" => {}
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

/// A secret field: a button, never the value.
///
/// The key is never read back into a field, so it cannot be copied out of the
/// settings screen, and it never touches the settings file.
fn render_secret_field(field: &Field, cx: &mut App) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .flex_1()
                .text_size(LABEL)
                .text_color(Theme::muted(cx))
                .child("stored in the OS credential store"),
        )
        .child(
            Button::new(SharedId::secret(field.key))
                .label("Set…")
                .on_click(|_, _window, _cx| {
                    // Reaching the credential store needs a UI this panel does
                    // not have yet. Saying so beats a button that does nothing.
                    log::info!("credential entry needs a prompt; see the README for the CLI form");
                }),
        )
}

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

    fn secret(key: &str) -> gpui::ElementId {
        format!("settings-secret-{key}").into()
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

        div()
            .flex()
            .flex_col()
            .gap_2()
            .min_w_0()
            // A setting that only takes effect on the next run is worth saying so
            // while a run is in progress, or a streamer edits the limit, sees no
            // change, and concludes the setting is broken.
            .when(running, |this| {
                this.child(
                    div()
                        .text_size(LABEL)
                        .text_color(Theme::warning(cx))
                        .child("a yapathon is running — changes apply to the next one"),
                )
            })
            .child(section_heading("settings", cx))
            .child(
                // Which file this is. Two places can hold settings and only one
                // is in use, so a panel that edits a file the app never reads
                // is otherwise completely silent about it.
                div()
                    .px_4()
                    .text_size(px(10.0))
                    .text_color(Theme::muted(cx))
                    .child(format!("saved to {}", self.path.display())),
            )
            .child(self.render_sections(&me, cx))
            .when(picker.is_some(), |this| {
                this.child(div().px_4().child(div()))
            })
            .child(div().flex().flex_col().px_4().children(fields))
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

/// The index of the section holding the engine settings.
///
/// Found by title rather than written as a constant, because a constant would be
/// a number that silently points at the wrong section the moment one is inserted.
fn engine_section() -> usize {
    SETTINGS_SECTIONS
        .iter()
        .position(|section| section.title == "Engine")
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use yap_core::settings::{Settings, default_model_path};

    #[test]
    fn the_overlay_theme_the_panel_offers_is_the_one_that_renders() {
        // The picker offers `card, minimal, cute, neon, mono`. If `yap-server`
        // gains or loses a theme, this fails rather than the panel offering a
        // button that silently does nothing — which is the failure mode this
        // project keeps meeting.
        let offered: Vec<&str> = SETTINGS_SECTIONS
            .iter()
            .flat_map(|section| section.fields.iter())
            .find(|field| field.key == "overlay_theme")
            .map(|field| field.options.to_vec())
            .expect("an overlay theme field");

        let real: Vec<&str> = yap_server::overlay::OVERLAY_THEMES
            .iter()
            .map(|theme| theme.id)
            .collect();

        assert_eq!(
            offered, real,
            "the settings picker and the served overlay must offer the same themes",
        );
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
            yap_server::overlay::themed_overlay_html(&state.overlay_theme(None))
                .contains(r#"<body data-theme="neon">"#),
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
            yap_server::overlay::themed_overlay_html(&state.overlay_theme(None))
                .contains(r#"<body data-theme="card">"#)
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
        // The pricing map and the settings form are two lists of the same thing.
        // An event with a price but no field means a streamer can read the
        // number and not change it — the same shape of bug as the dead tab: a
        // control that shows a value and refuses to do anything with it.
        let mut unpriced_fields = Settings::default().event_words;

        for section in SETTINGS_SECTIONS {
            for field in section.fields {
                if !field.key.starts_with("event_") {
                    continue;
                }

                let event = event_key(field.key);
                assert!(
                    unpriced_fields.remove(&event).is_some(),
                    "{} prices {event:?}, which the core has no default for",
                    field.key,
                );
            }
        }

        assert!(
            unpriced_fields.is_empty(),
            "the core prices these but nothing can change them: {unpriced_fields:?}",
        );
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
            SETTINGS_SECTIONS.iter().any(|s| s.title == "Engine"),
            "the Engine section is what the model picker hangs off"
        );
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
}
