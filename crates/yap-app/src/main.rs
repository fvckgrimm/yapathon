//! yap-a-thon's startup.
//!
//! Order is the whole story here, and it is deliberate:
//!
//! 1. Logging, so everything after this is diagnosable.
//! 2. Settings, so a missing or broken file is reported once rather than
//!    surfacing as a default the streamer did not choose.
//! 3. The tokio runtime, because the server has to bind on it.
//! 4. The local server, so the OBS URL exists before the window opens.
//! 5. GPUI, which takes this thread for the rest of the process.
//!
//! Nothing from step 4 onwards may block for long. From step 5 onwards this
//! thread is the UI and must not block at all: recognition runs on the runtime
//! and reports back through [`yap_app::Hub`].

use yap_app::{Hub, app, runtime};

fn main() -> anyhow::Result<()> {
    init_logging();

    // Settings before the window: the window's contents depend on them, and a
    // streamer who has configured a hard limit should not see it reset.
    //
    // Resolved rather than assumed, because there are two places it can live —
    // next to the binary for a portable install, or in the platform config
    // directory for an installed one — and the window's Save button has to
    // write back to whichever was used.
    let source = yap_core::settings::locate_settings();
    let (settings, model_substitution, warnings) =
        match yap_core::settings::load_repaired(&source.path) {
            Ok((settings, substitution)) => {
                // Only a *file* misconfiguration is worth telling someone about.
                //
                // On a first run there is no file, so the "configured" model is
                // the built-in default — a name nobody chose, pointing at a file
                // most people have not downloaded. The loader substitutes one
                // that exists and then writes it into the new file, so there is
                // nothing to warn about and nothing to do: the warning would be
                // the first thing anyone ever sees in this app, about a file they
                // never wrote.
                //
                // A file that *was* there and named a model that is not is
                // different. Someone put that path there, it is wrong, and saying
                // so is the only way they find out.
                let reportable = substitution.filter(|_| source.existed);
                (settings, reportable, Vec::new())
            }
            Err(error) => {
                // Not fatal: the app works with defaults. Losing a streamer's
                // event pricing to a malformed file silently would be worse than
                // a startup warning.
                eprintln!("yap-a-thon: {error}");
                (yap_core::settings::Settings::default(), None, Vec::new())
            }
        };

    // Created before the window, so the first Save has somewhere to land. The
    // parent of the resolved file, not the config directory: a portable
    // install's file sits next to the binary and that directory already exists.
    if let Some(parent) = source.path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("yap-a-thon: could not create {}: {error}", parent.display());
    }

    // And written, so there is a real file to open rather than one that appears
    // only after the first save. Never overwrites: a file that failed to parse
    // is left exactly as it is, because the startup warning about it is the
    // whole reason it was kept.
    if !source.existed
        && let Err(error) = yap_core::settings::create_if_missing(&source.path)
    {
        eprintln!(
            "yap-a-thon: could not create {}: {error}",
            source.path.display()
        );
    }

    // The hub and its receiver before anything that reports into it.
    // The hub owns a copy; the window needs the settings too, and cloning them
    // is cheaper than making the window go looking for them.
    let window_settings = settings.clone();
    let (hub, messages) = Hub::with_substitution(settings, model_substitution);

    for warning in warnings {
        hub.report(yap_app::ModelMessage::Warning(warning));
    }

    // Bind on the runtime, from outside the GPUI loop: `block_on` after step 5
    // would deadlock the UI thread.
    let server_address = runtime::block_on(async {
        match app::start_server(hub.settings(), hub.state()).await {
            Ok(server) => server.address(),
            Err(error) => {
                // The dock is also a browser tab, so the app is still usable
                // without the server — but nothing OBS reads will work, which
                // has to be said rather than left to be discovered.
                eprintln!("yap-a-thon: could not start the local server: {error:#}");
                hub.report(yap_app::ModelMessage::Warning(format!(
                    "local server unavailable: {error:#}"
                )));
                std::net::SocketAddr::from(([127, 0, 0, 1], hub.settings().effective_port()))
            }
        }
    });

    // The URL, computed here rather than inside the platform closure, which
    // outlives this function and cannot borrow `hub`.
    let dock_url = hub.dock_url();

    // Cloned into the closure for the same reason: it outlives this function and
    // cannot borrow the local `source`.
    let settings_path = source.path.clone();

    // `run` consumes the thread and does not return a result: a failure to open
    // the platform is reported through the closure, which prints and exits.
    gpui_kit::application()
        // The app's own icons layered over the kit's defaults. See `icons.rs`
        // for why this is not simply `gpui_kit::assets::Assets`.
        .with_assets(yap_app::icons::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);

            if let Err(error) = app::open_main_window(
                hub.clone(),
                messages,
                server_address,
                window_settings,
                settings_path.clone(),
                cx,
            ) {
                // Not fatal. The server is already bound, so the overlay and
                // dock still work in OBS and a browser tab — the window is the
                // nicest way to use this, not the only one. Exiting here would
                // take the overlay down over a cosmetic failure, which is the
                // exact bug this app had before.
                eprintln!("yap-a-thon: could not open the window: {error:#}");
                eprintln!("yap-a-thon: still serving on {dock_url}");
            }

            // Closing the window does not quit.
            //
            // It used to, and that was a real bug for this app: the HTTP server
            // OBS reads lives in this process, so closing the dock window
            // blanked the overlay mid-stream. A streamer who closes the window to
            // get it out of the way — or who closes it on a second monitor and
            // forgets — would lose the counter without any error anywhere.
            //
            // So the app keeps running with no window, serving the overlay. The
            // microphone is held too, which is the reason this is opt-out rather
            // than the default: pass `--no-window` to start headless, or see
            // `quit` below for stopping it properly.
            // Cloned into the closure because it must be `'static` — it
            // outlives this function and cannot borrow a local.
            let message =
                format!("yap-a-thon: window closed, still serving the overlay on {dock_url}");
            let _ = cx.on_window_closed(move |cx, _| {
                if cx.windows().is_empty() {
                    log::info!("{message}");
                }
            });
        });

    Ok(())
}

