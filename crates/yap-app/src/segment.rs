//! The one button used for every exclusive choice in the app.
//!
//! There are four of these rows — the window's tabs, the run-mode picker, the
//! settings sections, and every choice field inside a section — and they all
//! share one bug-prone detail, so they share one function.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::Selectable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{App, IntoElement, ParentElement as _, SharedString, Styled as _, div};

/// One option in an exclusive row.
///
/// `selected` marks the current one. The styling for that comes from the
/// *variant*, not from `disabled`.
///
/// This distinction is the whole reason the function exists. `Button::disabled`
/// removes the click handler along with the styling, so the obvious way to say
/// "these are not the current one" produces a row where only the already-current
/// option responds to a click. Every other option is a dead control that looks
/// like it might work: same shape, same label, no error, no log line, nothing
/// to notice.
///
/// All four rows were built that way at some point. In practice it meant the
/// Settings tab could not be opened, no settings section could be switched to,
/// no choice field could be changed, and no other model could be selected — the
/// entire settings panel was decorative. Nothing crashed and nothing logged,
/// which is why it survived being looked at several times.
///
/// Picking the variant per segment instead gives the same "you are here"
/// reading without touching the click handler, and `selected` still carries it
/// to assistive technology where a colour difference is worth nothing.
pub fn segment_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    on_click: impl Fn(&mut gpui::Window, &mut App) + 'static,
) -> Button {
    Button::new(id)
        .label(label)
        .when(selected, |button| button.primary())
        .selected(selected)
        .on_click(move |_, window, cx| on_click(window, cx))
}

/// One option in an exclusive row, with a symbol in front of its label.
///
/// The same button as [`segment_button`], so the selected variant and the click
/// handler are chosen the same way and cannot drift apart.
///
/// Separate rather than a parameter on `segment_button` because a row of
/// mixed icon-and-text segments is worse than either: a symbol means the
/// section is recognisable, and a row where some options have one and some do
/// not makes you read the text to work out which convention you are in.
pub fn icon_segment_button(
    id: impl Into<gpui::ElementId>,
    icon: IconName,
    label: impl Into<SharedString>,
    selected: bool,
    on_click: impl Fn(&mut gpui::Window, &mut App) + 'static,
) -> Button {
    segment_button(id, label, selected, on_click).icon(icon)
}

