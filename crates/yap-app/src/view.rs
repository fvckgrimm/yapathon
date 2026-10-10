//! Drawing the window.
//!
//! The dock is a streamer's tool, so it is dense and numeric: the count, what is
//! left, the rate, the controls, in that order of what matters at a glance
//! during a stream. The audit trail and the event feed sit below them because
//! they are for when a number is disputed, not for the common case.
//!
//! Colour follows state, not decoration. One tone for counting, one for paused,
//! one for finished, so a streamer who looks away from the screen still knows
//! which state they are in from the edge of their vision.

// `InteractiveElement` supplies `.id(..)`, which is what turns a `Div` into the
// `Stateful<Div>` the scroll helpers live on. `FluentBuilder` supplies `.when`.
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use gpui_kit::Subscription;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputState;

use yap_core::goal::GoalMode;

use crate::model::{SessionEntity, SessionModel};
use crate::segment::segmented;
use crate::settings_view::SettingsView;
use crate::theme;
use crate::theme::Theme;
use crate::view::style::{StatusTone, tone_for};
use crate::view::text::{HEADLINE, LABEL, VALUE};

/// Font sizes, named.
///
/// Sizes used more than once get a name so a change lands everywhere. They are
/// `Pixels` rather than a scale step because this is a numeric tool: the headline
/// is sized against the digits it holds, not against a type scale.
pub(crate) mod text {
    use gpui::px;

    /// A big number.
    pub const HEADLINE: gpui::Pixels = px(48.0);
    /// A figure that needs reading, not scanning.
    pub const VALUE: gpui::Pixels = px(17.0);
    /// Supporting text.
    pub const LABEL: gpui::Pixels = px(12.0);
}

/// The window's root.
pub struct YapView {
    session: Entity<SessionEntity>,
    /// The settings panel, once it has been opened.
    ///
    /// An entity because it holds unsaved edits: a settings form whose state
    /// lives in a render closure loses everything the streamer typed on the first
    /// redraw, which is every keystroke that triggers one.
    settings: Option<Entity<SettingsView>>,
    /// Which panel the window is showing.
    tab: Tab,
    /// The run form: mode, goal, limit.
    ///
    /// These were `String`s that nothing was bound to, so the Start button could
    /// only ever start a fixed 5,000-word run and there was no way to choose a
    /// mode. An app you manage yapathons from has to let you set them.
    form: RunForm,
    /// The manual-correction form.
    correction: CorrectionForm,
    /// Whether the overlay URL was just copied.
    ///
    /// Held so the row can say so. A click handler with no visible result is
    /// indistinguishable from a click handler that is not wired up, which is
    /// exactly the class of bug this file has been fighting.
    copied_url: bool,
    /// A weak handle to this view, for click handlers.
    ///
    /// Set during construction from the `Context<Self>` that `cx.new` provides,
    /// and used by the tab buttons: their handlers receive `&mut App` rather than
    /// a `Context<Self>`, so they cannot reach the view without a handle to it.
    me: gpui::WeakEntity<Self>,
    /// Kept alive so the observation above survives.
    ///
    /// `cx.observe` returns a subscription that unsubscribes on drop, so
    /// dropping it means the view stops updating — which looks exactly like a
    /// broken app rather than a dropped listener.
    _observe: Subscription,
}

/// The editable fields for starting a yapathon.
///
/// One struct rather than loose fields, because the three values are always read
/// together when Start is pressed and keeping them together makes it obvious
/// when one of them is not being set.
pub struct RunForm {
    mode: RunMode,
    goal: Entity<InputState>,
    limit: Entity<InputState>,
}

/// The mode a new yapathon will use.
///
/// Buttons rather than a dropdown, because there are three of them and each one
/// is a mode you actually want to see labelled rather than infer from a
/// collapsed control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// Chat moves the finish line.
    Extend,
    /// The goal never moves; events grant score.
    Fixed,
    /// The goal is a clock; events extend it.
    Time,
}

impl RunMode {
    const ALL: [RunMode; 3] = [RunMode::Extend, RunMode::Fixed, RunMode::Time];

    fn label(self) -> &'static str {
        match self {
            Self::Extend => "chat extends goal",
            Self::Fixed => "fixed goal",
            Self::Time => "chat extends clock",
        }
    }

    /// The equivalent [`GoalMode`], so a chosen mode is the one that runs.
    ///
    /// The mapping is the whole point of the picker: a mode that displays as
    /// "chat extends clock" but starts an extend run would be worse than no
    /// picker at all.
    pub fn goal_mode(self, goal: u32) -> GoalMode {
        match self {
            Self::Extend => GoalMode::Extend { base_words: goal },
            Self::Fixed => GoalMode::Fixed { words: goal },
            Self::Time => GoalMode::Time {
                base: std::time::Duration::from_secs(u64::from(goal)),
            },
        }
    }

    /// What this mode does, for the one-line explanation under the buttons.
    ///
    /// Shown because "chat extends goal" is not obviously the same thing as
    /// "the goal grows", and picking the wrong one ends a yapathon wrongly.
    fn explain(self) -> &'static str {
        match self {
            Self::Extend => "every sub, cheer and raid adds words to the goal",
            Self::Fixed => "the goal never moves; events only add score",
            Self::Time => "the goal is a clock; events push the finish further away",
        }
    }

    fn from_short_name(name: &str) -> Option<Self> {
        match name.trim().to_lowercase().as_str() {
            "extend" => Some(Self::Extend),
            "fixed" => Some(Self::Fixed),
            "time" => Some(Self::Time),
            _ => None,
        }
    }
}

/// The manual-correction form.
pub struct CorrectionForm {
    words: Entity<InputState>,
    note: Entity<InputState>,
    /// An absolute target, for the "set the count to exactly this" case.
    set_to: Entity<InputState>,
}

/// Round-number adjustments offered as one-click buttons.
///
/// Ordered smallest first so the row reads as a scale rather than a set. The
/// subtraction is 100 and the additions 10 through 100 because those are the
/// amounts a gifted sub or a mod-double is worth; a correction nobody needs
/// twice is not worth a button.
const CORRECTION_PRESETS: [(i64, &str); 5] = [
    (10, "+10"),
    (25, "+25"),
    (50, "+50"),
    (100, "+100"),
    (-100, "\u{2212}100"),
];

/// Which panel the window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Run,
    Settings,
}

impl Tab {
    const ALL: [Tab; 2] = [Tab::Run, Tab::Settings];

    fn label(self) -> &'static str {
        match self {
            Tab::Run => "Run",
            Tab::Settings => "Settings",
        }
    }
}

/// The label for the goal input, which changes unit with the mode.
///
/// A clock measured in "words" is a confusing thing to type into.
fn goal_label(mode: RunMode) -> &'static str {
    match mode {
        RunMode::Time => "Clock (seconds)",
        _ => "Goal (words)",
    }
}