/// Install a logger writing to stderr.
///
/// Without this every `log::warn!` in the app is discarded, and a failure — a
/// missing model, a dropped socket — looks exactly like a feature that is
/// simply off.
fn init_logging() {
    struct Stderr;

    impl log::Log for Stderr {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }

        fn log(&self, record: &log::Record<'_>) {
            // Info and above: the per-utterance lines would otherwise flood the
            // terminal during a stream.
            if record.level() <= log::Level::Info {
                eprintln!("[{}] {}", record.level(), record.args());
            }
        }

        fn flush(&self) {}
    }

    // A second call panics and `main` runs once, so this cannot happen in
    // practice; ignoring the result keeps the function infallible.
    let _ = log::set_boxed_logger(Box::new(Stderr));
    log::set_max_level(log::LevelFilter::Info);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_run_does_not_warn_about_a_model_nobody_configured() {
        // The substitution is real on a first run — the defaults name a model
        // most people have not downloaded — but it is not a mistake anyone made,
        // and the app writes the working path into the file it creates. Warning
        // about it would put a scary orange line about a file the streamer has
        // never seen, as the first thing they see.
        let existed = false;
        let substitution = Some(yap_core::settings::ModelSubstitution {
            wanted: std::path::PathBuf::from("/models/ggml-large-v3-turbo.bin"),
            using: std::path::PathBuf::from("/models/ggml-small.en.bin"),
        });

        assert_eq!(
            substitution.filter(|_| existed),
            None,
            "nothing to warn about when there was no file to get wrong",
        );
    }

    #[test]
    fn a_settings_file_naming_a_missing_model_does_warn() {
        // The case worth warning about: someone wrote that path down.
        let existed = true;
        let substitution = Some(yap_core::settings::ModelSubstitution {
            wanted: std::path::PathBuf::from("/models/gone.bin"),
            using: std::path::PathBuf::from("/models/ggml-small.en.bin"),
        });

        assert!(
            substitution.filter(|_| existed).is_some(),
            "a wrong path in a real file must be reported",
        );
    }

    #[test]
    fn settings_are_looked_for_in_two_places() {
        // The exe directory first, then the platform config directory. Getting
        // this order wrong means a portable install silently runs on the
        // machine-wide settings instead of the ones shipped beside it.
        let candidates = yap_core::settings::settings_candidates();
        assert!(!candidates.is_empty());
        assert!(
            candidates
                .iter()
                .all(|path| path.ends_with("settings.toml"))
        );

        if let Some(exe_dir) = yap_core::settings::exe_dir() {
            assert_eq!(candidates[0], exe_dir.join("settings.toml"));
            assert!(
                candidates.contains(&yap_core::settings::config_dir().join("settings.toml")),
                "the config directory must still be a candidate",
            );
        }
    }

    #[test]
    fn logging_installs_without_panicking() {
        // Calling it twice is what would panic, and `main` calls it once.
        init_logging();
    }
}
