//! The one button used for every exclusive choice in the app.
//!
//! There are four of these rows — the window's tabs, the run-mode picker, the
//! settings sections, and every choice field inside a section — and they all
//! share one bug-prone detail, so they share one function.

use std::rc::Rc;

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

    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{IntoElement, Render, SharedString};

    use super::segmented;

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
}