fn limit_label(mode: RunMode) -> &'static str {
    match mode {
        RunMode::Time => "Max seconds",
        _ => "Max words",
    }
}

impl YapView {
    pub fn new(
        session: Entity<SessionEntity>,
        settings: yap_core::settings::Settings,
        settings_path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        // The form starts from the configured defaults, so the app opens showing
        // what a Start would actually do rather than a hardcoded guess.
        let default_mode =
            RunMode::from_short_name(settings.default_mode.short_name()).unwrap_or(RunMode::Extend);
        let default_goal = settings
            .default_mode
            .base_words()
            .unwrap_or(5_000)
            .to_string();
        let default_limit = settings
            .default_hard_limit
            .map(|limit| limit.to_string())
            .unwrap_or_default();

        cx.new(|cx| {
            let me = cx.weak_entity();

            // Observe the session entity, not just read it.
            //
            // A read is a momentary look; an observation is a subscription to
            // every future change. Without this the view renders once at
            // startup and then never again — the counter stays at zero, the
            // controls appear inert, and nothing errors. It is the single most
            // confusing failure a GPUI view can have, because every piece looks
            // correct in isolation.
            let _observe = cx.observe(&session, |_, _, cx| cx.notify());

            // Input states need a `Window`, which is why they are built here
            // rather than being plain strings read at Start time. A string with
            // nothing bound to it is a field that cannot be typed into — which
            // is what the goal and limit used to be.
            let mut input = |value: &str| -> Entity<InputState> {
                cx.new(|cx| {
                    let mut state = InputState::new(window, cx);
                    state.set_value(value.to_string(), window, cx);
                    state
                })
            };

            let form = RunForm {
                mode: default_mode,
                goal: input(&default_goal),
                limit: input(&default_limit),
            };

            let correction = CorrectionForm {
                words: input("0"),
                note: input(""),
                set_to: input(""),
            };

            Self {
                settings: Some(SettingsView::new(
                    session.clone(),
                    settings,
                    settings_path,
                    window,
                    cx,
                )),
                session,
                me,
                tab: Tab::Run,
                form,
                correction,
                copied_url: false,
                _observe,
            }
        })
    }

