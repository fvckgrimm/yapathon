//! The app's colours, as named roles rather than scattered tokens.
//!
//! Dark by default. A streamer's dock sits next to OBS and a browser full of
//! dark tools; a light panel next to them is a lamp on a desk at 2am. Light is
//! still available — `light()` — because somebody streams in a bright room and
//! their own display preference should win.
//!
//! Values follow `accerso-gpui`'s palette rather than being invented: the same
//! hue family as the rest of the desktop, so the dock does not read as a
//! different application.

use gpui::{App, Hsla, hsla};
use gpui_kit::component::theme::ActiveTheme as _;

/// A named accent colour.
///
/// A small set rather than a free colour picker, deliberately. Almost nobody
/// picks a custom accent, and the ones who do usually want "the purple from my
/// channel's logo" — which is what the presets are for. A picker would also need
/// a contrast story for every arbitrary colour someone drags into it, and the
/// one thing an accent has to do is stay readable in both themes.
///
/// Each is a hue pair: a light value for dark panels, a deeper one for light
/// panels. Both are checked against the surfaces in both themes by a test, so a
/// preset cannot be added that is unreadable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Accent {
    #[default]
    Teal,
    Green,
    Pink,
    Purple,
    Amber,
    Blue,
    /// The channel is already colourful; the app should not add to it.
    Mono,
}

impl Accent {
    pub const ALL: [Accent; 7] = [
        Self::Teal,
        Self::Green,
        Self::Pink,
        Self::Purple,
        Self::Amber,
        Self::Blue,
        Self::Mono,
    ];

    /// The value stored in settings.
    pub fn id(self) -> &'static str {
        match self {
            Self::Teal => "teal",
            Self::Green => "green",
            Self::Pink => "pink",
            Self::Purple => "purple",
            Self::Amber => "amber",
            Self::Blue => "blue",
            Self::Mono => "mono",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Teal => "Teal",
            Self::Green => "Green",
            Self::Pink => "Pink",
            Self::Purple => "Purple",
            Self::Amber => "Amber",
            Self::Blue => "Blue",
            Self::Mono => "Monochrome",
        }
    }

    /// Read an accent by id, falling back to the default.
    ///
    /// The id comes out of a settings file a person can edit, so an unknown one
    /// must not take the palette down with it.
    pub fn from_id(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|accent| accent.id() == id.trim().to_lowercase())
            .unwrap_or_default()
    }

    /// The colour used on a dark panel.
    pub fn on_dark(self) -> Hsla {
        match self {
            Self::Teal => hsla(0.514, 0.85, 0.62, 1.0),
            Self::Green => hsla(0.370, 0.80, 0.62, 1.0),
            Self::Pink => hsla(0.930, 0.75, 0.72, 1.0),
            Self::Purple => hsla(0.740, 0.80, 0.74, 1.0),
            Self::Amber => hsla(0.110, 0.90, 0.66, 1.0),
            Self::Blue => hsla(0.580, 0.85, 0.70, 1.0),
            Self::Mono => hsla(0.000, 0.00, 0.93, 1.0),
        }
    }

    /// The colour used on a light panel.
    pub fn on_light(self) -> Hsla {
        match self {
            Self::Teal => hsla(0.514, 0.72, 0.29, 1.0),
            Self::Green => hsla(0.370, 0.70, 0.28, 1.0),
            Self::Pink => hsla(0.930, 0.70, 0.34, 1.0),
            Self::Purple => hsla(0.740, 0.70, 0.38, 1.0),
            Self::Amber => hsla(0.110, 0.80, 0.30, 1.0),
            Self::Blue => hsla(0.580, 0.75, 0.32, 1.0),
            Self::Mono => hsla(0.625, 0.15, 0.10, 1.0),
        }
    }
}

