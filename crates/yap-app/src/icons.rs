//! The icon set, and the asset source that serves it.
//!
//! # Why this is not just `gpui_kit::assets::Assets`
//!
//! That bundle holds 104 icons — the ones GPUI Kit's own widgets draw, like the
//! chevron in a dropdown. It has no microphone, no target, and no rocket, which
//! is everything a settings section wants to be recognisable by. The full
//! catalogue is 1830 icons and 731 KB of SVG embedded in the binary, to draw
//! about a dozen of them.
//!
//! So the icons the app actually names are listed explicitly here and embedded
//! on their own, with the kit's defaults still reachable underneath. The list
//! is the whole point: an icon that is not on it renders as nothing, silently,
//! which is the failure this project keeps meeting in other shapes.
//!
//! `IconName` is a name, not a guarantee. Nothing about
//! `IconName::Mic.path()` says the bytes are anywhere, so
//! [`every_icon_this_app_names_is_actually_served`] is the test that closes
//! that gap.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

// Every icon the app names, and nothing else.
//
// Kept alphabetical so a new one is easy to place and a duplicate is easy to
// spot. Adding an icon here is the *only* step between naming it and it
// drawing — the name resolves to a path, the path resolves to these bytes.
//
// A comment rather than a doc comment: this is a macro invocation, and `///`
// on one documents the expansion rather than the item.
gpui_kit::assets::icon_assets!(
    YapIcons,
    [
        Cpu,
        Heart,
        MessagesSquare,
        Mic,
        Orbit,
        Play,
        Radio,
        Rocket,
        Server,
        Sigma,
        Target,
    ]
);

/// The app's icons, with GPUI Kit's own underneath.
///
/// The fallback is not optional. GPUI Kit's widgets draw from their own bundle
/// — the chevron in a dropdown, the tick in a checkbox — and replacing the
/// source rather than layering onto it turns every one of those into an empty
/// box. The app's icons are asked for first so a name in both bundles resolves
/// to the app's copy.
#[derive(Clone, Copy, Debug, Default)]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = YapIcons.load(path)? {
            return Ok(Some(bytes));
        }

        // `Err` for a miss, not `Ok(None)`, is what the kit's own source does.
        //
        // Propagating that would make every unknown icon an error rather than
        // an absence, and the difference is not cosmetic: an icon nobody asked
        // for is a normal thing for a widget to ask about while it lays out,
        // and answering with a failure turns a gap into a fault. The
        // `icon_assets!` macro above returns `Ok(None)` for the same case, so
        // this is also the only way the two halves of this source agree.
        Ok(gpui_kit::assets::Assets.load(path).unwrap_or(None))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = YapIcons.list(path)?;

        // Deduped, because an icon in both bundles would otherwise be listed
        // twice and a caller walking the list would load it twice.
        for other in gpui_kit::assets::Assets.list(path)? {
            if !paths.contains(&other) {
                paths.push(other);
            }
        }

        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use gpui::AssetSource;
    use gpui_kit::assets::IconName;

    use super::Assets;

    /// The icons the app puts on screen, by name.
    ///
    /// Spelled out here rather than imported from the schema so this test is
    /// checking a list that was written down independently of the code that
    /// draws. A test that imported `section.icon()` would be checking that the
    /// schema agrees with itself.
    const NAMED: &[(&str, IconName)] = &[
        ("Engine", IconName::Cpu),
        ("Events · Twitch", IconName::Heart),
        ("Events · YouTube", IconName::Play),
        ("Events · Kick", IconName::Rocket),
        ("Events · Chat", IconName::MessagesSquare),
        ("Goal", IconName::Target),
        ("Counting", IconName::Sigma),
        ("Voice", IconName::Mic),
        ("Astro", IconName::Orbit),
        ("Twitch", IconName::Radio),
        ("Server", IconName::Server),
    ];

    #[test]
    fn every_icon_this_app_names_is_actually_served() {
        // The gap this closes: `IconName::Mic` is a *name*, and the name
        // resolves to a path. Nothing anywhere says the bytes for that path
        // exist. Name an icon that is not embedded and it renders as nothing —
        // no error, no log line, a settings section with a gap where its
        // symbol should be.
        //
        // GPUI Kit's own 104 icons would cover the first few of these by
        // accident. The full 1830-icon catalogue would cover all of them and
        // cost 731 KB. This bundle is explicit, so this test is what keeps it
        // honest.
        for (section, icon) in NAMED {
            let path = icon.path();

            let bytes = Assets
                .load(&path)
                .unwrap_or_else(|error| {
                    panic!("{section}: {icon:?} at {path:?} errored: {error:#}")
                })
                .unwrap_or_else(|| {
                    panic!(
                        "{section}: {icon:?} is named but not served — add it to the \
                         icon_assets! list in icons.rs"
                    )
                });

            assert!(!bytes.is_empty(), "{section}: {icon:?} is served but empty",);
        }
    }

    #[test]
    fn the_kits_own_icons_are_still_reachable() {
        // The other half of layering rather than replacing. GPUI Kit's widgets
        // draw from their own bundle, and an asset source that answers only
        // for the app's names turns every chevron and tick into an empty box.
        //
        // Nothing in this crate names these two, so nothing else would notice.
        let fallback = gpui_kit::assets::Assets;

        for icon in [IconName::ChevronDown, IconName::Bot] {
            let path = icon.path();

            assert!(
                Assets.load(&path).unwrap().is_some(),
                "{icon:?} is drawn by a kit widget but the app's source cannot serve it",
            );
            assert!(
                fallback.load(&path).unwrap().is_some(),
                "{icon:?} is expected to come from the kit's own bundle",
            );
        }
    }

    #[test]
    fn an_icon_nobody_asked_for_is_not_served() {
        // The other direction, and the reason the list is a list. If the source
        // answered for everything, this would pass too, and 731 KB of SVG would
        // be on disk for no reason.
        let asked_for = IconName::Mic.path();

        assert!(Assets.load(&asked_for).unwrap().is_some());
        assert!(
            Assets.load(&IconName::Anchor.path()).unwrap().is_none(),
            "an icon outside the list must not resolve, or the list means nothing",
        );
    }

    #[test]
    fn listing_agrees_with_serving() {
        // A path that `list` reports and `load` cannot produce is the same
        // silent gap as one that was never listed, reached a different way.
        let listed = Assets.list("icons/").unwrap();

        assert!(!listed.is_empty());
        for path in &listed {
            assert!(
                Assets.load(path).unwrap().is_some(),
                "{path} is listed but cannot be loaded",
            );
        }
    }

    #[test]
    fn listing_does_not_repeat_an_icon_in_both_bundles() {
        // `Bot` is in the kit's default bundle and `Mic` is not. An icon in both
        // would be loaded, drawn, and listed twice.
        let listed = Assets.list("icons/").unwrap();

        let mut seen = listed.clone();
        seen.sort();
        let before = seen.len();
        seen.dedup();

        assert_eq!(
            before,
            seen.len(),
            "the listing repeats a path, so a caller walking it loads it twice",
        );
    }
}