    fn render_header(&self, model: &SessionModel, cx: &mut App) -> impl IntoElement {
        let tone = tone_for(model);
        let unit = format!("{} {}", model.headline_unit(), model.goal_line());

        div()
            .flex()
            .flex_col()
            .px_4()
            .pt_4()
            .pb_3()
            .child(label(format!("yap-a-thon · {}", model.status_label()), cx))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_2()
                    .child(
                        div()
                            .text_size(HEADLINE)
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(tone.text(cx))
                            .child(model.headline()),
                    )
                    .child(label(unit, cx)),
            )
            .child(progress_bar(model.fraction(), tone, cx))
    }

    /// The Run / Settings switch.
    ///
    /// `me` is a weak handle to this view, captured during render, because the
    /// click handler receives `&mut App` rather than a `Context<Self>`. Weak so a
    /// button's closure cannot keep the window alive after it closes.
    fn render_tabs(&self, me: &gpui::WeakEntity<Self>, _cx: &mut App) -> impl IntoElement {
        let me = me.clone();

        segmented(
            Tab::ALL.into_iter().map(|tab| (tab, tab.label())),
            self.tab,
            move |tab, cx| {
                let _ = me.update(cx, |this, _cx| this.tab = tab);
            },
        )
    }

    /// The settings panel.
    ///
    /// Held as an entity so its unsaved edits survive redraws. Rendered through
    /// its own `Render`, which means the panel is one component rather than a
    /// block of this file.
    ///
    /// The session entity is carried for the panel's own future needs; nothing
    /// reads it yet, so it is deliberately kept rather than threaded through.
    fn render_settings(&self, _cx: &mut gpui::App) -> Option<gpui::AnyElement> {
        self.settings.clone().map(|panel| panel.into_any_element())
    }

    fn render_stats(&self, model: &SessionModel, cx: &mut App) -> impl IntoElement {
        div()
            .flex()
            .gap_6()
            .px_4()
            .pb_3()
            .child(stat("to go", model.remaining(), cx))
            .child(stat("rate", model.rate(), cx))
            .child(stat("elapsed", model.elapsed(), cx))
            .child(stat("from chat", model.from_chat(), cx))
            .child(stat("fillers", model.fillers(), cx))
    }

    fn render_notices(&self, model: &SessionModel, cx: &mut App) -> impl IntoElement {
        let mut row = div().flex().flex_col().gap_1().px_4().pb_3();

        if let Some(error) = &model.mic_error {
            row =
                row.child(label(format!("microphone: {error}"), cx).text_color(Theme::danger(cx)));
        }

        // The recogniser falling behind is worth saying out loud. A stale count
        // presented as current is worse than a visible delay.
        if model.behind {
            row = row.child(
                label("catching up — the counter is behind your voice", cx)
                    .text_color(Theme::warning(cx)),
            );
        }

        // Likewise a capped goal: without this, chat keeps extending a run that
        // ended by design.
        if model.is_capped() {
            row = row.child(
                label("goal capped — no more events can extend it", cx)
                    .text_color(Theme::danger(cx)),
            );
        }

        row
    }

    /// Mode, goal and limit — everything needed to start a run.
    fn render_start_form(&self, _model: &SessionModel, cx: &mut App) -> impl IntoElement {
        let me = self.me.clone();

        let modes = segmented(
            RunMode::ALL
                .into_iter()
                .map(|mode| (mode, mode.label().to_string())),
            self.form.mode,
            move |mode, cx| {
                let _ = me.update(cx, |this, _cx| this.form.mode = mode);
            },
        );

        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .pb_3()
            .child(modes)
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(Theme::muted(cx))
                    .child(self.form.mode.explain()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(numbered_field(
                        goal_label(self.form.mode),
                        &self.form.goal,
                        cx,
                    ))
                    .child(numbered_field(
                        limit_label(self.form.mode),
                        &self.form.limit,
                        cx,
                    )),
            )
    }

    /// The manual correction form.
    ///
    /// For the two cases no automatic method covers: a stretch the recogniser
    /// missed entirely, and one it misheard that should not have counted. Paired
    /// with Undo for the common case, this is the escape hatch.
    fn render_correction_form(&self, model: &SessionModel, cx: &mut App) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .pb_4()
            .child(section_heading("manual correction", cx))
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(Theme::muted(cx))
                    .child(
                        "Undo handles one misheard utterance. This is for a stretch that was \
                         missed, or a correction you want recorded. The note goes in the log.",
                    ),
            )
            // Presets first, because most corrections are round numbers and
            // typing "250" into a field to add 250 is the slowest way to say it.
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .children(CORRECTION_PRESETS.map(|(delta, label)| {
                        Button::new(format!("preset-delta-{delta}"))
                            .label(label)
                            .on_click({
                                let session = self.session.clone();
                                move |_, _window, cx| {
                                    command(
                                        &session,
                                        cx,
                                        Command::Adjust {
                                            words: delta,
                                            note: format!("preset {delta:+}"),
                                        },
                                    );
                                }
                            })
                            .into_any_element()
                    }))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(Theme::muted(cx))
                            .child("one click, no typing"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .items_center()
                    .child(numbered_field("Words", &self.correction.words, cx))
                    .child(
                        div().flex().flex_1().min_w(px(120.0)).child(
                            gpui_kit::component::input::Input::new(&self.correction.note)
                                .h(px(28.0))
                                .w_full(),
                        ),
                    ),
            )
            .child(
                Button::new("apply-correction")
                    .label("Apply correction")
                    .on_click({
                        let session = self.session.clone();
                        let me = self.me.clone();
                        move |_, window, cx| {
                            let parsed = me
                                .update(cx, |this, cx| {
                                    let words =
                                        parse_adjustment(&this.correction.words.read(cx).value());
                                    let note = this.correction.note(cx);

                                    // Reset the field, so applying twice does
                                    // not silently apply twice.
                                    this.correction.words.update(cx, |state, cx| {
                                        state.set_value("0".to_string(), window, cx)
                                    });

                                    (words, note)
                                })
                                .ok();

                            // A zero adjustment is a no-op rather than an error:
                            // pressing the button without typing anything is
                            // not a mistake worth shouting about.
                            if let Some((words, note)) = parsed
                                && words != 0
                            {
                                command(&session, cx, Command::Adjust { words, note });
                            }
                        }
                    }),
            )
            .child(self.render_set_to_row(model, cx))
    }

    /// "Set the count to exactly this."
    ///
    /// The case the delta form cannot cover: a hundred gifted subs extended the
    /// goal by some amount nobody agreed on, and the streamer knows the number
    /// it should have been. Making them work out the difference by hand, while a
    /// chat is counting up numbers at them, is how the wrong number gets
    /// accepted.
    ///
    /// Applied as a correction rather than as a new kind of edit, so it lands in
    /// the audit log with a note saying what it was — the same record every other
    /// correction leaves.
    fn render_set_to_row(&self, model: &SessionModel, cx: &mut App) -> impl IntoElement {
        let current = model
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.words)
            .unwrap_or(0);

        div()
            .flex()
            .flex_wrap()
            .gap_3()
            .items_center()
            .mt_2()
            .pt_3()
            .border_t_1()
            .border_color(Theme::border(cx))
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(Theme::muted(cx))
                    .w(px(140.0))
                    .child(format!("currently {current} words")),
            )
            .child(numbered_field(
                "Set the count to",
                &self.correction.set_to,
                cx,
            ))
            .child(Button::new("apply-set-to").label("Set it").on_click({
                let session = self.session.clone();
                let me = self.me.clone();
                move |_, window, cx| {
                    let parsed = me
                        .update(cx, |this, cx| {
                            let wanted = parse_target(&this.correction.set_to.read(cx).value());
                            let note = this.correction.explicit_note(cx);
                            this.correction
                                .set_to
                                .update(cx, |state, cx| state.set_value(String::new(), window, cx));
                            (wanted, note)
                        })
                        .ok();

                    // The delta is what the log records, so someone
                    // reading it later sees how far it moved, not just
                    // where it landed.
                    if let Some((Some(wanted), note)) = parsed {
                        let delta = i64::from(wanted) - i64::from(current);
                        if delta != 0 {
                            command(
                                &session,
                                cx,
                                Command::Adjust {
                                    words: delta,
                                    // What the streamer typed about it
                                    // wins; otherwise the log says what
                                    // the correction did.
                                    note: note.unwrap_or(format!("set to {wanted}")),
                                },
                            );
                        }
                    }
                }
            }))
    }

    fn render_controls(&self, model: &SessionModel, _cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.session.clone();

        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .px_4()
            .pb_4()
            .child(
                Button::new("start")
                    .label("Start")
                    .primary()
                    .disabled(!model.can_start())
                    .on_click({
                        let session = session.clone();
                        let me = self.me.clone();
                        move |_, _window, cx| {
                            let request = me.update(cx, |this, cx| this.form.request(cx)).ok();

                            if let Some(request) = request {
                                command(&session, cx, Command::Start(request));
                            }
                        }
                    }),
            )
            .child(
                Button::new("pause")
                    .label("Pause")
                    .disabled(!model.can_pause())
                    .on_click({
                        let session = session.clone();
                        move |_, _window, cx| command(&session, cx, Command::Pause)
                    }),
            )
            .child(
                Button::new("resume")
                    .label("Resume")
                    .disabled(!model.can_resume())
                    .on_click({
                        let session = session.clone();
                        move |_, _window, cx| command(&session, cx, Command::Resume)
                    }),
            )
            .child(
                Button::new("undo")
                    .label("Undo last")
                    .disabled(!model.can_undo())
                    .on_click({
                        let session = session.clone();
                        move |_, _window, cx| command(&session, cx, Command::Undo)
                    }),
            )
            .child(
                Button::new("end")
                    .label("End")
                    .danger()
                    .disabled(!model.can_start())
                    .on_click({
                        let session = session.clone();
                        move |_, _window, cx| command(&session, cx, Command::End)
                    }),
            )
    }

    fn render_events(&self, model: &SessionModel, cx: &mut App) -> impl IntoElement {
        let items: Vec<String> = model
            .events
            .iter()
            .rev()
            .take(12)
            .map(|event| {
                let who = event
                    .user
                    .as_deref()
                    .map(|user| format!(" — {user}"))
                    .unwrap_or_default();
                let words = match event.words {
                    Some(words) => format!("+{words} words"),
                    // An event with no price still reached the feed; saying "no
                    // effect" is more honest than hiding it.
                    None => "no effect".to_string(),
                };
                format!("{}:{}{} {words}", event.platform, event.kind, who)
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(section_heading("chat events", cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .max_h(px(180.0))
                    // `.id(..)` before `.overflow_y_scroll()`: the scroll
                    // helpers live on `Stateful<E>`, not on `Div`.
                    .id("events-scroll")
                    .overflow_y_scroll()
                    .when(items.is_empty(), |this| this.child(label("none yet", cx)))
                    .children(items.into_iter().map(|line| label(line, cx))),
            )
    }

    fn render_transcript(&self, model: &SessionModel, cx: &mut App) -> impl IntoElement {
        let lines: Vec<gpui::AnyElement> = model
            .transcript
            .iter()
            .rev()
            .take(60)
            .map(|line| {
                let muted = line.voided;
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .w(px(34.0))
                            .flex_shrink_0()
                            .text_size(LABEL)
                            .text_color(Theme::muted(cx))
                            .child(format!("{}w", line.words)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(LABEL)
                            .text_color(if muted {
                                Theme::muted(cx)
                            } else {
                                Theme::text(cx)
                            })
                            .child(line.text.clone()),
                    )
                    .into_any_element()
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(section_heading("transcript", cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .max_h(px(300.0))
                    .id("transcript-scroll")
                    .overflow_y_scroll()
                    .when(lines.is_empty(), |this| {
                        this.child(label("nothing said yet", cx))
                    })
                    .children(lines),
            )
    }

    fn render_warnings(&self, model: &SessionModel, cx: &mut App) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .when(model.warnings.is_empty(), |this| this.child(div()))
            .when(!model.warnings.is_empty(), |this| {
                this.child(section_heading("warnings", cx)).child(
                    div().flex().flex_col().gap_1().children(
                        model.warnings.iter().rev().take(6).map(|warning| {
                            label(warning.clone(), cx).text_color(Theme::warning(cx))
                        }),
                    ),
                )
            })
    }

    /// What OBS needs to be told, since getting it wrong is the difference
    /// between a visible overlay and an invisible one.
    fn render_obs_help(&self, model: &SessionModel, cx: &mut App) -> impl IntoElement {
        // The theme is baked into the URL OBS is given, rather than left to
        // whatever the app's default happens to be on the day.
        //
        // A Browser Source is created once and then just sits there. If the URL
        // were `/overlay`, changing the theme in settings would silently change
        // what is on stream — or, worse, appear to do nothing on a machine
        // where OBS was set up before the setting existed. Putting the chosen
        // look in the URL makes the source self-describing: this is what the
        // streamer picked, and copying the settings elsewhere changes it.
        //
        // `?theme=` already wins over the saved setting on the server, so no
        // other change is needed for the URL to be authoritative.
        let url = format!("{}?theme={}", model.overlay_url, model.overlay_theme);
        let copied = self.copied_url;

        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .py_3()
            .child(section_heading("obs", cx))
            .child(label(
                "Add a Browser Source to your scene with this URL, and size it to your \
                 scene (480x270 is plenty). It has to be at least as wide as the number.",
                cx,
            ))
            .child(
                // A quiet surface rather than the accent colour.
                //
                // The accent is a light cyan and the app's text colour is near
                // white, so putting one behind the other is pale-on-pale: legible
                // in a screenshot, unreadable on a real screen against a stream.
                // The URL is the one thing a streamer types by hand, so it is
                // also the one thing that must not be the hardest text to read.
                div()
                    .id("copy-overlay-url")
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(theme::raised(cx))
                    .text_color(Theme::text(cx))
                    .text_size(LABEL)
                    .cursor(gpui::CursorStyle::PointingHand)
                    .child(url.clone())
                    .on_click({
                        let url = url.clone();
                        let me = self.me.clone();
                        move |_, _window, cx| {
                            write_to_clipboard(&url, cx);
                            let _ = me.update(cx, |this, cx| {
                                this.copied_url = true;
                                cx.notify();
                            });
                        }
                    }),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(if copied {
                        Theme::success(cx)
                    } else {
                        Theme::muted(cx)
                    })
                    .child(if copied {
                        "copied — paste it into the Browser Source's URL"
                    } else {
                        "click the URL to copy it"
                    }),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(Theme::muted(cx))
                    .child(
                        "If it does not appear, check the source's width and height are \
                         not zero, and that it is in the scene you are actually streaming.",
                    ),
            )
    }
}

impl RunForm {
    /// Read the form into a start request.
    ///
    /// Parses rather than trusting the text: `5000abc` is not a goal, and a
    /// silently-ignored field means starting the wrong yapathon.
    fn request(&self, cx: &App) -> crate::session::StartRequest {
        let goal = self.goal.read(cx).value().to_string();
        let limit = self.limit.read(cx).value().to_string();

        crate::session::StartRequest {
            mode: self.mode.goal_mode(parse_goal(&goal)),
            hard_limit: parse_limit(&limit),
        }
    }
}

impl CorrectionForm {
    /// The note typed alongside a correction.
    ///
    /// Falls back to something honest when the field is blank, because a
    /// correction in the log with no reason is the least useful kind of record.
    fn note(&self, cx: &App) -> String {
        self.explicit_note(cx)
            .unwrap_or_else(|| "manual correction".to_string())
    }

    /// What the streamer actually typed, if anything.
    ///
    /// Split from [`note`](Self::note) because a blank note has two different
    /// sensible fallbacks — "manual correction" for a delta, "set to 4200" for
    /// an absolute one — and a caller that wants to say why cannot tell from the
    /// filled-in version whether anyone bothered.
    fn explicit_note(&self, cx: &App) -> Option<String> {
        let note = self.note.read(cx).value().trim().to_string();
        (!note.is_empty()).then_some(note)
    }
}

/// Put `text` on the clipboard.
///
/// GPUI's write is fire-and-forget — it hands the text to the platform and
/// returns — so there is no failure to report and nothing to log. The caller
/// shows its own confirmation, which is the part a streamer actually reads.
pub fn write_to_clipboard(text: &str, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
}

/// A labelled numeric input.
fn numbered_field(label: &str, state: &Entity<InputState>, cx: &mut App) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .w(px(140.0))
        .child(label_text(label, cx))
        .child(
            gpui_kit::component::input::Input::new(state)
                .h(px(28.0))
                .w_full(),
        )
}

/// Small supporting text, used for form field labels.
fn label_text(text: &str, cx: &mut App) -> gpui::Div {
    div()
        .text_size(LABEL)
        .text_color(Theme::muted(cx))
        .child(text.to_string())
}

impl YapView {
    /// What the headline currently reads.
    ///
    /// Goes through the same accessor the render does, because a test that read
    /// the model directly would have passed while the counter was frozen — the
    /// model was always right; the window just never asked it again.
    pub fn headline(&self, cx: &App) -> String {
        self.session.read(cx).model().headline()
    }

    /// The settings panel, if one is open.
    pub fn settings(&self) -> Option<Entity<SettingsView>> {
        self.settings.clone()
    }
}

impl Render for YapView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.session.read(cx).model().clone();

        // A weak handle to this view, for the tab buttons below.
        //
        // `Context::entity` is not available on this snapshot, so the handle is
        // stored on the struct at construction: `cx.new` gets a `Context<Self>`,
        // which can be downgraded, and the result is kept for the buttons'
        // closures to use. Weak rather than strong so a button's closure cannot
        // keep the window alive after it closes.
        let me = self.me.clone();

        // `min_w_0` on the scrolling column.
        //
        // A flex item's default `min-width` is `auto`, so one long URL or a long
        // transcript line sets the width for the whole column and pushes
        // everything to its right off the edge of a narrow window. The whole
        // panel then appears to be missing its controls rather than merely
        // clipped, which is what makes this look like a layout bug elsewhere.
        div()
            .size_full()
            .id("dock-scroll")
            .overflow_y_scroll()
            .min_w_0()
            .bg(Theme::bg(cx))
            .text_color(Theme::text(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .min_w_0()
                    .p_4()
                    .child(self.render_tabs(&me, cx))
                    .when(self.tab == Tab::Run, |this| {
                        this.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_4()
                                .child(self.render_header(&model, cx))
                                .child(self.render_stats(&model, cx))
                                .child(self.render_notices(&model, cx))
                                .child(self.render_start_form(&model, cx))
                                .child(self.render_controls(&model, cx))
                                .child(self.render_correction_form(&model, cx))
                                .child(self.render_events(&model, cx))
                                .child(self.render_transcript(&model, cx))
                                .child(self.render_warnings(&model, cx))
                                .child(self.render_obs_help(&model, cx)),
                        )
                    })
                    .when(self.tab == Tab::Settings, |this| {
                        // The panel is its own entity, so it renders itself and
                        // keeps whatever was typed into it across redraws.
                        if let Some(panel) = self.render_settings(cx) {
                            this.child(panel)
                        } else {
                            this.child(label("settings unavailable", cx))
                        }
                    }),
            )
    }
}

/// The goal the start form asks for.
///
/// Falls back to 5,000 rather than zero: a zero goal is a legal configuration
/// that finishes instantly, and a streamer who cleared the field meant to type a
/// number, not to end the run before it began. Commas are accepted, because
/// `5,000` is how people type it.
pub fn parse_goal(text: &str) -> u32 {
    parse_count(text)
        .filter(|value| *value > 0)
        .unwrap_or(5_000)
}

/// The hard limit the start form asks for.
///
/// Empty means no limit, which is a legitimate choice for a fixed goal. That is
/// different from a field containing junk, which also means no limit rather than
/// a silent fallback — a limit nobody chose is better than one nobody sees.
pub fn parse_limit(text: &str) -> Option<u32> {
    parse_count(text).filter(|value| *value > 0)
}

fn parse_count(text: &str) -> Option<u32> {
    let cleaned: String = text
        .trim()
        .chars()
        .filter(|c| !matches!(c, ',' | '_' | ' '))
        .collect();

    // A trailing `k` is how chat types it, so accept it here too.
    let (digits, multiplier) = match cleaned.chars().last() {
        Some('k') | Some('K') => (&cleaned[..cleaned.len() - 1], 1_000u32),
        _ => (cleaned.as_str(), 1),
    };

    digits.parse::<u32>().ok()?.checked_mul(multiplier)
}

/// Parse a correction, which may be negative.
///
/// Unlike the goal, this must not fall back to a default: a mistyped correction
/// should change nothing rather than silently moving the count.
pub fn parse_target(text: &str) -> Option<u32> {
    parse_count(text)
}

/// The correction presets, for the tests.
pub const PRESETS: &[(i64, &str)] = &CORRECTION_PRESETS;

pub fn parse_adjustment(text: &str) -> i64 {
    let cleaned: String = text
        .trim()
        .chars()
        .filter(|c| !matches!(c, ',' | '_' | ' '))
        .collect();

    cleaned.parse::<i64>().unwrap_or(0)
}

/// What a control asked for.
///
/// Named so a click handler reads as intent rather than as a sequence of
/// `runtime::spawn` and lock calls. `Start` carries the goal because a button
/// that starts "whatever the defaults are" cannot be adjusted without editing a
/// file and restarting.
#[derive(Debug, Clone)]
pub enum Command {
    /// Start a yapathon with everything the run form says.
    Start(crate::session::StartRequest),
    Pause,
    Resume,
    Undo,
    End,
    /// A manual correction, in words. Negative to subtract.
    Adjust {
        words: i64,
        note: String,
    },
}

impl Command {
    /// A short name for the log.
    ///
    /// The whole `Start` request or the correction note would be noise in a line
    /// whose job is to name the thing that failed.
    fn name(&self) -> &'static str {
        match self {
            Command::Start(_) => "start",
            Command::Pause => "pause",
            Command::Resume => "resume",
            Command::Undo => "undo",
            Command::End => "end",
            Command::Adjust { .. } => "adjust",
        }
    }
}