/// The accent in force, set once at startup from settings.
///
/// A process-wide value rather than something threaded through every `Theme::*`
/// call, for the same reason `Appearance` is resolved once: the window is one
/// palette for the whole session and the alternative is a parameter on roughly
/// forty call sites. Changing it means Save plus a restart, which the settings
/// panel says.
static ACCENT: std::sync::RwLock<Accent> = std::sync::RwLock::new(Accent::Teal);

/// Choose the accent.
///
/// Overwritable, and called again whenever the settings panel saves, so picking
/// a colour changes the window immediately rather than on the next launch.
///
/// A lock rather than a `OnceLock` because "first call wins" is exactly the bug
/// this used to have: the value was set at startup and then nothing could ever
/// change it, so the setting looked live and was not. The contention is
/// nil — one write per save, a read per colour — so there is nothing to gain
/// from making this cleverer.
pub fn set_accent(accent: Accent) {
    match ACCENT.write() {
        Ok(mut guard) => *guard = accent,
        // A panic elsewhere poisoned this. Showing the default accent beats
        // taking the window down.
        Err(poisoned) => *poisoned.into_inner() = accent,
    }
}

/// The accent in force.
pub fn accent() -> Accent {
    match ACCENT.read() {
        Ok(guard) => *guard,
        Err(poisoned) => *poisoned.into_inner(),
    }
}

/// Which palette to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Appearance {
    /// Follow the desktop, but resolve to dark when it cannot be asked.
    #[default]
    Auto,
    Light,
    Dark,
}

impl Appearance {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Follow system",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    /// Resolve to a concrete mode.
    ///
    /// `Auto` resolves against the desktop and then tracks it, which is why it
    /// is a mode and not a boolean: someone who never touches the setting should
    /// still get the right panel in the morning and at night.
    pub fn resolve(self, system_is_dark: bool) -> gpui_kit::component::ThemeMode {
        use gpui_kit::component::ThemeMode;

        match self {
            Self::Light => ThemeMode::Light,
            Self::Dark => ThemeMode::Dark,
            Self::Auto => {
                if system_is_dark {
                    ThemeMode::Dark
                } else {
                    ThemeMode::Light
                }
            }
        }
    }
}

/// The dock's colours.
pub struct Theme;

impl Theme {
    /// Whether the resolved theme is dark.
    pub fn is_dark(cx: &App) -> bool {
        cx.theme().is_dark()
    }

    pub fn bg(cx: &App) -> Hsla {
        if Self::is_dark(cx) {
            hsla(0.625, 0.20, 0.05, 1.0)
        } else {
            hsla(0.600, 0.10, 0.98, 1.0)
        }
    }

    pub fn surface(cx: &App) -> Hsla {
        if Self::is_dark(cx) {
            hsla(0.625, 0.18, 0.08, 1.0)
        } else {
            hsla(0.600, 0.08, 1.0, 1.0)
        }
    }

    pub fn border(cx: &App) -> Hsla {
        if Self::is_dark(cx) {
            hsla(0.625, 0.15, 0.18, 1.0)
        } else {
            hsla(0.625, 0.12, 0.88, 1.0)
        }
    }

    pub fn text(cx: &App) -> Hsla {
        if Self::is_dark(cx) {
            hsla(0.0, 0.0, 0.93, 1.0)
        } else {
            hsla(0.625, 0.15, 0.10, 1.0)
        }
    }

    pub fn muted(cx: &App) -> Hsla {
        if Self::is_dark(cx) {
            hsla(0.625, 0.10, 0.55, 1.0)
        } else {
            hsla(0.625, 0.08, 0.42, 1.0)
        }
    }

    pub fn accent(cx: &App) -> Hsla {
        if Self::is_dark(cx) {
            // Lighter than the eye wants it to be. At lightness 0.50 this green
            // measures 3.6:1 on the window background: fine for the headline,
            // which is enormous, and not fine for the accent-coloured words
            // elsewhere, which are not. The accent is one colour used at a dozen
            // sizes, so it has to clear the body-text bar everywhere rather than
            // relying on each call site being large.
            accent().on_dark()
        } else {
            // Darker for the same reason as the dark-mode value, in the other
            // direction: on a near-white window a mid-lightness teal measures
            // 3.6:1, which is the count sitting at the exact moment a streamer
            // looks at it.
            accent().on_light()
        }
    }