/// A row of mutually exclusive options, one of which is current.
///
/// `on_pick` receives the option and the app, so a caller can update whatever
/// state it likes without this function knowing what that state is. That also
/// makes the row testable on its own: the callback is the only thing a test
/// needs to observe.
pub fn segmented<T, I, S, F>(options: I, current: T, on_pick: F) -> impl IntoElement
where
    T: Copy + PartialEq + 'static,
    I: IntoIterator<Item = (T, S)>,
    S: Into<SharedString>,
    F: Fn(T, &mut App) + 'static,
{
    let on_pick = Rc::new(on_pick);

    div()
        .flex()
        .flex_wrap()
        .gap_2()
        .children(options.into_iter().map(|(option, text)| {
            let text = text.into();
            let id = format!("segment-{text}");

            segment_button(id, text, option == current, {
                // Shared, not copied: each segment needs its own `'static`
                // closure and the callback only ever runs on the UI thread,
                // so a reference count is enough.
                let on_pick = Rc::clone(&on_pick);
                move |_window, cx| on_pick(option, cx)
            })
            .into_any_element()
        }))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use gpui_kit::assets::IconName;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        App, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
        Styled as _, Window, div, px,
    };

    use super::{icon_segment_button, segment_button, segmented};

    /// One option, for a row of tests to pick between.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Choice {
        First,
        Second,
        Third,
    }

    /// The element id `segmented` gives this choice.
    ///
    /// Spelled out here rather than copied into each test, so a change to the
    /// id scheme shows up as tests that cannot find the button instead of tests
    /// that quietly stop clicking anything.
    fn element_id(choice: Choice) -> String {
        format!("segment-{}", choice.label())
    }

    impl Choice {
        const ALL: [Choice; 3] = [Choice::First, Choice::Second, Choice::Third];

        fn label(self) -> &'static str {
            match self {
                Self::First => "first",
                Self::Second => "second",
                Self::Third => "third",
            }
        }
    }

    /// A window showing one row that records what got picked.
    struct Harness<T> {
        picked: Rc<Cell<Option<T>>>,
        options: Vec<(T, gpui::SharedString)>,
        current: T,
    }

    impl<T: Copy + PartialEq + 'static> Render for Harness<T> {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let picked = Rc::clone(&self.picked);
            let options = self.options.clone();

            segmented(options, self.current, move |option, _cx| {
                picked.set(Some(option));
            })
        }
    }

    /// Render a row, click `target`, and report what was picked.
    ///
    /// Clicks by element id rather than by coordinates, so the check does not
    /// depend on font metrics or padding, and renders into a headless window,
    /// so it needs no display and no input from anyone. That matters: whether a
    /// control is reachable is something to assert, not something to go and poke
    /// at on someone's desktop.
    fn click_one(
        cx: &mut gpui_kit::TestAppContext,
        current: Choice,
        target: Choice,
    ) -> Option<Choice> {
        let picked: Rc<Cell<Option<Choice>>> = Rc::new(Cell::new(None));

        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view({
            let picked = Rc::clone(&picked);
            move |_, _| Harness {
                picked,
                options: Choice::ALL
                    .into_iter()
                    .map(|choice| (choice, choice.label().into()))
                    .collect(),
                current,
            }
        });

        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| window.click(SharedString::from(element_id(target)), cx));

        picked.get()
    }

    #[gpui_kit::test]
    fn an_unselected_option_still_responds_to_a_click(cx: &mut gpui_kit::TestAppContext) {
        assert_eq!(
            click_one(cx, Choice::First, Choice::Second),
            Some(Choice::Second)
        );
    }

    #[gpui_kit::test]
    fn the_last_option_is_reachable_too(cx: &mut gpui_kit::TestAppContext) {
        // The last one is the easiest to break by accident: it is furthest from
        // the current option and sits where an overflowing row would be.
        assert_eq!(
            click_one(cx, Choice::First, Choice::Third),
            Some(Choice::Third)
        );
    }

    #[gpui_kit::test]
    fn the_current_option_still_responds(cx: &mut gpui_kit::TestAppContext) {
        // Selecting what is already selected is harmless and must not be the
        // only thing that works.
        assert_eq!(
            click_one(cx, Choice::Second, Choice::Second),
            Some(Choice::Second)
        );
    }

    #[gpui_kit::test]
    fn every_option_is_reachable_from_every_starting_point(cx: &mut gpui_kit::TestAppContext) {
        // The full matrix, because "unselected options work" is only convincing
        // if it holds for every position rather than one lucky click.
        for current in Choice::ALL {
            for target in Choice::ALL {
                assert_eq!(
                    click_one(cx, current, target),
                    Some(target),
                    "{target:?} must be reachable while {current:?} is current",
                );
            }
        }
    }

    /// One button, built with or without an icon.
    struct OneButton {
        icon: Option<IconName>,
        clicked: Rc<Cell<bool>>,
    }

    impl Render for OneButton {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let clicked = Rc::clone(&self.clicked);

            // Measured through `debug_selector` on a fixed-width wrapper rather
            // than through the id the button was built with.
            //
            // That id is not an observable boundary: looking it up finds the
            // maximized test window's root, which is the same width for both
            // cases, so the comparison would pass on two equal numbers while
            // saying nothing about icons. This wrapper is a known 400px, so its
            // bounds vary only with what the button put inside it.
            div()
                .w(px(400.0))
                .flex()
                .flex_col()
                .items_start()
                .child(
                    div()
                        .debug_selector(|| "probe".into())
                        .child(match self.icon {
                            Some(icon) => {
                                icon_segment_button("button", icon, "Goal", false, move |_, _| {
                                    clicked.set(true)
                                })
                                .into_any_element()
                            }
                            None => segment_button("button", "Goal", false, move |_, _| {
                                clicked.set(true)
                            })
                            .into_any_element(),
                        }),
                )
                .into_any_element()
        }
    }

    /// Draw one button, then report its rendered width and whether it was clicked.
    ///
    /// Measured rather than assumed. `Icon::path` names a file; whether those
    /// bytes ever reach layout is a separate question, and an icon that fails to
    /// load takes up no space at all — so the width is the observation. A button
    /// that compiles, resolves a name, and draws nothing is indistinguishable
    /// from one without the icon by any other means.
    fn probe(icon: Option<IconName>, cx: &mut gpui_kit::TestAppContext) -> (f32, Rc<Cell<bool>>) {
        // Cloned before the move: one handle goes into the view, one stays
        // here to be read after the click.
        let clicked: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let in_view = Rc::clone(&clicked);

        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(move |_window, _cx| OneButton {
            icon,
            clicked: in_view,
        });

        cx.update(|window, cx| window.draw(cx).clear(cx));

        let width = cx
            .debug_bounds("probe")
            .expect("the probe div rendered")
            .size
            .width
            .as_f32();

        cx.update(|window, cx| window.click(SharedString::from("button"), cx));

        (width, clicked)
    }

    #[gpui_kit::test]
    fn an_icon_actually_takes_up_room_on_the_button(cx: &mut gpui_kit::TestAppContext) {
        // The failure this catches: the icon is named, the name compiles, the
        // bytes are missing, and the button renders exactly as it would without
        // one — eleven settings sections looking identical to before the icons
        // were added, with nothing anywhere saying so.
        //
        // Width is the tell. A glyph occupies space whether or not anyone
        // checked that it loaded, and an empty one occupies none.
        let (plain, plain_clicked) = probe(None, cx);
        let (with_icon, icon_clicked) = probe(Some(IconName::Target), cx);

        assert!(
            with_icon - plain > 8.0,
            "the icon adds only {:.1}px to a {plain:.1}px button, which is not a \
             glyph — it is not being drawn",
            with_icon - plain,
        );

        // The click half is here rather than in a test of its own because it is
        // the same risk: the obvious way to make room for an icon is to stop the
        // button working, and `Button::disabled` does exactly that while looking
        // like the right thing to press.
        assert!(plain_clicked.get(), "the plain button stopped responding");
        assert!(
            icon_clicked.get(),
            "an icon on the button must not take its click handler with it"
        );
    }
}