/// Run a command against the shared state.
///
/// Spawned rather than awaited: these take the session's write lock and a click
/// handler runs on the UI thread, which must not block. The result arrives back
/// through the same broadcast the overlay reads, so there is one path to the
/// screen regardless of where the change came from.
fn command(session: &Entity<SessionEntity>, cx: &mut gpui::App, what: Command) {
    let state = session.read(cx).state().clone();

    // Named before the match, because the match consumes `what` and the failure
    // log below still has to say which command it was.
    let name = what.name();

    crate::runtime::spawn(async move {
        let result: anyhow::Result<()> = match what {
            Command::Start(request) => crate::session::start(&state, request).await,
            Command::Pause => crate::session::pause(&state).await,
            Command::Resume => crate::session::resume(&state).await,
            Command::Undo => crate::session::undo(&state).await.map(|removed| {
                if !removed {
                    log::info!("yap-a-thon: nothing to undo");
                }
            }),
            Command::End => crate::session::end(&state).await,
            Command::Adjust { words, ref note } => {
                crate::session::adjust(&state, words, note).await
            }
        };

        if let Err(error) = result {
            // Surfaced in the log rather than swallowed: a control that silently
            // does nothing looks broken in a way that is hard to diagnose.
            log::warn!("yap-a-thon: {name} failed: {error:#}");
        }
    });
}