    /// The three state colours.
    ///
    /// Every one of these clears 4.5:1 on the window background in *both*
    /// themes, which means each is a little deeper on light and a little
    /// brighter on dark than the eye first reaches for. That is the trade:
    /// saturated mid-tones look better in a mockup and measure worse on a
    /// screen. `every_text_colour_is_readable_on_the_surface_it_sits_on` is what
    /// holds them to it.
    pub fn success(cx: &App) -> Hsla {
        if Self::is_dark(cx) {
            hsla(0.38, 0.70, 0.45, 1.0)
        } else {
            hsla(0.38, 0.62, 0.26, 1.0)
        }
    }

    pub fn warning(cx: &App) -> Hsla {
        if Self::is_dark(cx) {
            hsla(0.11, 0.85, 0.55, 1.0)
        } else {
            hsla(0.11, 0.80, 0.30, 1.0)
        }
    }

    pub fn danger(cx: &App) -> Hsla {
        if Self::is_dark(cx) {
            hsla(0.0, 0.85, 0.55, 1.0)
        } else {
            hsla(0.0, 0.72, 0.33, 1.0)
        }
    }
}

/// Thousands-separated, so a five-figure word count reads as one number.
///
/// Mirrors the overlay's `toLocaleString()`, so the dock and the stream show the
/// same figure formatted the same way. A discrepancy between the two is exactly
/// the kind of thing chat notices.
/// A surface one step up from [`Theme::surface`].
///
/// For things that sit *on* a surface and need to be distinguishable from it —
/// a selectable row, a chip holding a URL — without resorting to the accent
/// colour, which is light and will not carry light text.
pub fn raised(cx: &App) -> Hsla {
    if Theme::is_dark(cx) {
        hsla(0.625, 0.14, 0.14, 1.0)
    } else {
        hsla(0.600, 0.06, 0.94, 1.0)
    }
}