/// The progress track and fill.
fn progress_bar(fraction: f32, tone: StatusTone, cx: &mut App) -> impl IntoElement {
    let ratio = fraction.clamp(0.0, 1.0);

    div()
        .h(px(6.0))
        .mt_2()
        .w_full()
        .rounded_full()
        .overflow_hidden()
        .bg(Theme::muted(cx))
        .child(
            // A relative width, because the track is whatever the window happens
            // to be and a fixed pixel width would not track it.
            div()
                .h_full()
                .w(relative(ratio))
                .bg(tone.accent(cx))
                .rounded_full(),
        )
}

/// A ratio as a width.
///
/// A percentage of the track, so the fill tracks whatever the window is sized
/// to. A fixed pixel width would not.
fn relative(ratio: f32) -> gpui::DefiniteLength {
    gpui::DefiniteLength::Fraction(ratio.clamp(0.0, 1.0))
}

/// One figure with a label under it.
fn stat(label_text: &str, value: String, cx: &mut App) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .text_size(VALUE)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(value),
        )
        .child(label(label_text, cx))
}

/// Small supporting text.
pub(crate) fn label(text: impl Into<gpui::SharedString>, cx: &mut App) -> gpui::Div {
    div()
        .text_size(LABEL)
        .text_color(Theme::muted(cx))
        .child(text.into())
}

/// A section heading.
pub(crate) fn section_heading(text: &str, cx: &mut App) -> impl IntoElement {
    div()
        .text_size(LABEL)
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(Theme::muted(cx))
        .child(text.to_uppercase())
}

/// Colour, and the mapping from status to tone.
///
/// Separate from the view so the mapping is testable without rendering.
pub mod style {
    use super::super::theme::Theme;
    use gpui::App;

    use crate::model::SessionModel;

    /// The visual states worth telling apart at a glance.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum StatusTone {
        /// Counting normally.
        Running,
        /// Not counting, but the run continues.
        Paused,
        /// Finished: goal reached, ended by hand, or hard limit hit.
        Finished,
        /// Nothing running.
        Idle,
    }

    /// Map a model onto a tone.
    pub fn tone_for(model: &SessionModel) -> StatusTone {
        use yap_core::session::SessionStatus;

        let Some(snapshot) = &model.snapshot else {
            return StatusTone::Idle;
        };

        match snapshot.status {
            SessionStatus::Running => StatusTone::Running,
            SessionStatus::Paused => StatusTone::Paused,
            SessionStatus::Idle => StatusTone::Idle,
            // All three endings share a tone: distinguishing them by hue would
            // suggest a difference the streamer cannot act on.
            SessionStatus::Won | SessionStatus::Capped | SessionStatus::Ended => {
                StatusTone::Finished
            }
        }
    }

    impl StatusTone {
        /// The colour of the headline number.
        pub fn text(self, cx: &App) -> gpui::Hsla {
            match self {
                Self::Running => Theme::text(cx),
                Self::Paused => Theme::warning(cx),
                Self::Finished => Theme::success(cx),
                Self::Idle => Theme::muted(cx),
            }
        }

        /// The colour of the progress fill.
        ///
        /// Deliberately the same three states as `text`, from the same palette,
        /// so the number and its bar can never disagree about what state the run
        /// is in.
        pub fn accent(self, cx: &App) -> gpui::Hsla {
            match self {
                Self::Running => Theme::accent(cx),
                Self::Paused => Theme::warning(cx),
                Self::Finished => Theme::success(cx),
                Self::Idle => Theme::muted(cx),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::style::{StatusTone, tone_for};
    use super::{PRESETS, RunMode, Tab, parse_adjustment, parse_goal, parse_limit, parse_target};
    use crate::model::{ModelMessage, SessionEntity, SessionModel};
    use crate::settings_view::SharedId;
    use yap_core::goal::{Event, GoalMode, HardLimit};
    use yap_core::session::{Session, SessionConfig};

    /// Stand the whole window up in a headless one and hand back its entity.
    ///
    /// `YapView` needs a session entity, which needs a shared state and a
    /// message channel. `AppState::new` binds nothing and starts no tasks, so
    /// this is cheap — and it means the real view can be rendered and clicked in
    /// a test rather than only looked at on a desktop.
    fn build_root(
        receiver: async_channel::Receiver<ModelMessage>,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Root>,
    ) -> Root {
        let session = SessionEntity::new(
            std::sync::Arc::new(yap_server::AppState::new()),
            receiver,
            "http://127.0.0.1:8973/overlay".to_string(),
            cx,
        );

        Root {
            view: super::YapView::new(
                session,
                yap_core::settings::Settings::default(),
                std::path::PathBuf::from("/tmp/yap-a-thon-test/settings.toml"),
                window,
                cx,
            ),
        }
    }

    /// Stand the real window up in a headless one.
    ///
    /// `YapView` needs a session entity, which needs a shared state and a message
    /// channel. `AppState::new` binds nothing and starts no tasks, so this is
    /// cheap — and it means the real view can be rendered and clicked in a test
    /// rather than only looked at on a desktop.
    ///
    /// Returns the root and shadows the context the way `add_window_view` does,
    /// so each test reads as setup, click, assert.
    macro_rules! open_window {
        ($cx:expr) => {{
            let (_sender, receiver) = async_channel::unbounded::<ModelMessage>();
            $cx.update(gpui_kit::init);
            let (root, cx) = $cx.add_window_view(|window, cx| build_root(receiver, window, cx));
            (root, cx)
        }};
    }

    /// Holds a session entity alive for a test that is about the entity rather
    /// than the window on top of it.
    ///
    /// Needs to be a window root because `add_window_view` hands the context it
    /// constructs the entity with; a bare `Entity` has no context to construct
    /// one in.
    struct Holder {
        entity: gpui::Entity<SessionEntity>,
    }

    impl gpui::Render for Holder {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::div()
        }
    }

    /// The window's root.
    ///
    /// A thin wrapper rather than making `YapView` itself the root, because that
    /// is what `open_window` does in production too — testing a different shape
    /// than the app uses would be testing something else.
    struct Root {
        view: gpui::Entity<super::YapView>,
    }

    impl gpui::Render for Root {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            self.view.clone()
        }
    }

    #[gpui_kit::test]
    fn the_settings_panel_renders_once_its_tab_is_reachable(cx: &mut gpui_kit::TestAppContext) {
        // Two separate failures, and only this test catches both at once.
        //
        // The tab used to be inert, so the panel was never even asked to draw;
        // and a panel can be perfectly clickable and still panic, or draw
        // nothing useful, when it finally is asked. Rendering it headlessly is
        // the only way to know that without sitting at someone's machine and
        // clicking through their UI.
        use gpui_kit::test::TestWindowExt as _;

        let (root, cx) = open_window!(cx);
        cx.update(|window, cx| window.draw(cx).clear(cx));

        // Reads go through `update` because a `VisualTestContext` is not an
        // `&App`; it only becomes one inside the closure.
        let tab = cx.update(|_window, cx| root.read(cx).view.read(cx).tab);
        assert_eq!(tab, Tab::Run, "starts on the run tab");

        cx.update(|window, cx| window.click("segment-Settings", cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let tab = cx.update(|_window, cx| root.read(cx).view.read(cx).tab);
        assert_eq!(tab, Tab::Settings);

        // The panel is the real one, wired to the file this window was opened
        // with — which is what makes "Save went somewhere else" a bug this can
        // see rather than one only a streamer finds.
        let path = cx.update(|_window, cx| {
            root.read(cx)
                .view
                .read(cx)
                .settings()
                .expect("a settings panel")
                .read(cx)
                .path()
                .to_path_buf()
        });

        assert_eq!(
            path,
            std::path::PathBuf::from("/tmp/yap-a-thon-test/settings.toml")
        );
    }

    #[gpui_kit::test]
    fn every_settings_section_can_be_switched_to(cx: &mut gpui_kit::TestAppContext) {
        // Every section button had the same dead-click bug as the tab, so the
        // panel could only ever show whichever section it happened to open on.
        use gpui_kit::test::TestWindowExt as _;

        let (root, cx) = open_window!(cx);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.click("segment-Settings", cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let sections = crate::settings::SETTINGS_SECTIONS.len();
        assert!(sections > 1, "there is more than one section to switch to");

        for index in 1..sections {
            let title = crate::settings::SETTINGS_SECTIONS[index].title;
            cx.update(|window, cx| {
                window.click(SharedId::section(index), cx);
                window.draw(cx).clear(cx);
            });

            // Reading the panel after the redraw, so this checks what is on
            // screen now rather than what the click set at the time.
            let (shown, browsing) = cx.update(|_window, cx| {
                let panel = root
                    .read(cx)
                    .view
                    .read(cx)
                    .settings()
                    .expect("a settings panel");
                let panel = panel.read(cx);
                (panel.section(), panel.is_browsing())
            });

            assert_eq!(shown, index, "the {title} section must be selectable");
            assert!(!browsing, "no chooser opens on its own");
        }
    }

    #[gpui_kit::test]
    fn a_credential_input_is_masked_and_watched_by_nothing(cx: &mut gpui_kit::TestAppContext) {
        // The mechanism that keeps a token out of `settings.toml`.
        //
        // Save is the panel's only commit and Save writes a file, so the whole
        // safety of the credential store rests on one structural fact: the
        // masked credential inputs have no change subscription, so typing in one
        // never reaches `SettingsModel::set`, never marks the panel dirty, and
        // never becomes part of what Save writes.
        //
        // Asserted on the real panel rather than on `SettingsModel`, because a
        // unit test of the model passes whether or not anything ever calls it —
        // the same shape as the four dead controls this project has already had,
        // where a model quietly held the right value and the user saw nothing.
        use gpui_kit::test::TestWindowExt as _;

        let (root, cx) = open_window!(cx);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.click("segment-Settings", cx));
        cx.update(|window, cx| window.draw(cx).clear(cx));

        // Find the section holding a credential, so this does not depend on
        // which section that happens to be today.
        let (section, key) = crate::settings::SETTINGS_SECTIONS
            .iter()
            .enumerate()
            .find_map(|(index, section)| {
                section
                    .fields
                    .iter()
                    .find(|field| field.kind == crate::settings::FieldKind::Secret)
                    .map(|field| (index, field.key))
            })
            .expect("some field is a credential");

        cx.update(|window, cx| {
            window.click(SharedId::section(section), cx);
            window.draw(cx).clear(cx);
        });

        let panel = cx.update(|_window, cx| {
            root.read(cx)
                .view
                .read(cx)
                .settings()
                .expect("a settings panel")
                .clone()
        });

        let input = cx
            .update(|_window, cx| panel.read(cx).secret_input(key))
            .unwrap_or_else(|| panic!("{key} has no masked input"));

        // Masked, and empty — a credential is never pre-filled, because a masked
        // box holding the real value defeats the masking.
        assert!(
            cx.update(|_window, cx| panel.read(cx).secret_input_is_masked(key)),
            "a credential input must start masked",
        );
        assert_eq!(
            cx.update(|_window, cx| input.read(cx).value()),
            "",
            "a credential input must start empty, not pre-filled with the stored value",
        );

        // The load-bearing assertion: nothing watches it.
        let watched = cx.update(|_window, cx| panel.read(cx).subscribed_keys());

        assert!(
            !watched.iter().any(|k| k == key),
            "{key} is watched for changes, so typing it arms Save — and Save writes \
             settings.toml, which is safe to share and safe to screenshot. Watched: {watched:?}",
        );

        // And the editable model it would have gone into is empty for it.
        assert_eq!(
            cx.update(|_window, cx| panel.read(cx).model_value(key)),
            "",
            "a credential must not be in the model Save serialises",
        );

        // Sanity on the other half: ordinary fields *are* watched, or this test
        // would pass for the wrong reason — a panel that subscribes to nothing at
        // all.
        assert!(
            watched.iter().any(|k| k == "twitch_channel"),
            "ordinary settings must be watched, or Save could never be armed: {watched:?}",
        );
    }

    #[gpui_kit::test]
    fn a_counted_session_tells_the_window_to_repaint(cx: &mut gpui_kit::TestAppContext) {
        // The bug this test exists for.
        //
        // The session entity folded new snapshots into its model but never
        // notified, so the window was subscribed to an entity that changed
        // silently: the count advanced in memory through an entire yapathon
        // while the number on screen stayed at zero. Nothing errored and nothing
        // logged, because from the model's point of view everything worked.
        //
        // What is asserted is that a repaint is *requested*, not that the model
        // holds the right number. Reading the model would pass either way — it
        // was always correct. The redraw only happens if something asks for one,
        // and this asks the same way the view does: by observing.
        use std::cell::Cell;
        use std::rc::Rc;

        let (_sender, receiver) = async_channel::unbounded::<ModelMessage>();
        let state = std::sync::Arc::new(yap_server::AppState::new());

        cx.update(gpui_kit::init);
        let (holder, cx) = cx.add_window_view(move |_window, cx| Holder {
            entity: SessionEntity::new(
                state,
                receiver,
                "http://127.0.0.1:8973/overlay".to_string(),
                cx,
            ),
        });
        let session = cx.update(|_window, cx| holder.read(cx).entity.clone());

        // Exactly what `YapView::new` does: subscribe, and repaint when it fires.
        let repaints = Rc::new(Cell::new(0usize));
        let _observing = cx.update(|_window, cx| {
            let repaints = Rc::clone(&repaints);
            cx.observe(&session, move |_entity, _cx| {
                repaints.set(repaints.get() + 1);
            })
        });

        let mut session = Session::new(SessionConfig::new(GoalMode::Fixed { words: 100 }, None));
        session.start();
        session.record("one two three", 0, 1000);

        _sender
            .send_blocking(ModelMessage::State(session.snapshot()))
            .expect("the entity is listening");
        cx.run_until_parked();

        assert_eq!(
            repaints.get(),
            1,
            "a counted session must ask the window to repaint, or the counter \
             never moves on screen",
        );
    }

    #[test]
    fn a_counted_session_is_readable_by_the_window() {
        // The other half: the number the header shows is the session's, not a
        // number the view keeps to itself.
        let mut session = Session::new(SessionConfig::new(GoalMode::Fixed { words: 100 }, None));
        session.start();
        session.record("one two three", 0, 1000);

        let mut model = SessionModel::default();
        assert!(model.apply(ModelMessage::State(session.snapshot())));
        assert_eq!(model.headline(), "3");
    }

    #[test]
    fn an_unchanged_snapshot_is_not_a_reason_to_repaint() {
        // The other half of the fix. A running session publishes several
        // snapshots a second and most are identical; repainting on all of them
        // is how a counter turns into a stutter.
        let mut model = SessionModel::default();
        let mut session = Session::new(SessionConfig::new(GoalMode::Fixed { words: 100 }, None));
        session.start();
        let snapshot = session.snapshot();

        assert!(model.apply(ModelMessage::State(snapshot.clone())));
        assert!(
            !model.apply(ModelMessage::State(snapshot)),
            "the same snapshot twice is one repaint, not two",
        );

        // A real change still goes through.
        session.record("one two three", 0, 1000);
        assert!(model.apply(ModelMessage::State(session.snapshot())));
    }

    #[test]
    fn the_presets_are_usable_round_numbers() {
        // Every preset must be a non-zero delta: a preset that applies nothing
        // is a button that looks like it did something and did not.
        for (delta, label) in PRESETS {
            assert_ne!(*delta, 0, "preset {label:?} would do nothing");
            assert!(
                delta.abs() % 5 == 0,
                "preset {label:?} is not an amount anyone would be gifted",
            );
            assert!(!label.is_empty(), "a preset needs a readable label");
        }
    }

    #[test]
    fn there_is_a_way_to_subtract_as_well_as_add() {
        // Only being able to add is how a count gets inflated: every correction
        // available goes up, so a mistake can only ever be made worse.
        assert!(
            PRESETS.iter().any(|(delta, _)| *delta < 0),
            "there must be a subtraction preset",
        );
        assert!(
            PRESETS.iter().any(|(delta, _)| *delta > 0),
            "there must be an addition preset",
        );
    }

    #[test]
    fn a_target_is_parsed_only_when_it_is_a_number() {
        assert_eq!(parse_target("4200"), Some(4_200));
        assert_eq!(parse_target(" 1,200 "), Some(1_200));
        assert_eq!(parse_target(""), None);
        assert_eq!(parse_target("-5"), None, "a count is not negative");
        assert_eq!(parse_target("lots"), None);
    }

    #[test]
    fn a_chosen_mode_is_the_mode_that_would_run() {
        // The label on the button and the GoalMode that actually starts are the
        // same decision written down twice, so they are checked against each
        // other rather than trusted.
        assert_eq!(
            RunMode::Extend.goal_mode(3_000),
            GoalMode::Extend { base_words: 3_000 },
        );
        assert_eq!(
            RunMode::Fixed.goal_mode(3_000),
            GoalMode::Fixed { words: 3_000 }
        );
        assert_eq!(
            RunMode::Time.goal_mode(90),
            GoalMode::Time {
                base: std::time::Duration::from_secs(90),
            },
        );
    }

    #[test]
    fn a_configured_mode_survives_the_trip_to_the_window() {
        // The window reads its default mode out of the configured `GoalMode` by
        // short name, so this is the real pairing: a name the core emits and a
        // name the UI understands. Renaming one side without the other silently
        // opens every yapathon in the wrong mode.
        let configured = [
            GoalMode::Extend { base_words: 1 },
            GoalMode::Fixed { words: 1 },
            GoalMode::Time {
                base: std::time::Duration::from_secs(1),
            },
        ];

        let mapped: Vec<Option<RunMode>> = configured
            .iter()
            .map(|mode| RunMode::from_short_name(mode.short_name()))
            .collect();

        assert_eq!(
            mapped,
            RunMode::ALL.into_iter().map(Some).collect::<Vec<_>>(),
            "every configured mode must map to a distinct one in the window",
        );
    }

    #[test]
    fn an_unknown_mode_name_is_not_a_mode() {
        // Better than defaulting: an unrecognised configured mode should fall
        // back visibly in `new`, not be silently read as some other mode here.
        assert_eq!(RunMode::from_short_name("words"), None);
        assert_eq!(RunMode::from_short_name(""), None);
    }

    #[test]
    fn a_correction_accepts_negative_numbers() {
        assert_eq!(parse_adjustment("-12"), -12);
        assert_eq!(parse_adjustment(" 7 "), 7);
    }

    #[test]
    fn a_junk_correction_changes_nothing() {
        // Unlike the goal, which falls back: a mistyped correction must not
        // silently move the count in a direction nobody typed.
        assert_eq!(parse_adjustment("abc"), 0);
        assert_eq!(parse_adjustment("3.5"), 0);
        assert_eq!(parse_adjustment(""), 0);
    }

    #[test]
    fn a_cleared_goal_does_not_start_a_zero_length_yapathon() {
        assert_eq!(parse_goal(""), 5_000);
        assert_eq!(parse_goal("0"), 5_000);
        assert_eq!(parse_goal("5,000"), 5_000);
        assert_eq!(parse_goal("2k"), 2_000);
    }

    #[test]
    fn an_empty_limit_means_no_limit() {
        assert_eq!(parse_limit(""), None);
        assert_eq!(parse_limit("0"), None);
        assert_eq!(parse_limit("12000"), Some(12_000));
    }

    fn model_of(session: &Session) -> SessionModel {
        SessionModel {
            snapshot: Some(session.snapshot()),
            ..SessionModel::default()
        }
    }

    fn session(mode: GoalMode, limit: Option<HardLimit>) -> Session {
        let mut session = Session::new(SessionConfig::new(mode, limit));
        session.start();
        session
    }

    #[test]
    fn a_running_session_reads_as_running() {
        let session = session(GoalMode::Fixed { words: 100 }, None);
        assert_eq!(tone_for(&model_of(&session)), StatusTone::Running);
    }

    #[test]
    fn a_paused_session_is_distinguishable() {
        // A distinct tone while paused: a streamer looking away should still
        // know the count is not moving.
        let mut session = session(GoalMode::Fixed { words: 100 }, None);
        session.pause();

        assert_eq!(tone_for(&model_of(&session)), StatusTone::Paused);
    }

    #[test]
    fn every_ending_is_the_same_tone() {
        let mut won = session(GoalMode::Fixed { words: 5 }, None);
        won.record("one two three four five", 0, 1000);
        assert_eq!(won.status(), yap_core::session::SessionStatus::Won);
        assert_eq!(tone_for(&model_of(&won)), StatusTone::Finished);

        let mut ended = session(GoalMode::Fixed { words: 100 }, None);
        ended.end();
        assert_eq!(tone_for(&model_of(&ended)), StatusTone::Finished);
    }

    #[test]
    fn no_session_reads_as_idle() {
        assert_eq!(tone_for(&SessionModel::default()), StatusTone::Idle);
    }

    #[test]
    fn a_capped_run_is_finished_and_flagged() {
        let mut session = session(
            GoalMode::Extend { base_words: 100 },
            Some(HardLimit::words(100)),
        );
        session.apply_event(&Event::granting("sub", 100));

        let model = model_of(&session);
        assert_eq!(tone_for(&model), StatusTone::Finished);
        assert!(model.is_capped());
    }

    #[test]
    fn the_fraction_never_exceeds_one() {
        // Over-counting past the goal must not produce a bar wider than its
        // track. The clamp lives in `GoalProgress::fraction`, so the value the
        // view renders is already in range — the view's own clamp is a second
        // guard, not the only one.
        let mut session = session(GoalMode::Fixed { words: 10 }, None);
        session.record("one two three four five six seven eight nine ten", 0, 1000);

        let fraction = model_of(&session).fraction();
        assert!(fraction > 0.9, "got {fraction}");
        assert!(
            fraction <= 1.0,
            "a finished run reads as full, not overfull"
        );
    }

    #[test]
    fn a_zero_goal_reads_as_full_rather_than_dividing_by_zero() {
        let session = session(GoalMode::Fixed { words: 0 }, None);
        assert_eq!(model_of(&session).fraction(), 1.0);
    }
}