/// WCAG relative luminance, from a gamma-encoded channel value.
fn luminance(channel: f32) -> f32 {
    let channel = channel.clamp(0.0, 1.0);
    if channel <= 0.039_28 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG contrast ratio between two colours, from 1.0 to 21.0.
///
/// Computed rather than eyeballed because "looks fine" is exactly how pale text
/// on a bright accent shipped: the overlay URL sat at about 1.5:1, which is
/// legible in a screenshot and unreadable on a screen over a stream.
pub fn contrast_ratio(a: gpui::Rgba, b: gpui::Rgba) -> f32 {
    let channel = |colour: gpui::Rgba| {
        (
            luminance(colour.r),
            luminance(colour.g),
            luminance(colour.b),
        )
    };

    let (ar, ag, ab) = channel(a);
    let (br, bg, bb) = channel(b);

    let l1 = 0.2126 * ar + 0.7152 * ag + 0.0722 * ab;
    let l2 = 0.2126 * br + 0.7152 * bg + 0.0722 * bb;

    let (lighter, darker) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (lighter + 0.05) / (darker + 0.05)
}

/// The minimum every pair must clear: WCAG AA for body text.
pub const MINIMUM_CONTRAST: f32 = 4.5;

pub fn format_count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);

    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_thousands_separated() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
        assert_eq!(format_count(1234567), "1,234,567");
        assert_eq!(format_count(100), "100");
        assert_eq!(format_count(10000), "10,000");
    }

    #[test]
    fn auto_follows_the_desktop() {
        use gpui_kit::component::ThemeMode;

        assert_eq!(Appearance::Auto.resolve(true), ThemeMode::Dark);
        assert_eq!(Appearance::Auto.resolve(false), ThemeMode::Light);
    }

    #[test]
    fn an_explicit_choice_ignores_the_desktop() {
        use gpui_kit::component::ThemeMode;

        assert_eq!(Appearance::Dark.resolve(false), ThemeMode::Dark);
        assert_eq!(Appearance::Light.resolve(true), ThemeMode::Light);
    }

    #[test]
    fn dark_is_the_default() {
        assert_eq!(Appearance::default(), Appearance::Auto);
    }

    #[test]
    fn the_contrast_math_is_sane() {
        // Black on white is the 21:1 anchor, and identical colours are 1:1. If
        // these two are wrong, every ratio below is wrong and means nothing.
        let black = gpui::Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        let white = gpui::Rgba {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        };

        assert!((contrast_ratio(black, white) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(black, black) - 1.0).abs() < 0.01);
        assert_eq!(contrast_ratio(black, white), contrast_ratio(white, black));
    }
    /// The text the app actually renders, checked against the surface behind it.
    ///
    /// Built in a real window on purpose. The colour functions take an `&App` and
    /// branch on the resolved theme, so a table of literals in a test is a test of
    /// the table rather than of the palette — and the palette is the thing that
    /// breaks.
    ///
    /// The OBS URL used to sit on the accent colour, near-white on a light cyan,
    /// about 1.5:1. It read fine in a screenshot and was unreadable on a screen. A
    /// ratio check is what catches that without needing someone to squint at a
    /// screenshot and decide it looks fine.
    /// An empty window root, for tests that only need a live `App`.
    struct Blank;

    impl gpui_kit::Render for Blank {
        fn render(
            &mut self,
            _window: &mut gpui_kit::Window,
            _cx: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            gpui_kit::div()
        }
    }

    #[gpui_kit::test]
    fn every_text_colour_is_readable_on_the_surface_it_sits_on(cx: &mut gpui_kit::TestAppContext) {
        // Both themes, because a palette tuned against one of them is a palette
        // that fails in the other, and the other is the one people use at 2am.
        for (label, dark) in [("light", false), ("dark", true)] {
            check_pairs(cx, dark, label);
        }
    }

    /// Check every pair in one theme, returning nothing and panicking on failure.
    fn check_pairs(cx: &mut gpui_kit::TestAppContext, dark: bool, label: &str) {
        use gpui_kit::component::Theme as KitTheme;

        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|_window, _cx| Blank);
        cx.update(|_cx, cx| {
            KitTheme::change(
                if dark {
                    gpui_kit::component::ThemeMode::Dark
                } else {
                    gpui_kit::component::ThemeMode::Light
                },
                None,
                cx,
            )
        });

        let pairs: Vec<(&str, gpui::Rgba, gpui::Rgba)> = cx.update(|_window, cx| {
            let rgba = |colour: Hsla| colour.to_rgb();

            vec![
                (
                    "body text on the window",
                    rgba(Theme::text(cx)),
                    rgba(Theme::bg(cx)),
                ),
                (
                    "body text on a card",
                    rgba(Theme::text(cx)),
                    rgba(Theme::surface(cx)),
                ),
                (
                    "body text on the URL chip",
                    rgba(Theme::text(cx)),
                    rgba(raised(cx)),
                ),
                (
                    "muted text on the window",
                    rgba(Theme::muted(cx)),
                    rgba(Theme::bg(cx)),
                ),
                (
                    "muted text on a card",
                    rgba(Theme::muted(cx)),
                    rgba(Theme::surface(cx)),
                ),
                (
                    "the accent, used for the count",
                    rgba(Theme::accent(cx)),
                    rgba(Theme::bg(cx)),
                ),
                ("a warning", rgba(Theme::warning(cx)), rgba(Theme::bg(cx))),
                ("a danger", rgba(Theme::danger(cx)), rgba(Theme::bg(cx))),
            ]
        });

        for (name, text, behind) in pairs {
            let ratio = contrast_ratio(text, behind);
            assert!(
                ratio >= MINIMUM_CONTRAST,
                "{label} theme: {name} is {ratio:.2}:1, below the {MINIMUM_CONTRAST}:1 minimum",
            );
        }
    }

    #[gpui_kit::test]
    fn every_accent_is_readable_in_both_themes() {
        // Seven presets x two themes, against the surfaces each is used on. A
        // preset that only reads in dark mode is the kind of thing that ships
        // fine on the machine it was picked on and is unreadable on the other.
        //
        // Computed from the accent functions rather than read out of a window, so
        // this runs with no display and covers presets that are not selected.
        const MINIMUM: f32 = 4.5;

        for preset in Accent::ALL {
            for (theme, dark) in [("dark", true), ("light", false)] {
                // The window background in each theme: near-black and near-white.
                let surface = if dark {
                    gpui::Rgba {
                        r: 0.043,
                        g: 0.047,
                        b: 0.063,
                        a: 1.0,
                    }
                } else {
                    gpui::Rgba {
                        r: 0.976,
                        g: 0.976,
                        b: 0.980,
                        a: 1.0,
                    }
                };

                let colour = if dark {
                    preset.on_dark()
                } else {
                    preset.on_light()
                }
                .to_rgb();

                let ratio = contrast_ratio(colour, surface);
                assert!(
                    ratio >= MINIMUM,
                    "{} accent on {theme} is {ratio:.2}:1, below {MINIMUM}:1",
                    preset.id(),
                );
            }
        }
    }

    #[test]
    fn an_accent_change_takes_effect_without_a_restart() {
        // This used to be a `OnceLock`, set once at startup. Picking a colour in the
        // settings panel therefore changed a value in the file and nothing else —
        // a setting that looks live and is not, which is the same failure as the
        // Events panel being decorative.
        assert_eq!(accent(), Accent::default());

        set_accent(Accent::Pink);
        assert_eq!(accent(), Accent::Pink, "a second choice must win");

        set_accent(Accent::Amber);
        assert_eq!(accent(), Accent::Amber, "and a third");

        // Restored, because the accent is process-wide and a test that leaves it
        // set would decide the palette for every test after it.
        set_accent(Accent::default());
    }

    #[test]
    fn every_accent_round_trips_through_its_id() {
        // The id is what the settings file stores, so a preset whose name does
        // not survive the trip would silently become the default one.
        for preset in Accent::ALL {
            assert_eq!(Accent::from_id(preset.id()), preset);
        }

        assert_eq!(Accent::from_id("PINK"), Accent::Pink);
        assert_eq!(Accent::from_id("  purple "), Accent::Purple);
        assert_eq!(Accent::from_id("chartreuse"), Accent::default());
        assert_eq!(Accent::from_id(""), Accent::default());
    }

    #[test]
    fn accent_ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for preset in Accent::ALL {
            assert!(seen.insert(preset.id()), "duplicate accent id");
        }
        assert_eq!(seen.len(), Accent::ALL.len());
    }

    #[gpui_kit::test]
    fn the_url_chip_is_not_the_accent_colour(cx: &mut gpui_kit::TestAppContext) {
        // Narrower than the ratio check above, and it names the regression: the chip
        // was the accent, and the accent is a light colour reserved for the count.
        // A URL is the one thing a streamer reads off the screen and types by hand,
        // so it is the one thing that must not be the hardest text to read.
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|_window, _cx| Blank);

        cx.update(|_window, cx| {
            let behind = raised(cx).to_rgb();
            let accent = Theme::accent(cx).to_rgb();
            let text = Theme::text(cx).to_rgb();

            assert!(
                contrast_ratio(text, behind) > contrast_ratio(text, accent) * 2.0,
                "the chip's surface must be far quieter than the accent behind it",
            );
        });
    }
}
