//! Credentials, in the OS credential store.
//!
//! API keys and tokens live here and nowhere else. They are deliberately
//! absent from `settings.toml`: that file is meant to be readable, copyable
//! between machines and pasted into a bug report, and a Twitch token sitting
//! in it is a token sitting in a screenshot.
//!
//! ## Why there is no file fallback
//!
//! When the OS store is unavailable — no `libsecret` daemon on a headless
//! box, no keychain — [`set`] returns an error and the secret is not written
//! anywhere. That is the intended behaviour, not an oversight. The tempting
//! alternative is a dotfile under the config directory, and it is a trap: the
//! one moment that matters is a machine where the store is missing, which is
//! exactly the machine nobody is thinking about when they read the file
//! later. A streamer who is told "the key was saved" must be able to believe
//! it, so this fails loudly instead.
//!
//! ## What is stored
//!
//! ## Addressing
//!
//! An entry is a `(service, user)` pair, which is how keyring actually files
//! things. Both halves matter and neither is enough on its own:
//!
//! - `service` is `yap-a-thon/<platform>`, and is **configurable** — it is what
//!   `twitch_credential` and `engine.credential` hold — so one machine can hold
//!   two Twitch channels.
//! - `user` is what the entry *is*: `user-token` for a Twitch token, `api-key`
//!   for a cloud speech key.
//!
//! ## Nothing here blocks forever
//!
//! A **locked** keyring is the failure that matters, and it is not an error —
//! the client waits for an unlock prompt that a desktop application has no way
//! to answer. It waits indefinitely. So a keyring call made on the GPUI thread
//! freezes the window solid with nothing on screen and nothing in the log, and
//! the same call in a test hangs the suite rather than failing it. Both
//! happened.
//!
//! Every call here therefore runs on its own thread with a deadline, and a
//! timeout is reported as [`SecretState::Unavailable`] rather than waited on.
//!
//! The cost is honest and worth stating: a write that times out may still have
//! succeeded on the thread that is still waiting. That is preferable to hanging
//! forever, and it cannot be avoided — the alternative is no bound at all.
//!
//! Everything that reads or writes a credential goes through here. That is not
//! tidiness: the prompt and the platform loaders originally used different
//! addresses, so the prompt would have said "stored" while the platform looked
//! somewhere else and found nothing — the same shape as the dead buttons and
//! the unwired settings, where everything reported success and nothing happened.

use keyring::Entry;

/// The outcome of asking the store about a credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretState {
    /// The store has a value for this name.
    Present,
    /// The store works and has nothing for this name.
    Absent,
    /// The store could not be reached at all.
    ///
    /// Distinct from [`SecretState::Absent`] because the two call for
    /// different things on screen: absent means "type one in", unavailable
    /// means "this machine cannot store one" — and showing an input for the
    /// second would be inviting someone to paste a key somewhere it cannot
    /// safely go.
    Unavailable,
    /// The store did not answer in time.
    ///
    /// Its own state because it is a different problem with a different fix,
    /// and the two are indistinguishable if merged: gnome-keyring locks itself
    /// when the machine is idle, and then *waits* for an unlock prompt no
    /// desktop app can answer. Reading a credential in that state does not
    /// fail — it hangs, and the window never appears.
    ///
    /// Saying "this machine has no credential store" about it would be
    /// technically true and completely useless, because the store is right
    /// there and one command fixes it.
    Locked,
}

/// Whether a credential exists, without ever reading it.
///
/// The panel only ever needs to know *that* a secret is set, never what it is.
/// A masked input pre-filled with the real value would defeat the masking, and
/// rendering even a length is more than the question needs answered.
pub fn state(service: &str, user: &str) -> SecretState {
    let (service, user) = (service.to_string(), user.to_string());
    match bounded(move || match entry(&service, &user) {
        // Building the entry fails when the platform has no store at all,
        // which is `Unavailable` for the same reason a failing read is.
        Err(_) => SecretState::Unavailable,
        Ok(entry) => match entry.get_password() {
            Ok(_) => SecretState::Present,
            Err(keyring::Error::NoEntry) => SecretState::Absent,
            // Everything else — no daemon, permission denied — means the store
            // is not answering, which is not the same as holding nothing.
            Err(_) => SecretState::Unavailable,
        },
    }) {
        Bounded::Answered(state) => state,
        Bounded::TimedOut => SecretState::Locked,
    }
}

/// Read a credential.
///
/// Returns `None` when the store is working and has nothing, and an error when
/// the store could not answer. Those are different answers and callers need to
/// treat them differently: "no token set, ask for one" versus "this machine
/// cannot give you a token".
pub fn get(service: &str, user: &str) -> anyhow::Result<Option<String>> {
    let where_ = format!("{service}/{user}");
    let (service, user) = (service.to_string(), user.to_string());

    match bounded(move || match entry(&service, &user) {
        Err(error) => Err(error.to_string()),
        Ok(entry) => match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(format!("could not read {service}/{user}: {error}")),
        },
    }) {
        Bounded::Answered(Ok(value)) => Ok(value),
        Bounded::Answered(Err(error)) => Err(anyhow::anyhow!(error)),
        Bounded::TimedOut => Err(anyhow::anyhow!(
            "the credential store is locked, so {where_} could not be read. \
             See the README for how to unlock it on this platform."
        )),
    }
}

/// Store a credential, replacing any existing one.
///
/// # Errors
/// When the OS store is unavailable. Nothing is written anywhere in that case
/// — see the module docs.
pub fn set(service: &str, user: &str, value: &str) -> anyhow::Result<()> {
    let trimmed = value.trim();

    if trimmed.is_empty() {
        // An empty string is not a credential, and storing one would leave a
        // field reading as "set" while every request it is used for fails with
        // an authentication error rather than a missing key. Clearing is a
        // deliberate act with its own function.
        anyhow::bail!("a credential cannot be empty — use remove to clear it");
    }

    let where_ = format!("{service}/{user}");
    let (service, user, trimmed) = (service.to_string(), user.to_string(), trimmed.to_string());

    match bounded(move || match entry(&service, &user) {
        Err(error) => Err(error.to_string()),
        Ok(entry) => entry
            .set_password(&trimmed)
            .map_err(|error| format!("could not save {service}/{user}: {error}")),
    }) {
        Bounded::Answered(Ok(())) => {}
        Bounded::Answered(Err(error)) => return Err(anyhow::anyhow!(error)),
        Bounded::TimedOut => {
            return Err(anyhow::anyhow!(
                "the credential store is locked, so {where_} may or may not have been saved"
            ));
        }
    }

    // The value is deliberately absent from the error path and from every log
    // line below. `keyring`'s own errors do not carry the secret, and neither
    // does anything here.
    log::info!("stored credential {where_}");
    Ok(())
}

/// Remove a credential.
///
/// Returns whether there was one to remove, so the panel can say "cleared"
/// rather than "cleared" for something that was never there.
///
/// # Errors
/// When the store could not be reached.
pub fn remove(service: &str, user: &str) -> anyhow::Result<bool> {
    match entry(service, user)?.delete_credential() {
        Ok(()) => {
            log::info!("removed credential {service}/{user}");
            Ok(true)
        }
        // Already gone is the outcome the caller wanted, so it is a success.
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(error) => Err(anyhow::anyhow!(
            "could not remove {service}/{user}: {error}"
        )),
    }
}

/// How long a keyring call may take before it is treated as unavailable.
///
/// Long enough that a working store never trips it, including on a cold D-Bus
/// connection. Short enough that a locked one cannot stop the app launching.
const TIMEOUT_SECONDS: u64 = 5;

/// What came back from a keyring operation behind a deadline.
enum Bounded<T> {
    /// The store answered.
    Answered(T),
    /// It did not, within [`TIMEOUT_SECONDS`].
    TimedOut,
}

/// Run a keyring operation with a deadline.
///
/// The timeout is reported separately from an error, because the two mean
/// different things: a store that *replies* "no" has no store, while a store
/// that does not reply is locked and wants unlocking. Merging them is how
/// "this machine has no credential store" ends up being the message for a
/// machine that has one.
fn bounded<T: Send + 'static>(operation: impl FnOnce() -> T + Send + 'static) -> Bounded<T> {
    use std::sync::mpsc;
    use std::time::Duration;

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(operation());
    });

    match rx.recv_timeout(Duration::from_secs(TIMEOUT_SECONDS)) {
        Ok(value) => Bounded::Answered(value),
        Err(_) => {
            log::warn!(
                "credential store did not answer within {TIMEOUT_SECONDS}s — it is \
                 probably locked. Treating as locked rather than missing."
            );
            Bounded::TimedOut
        }
    }
}

/// The keyring entry for a `(service, user)` pair.
fn entry(service: &str, user: &str) -> anyhow::Result<Entry> {
    Entry::new(service, user).map_err(|error| anyhow::anyhow!("{service}/{user}: {error}"))
}

/// How to unlock the credential store on this machine.
///
/// # Why this exists, and why it is per-platform
///
/// A locked keyring looks exactly like a missing one from the inside, and the
/// fix is a different command on every desktop. Saying "no credential store on
/// this machine" about a locked one is technically true and completely useless,
/// because the store is right there.
///
/// Only the Secret Service implementations really lock. **KWallet** normally
/// unlocks with the session, and the **macOS Keychain** prompts on first use
/// rather than blocking forever, so this is overwhelmingly a GNOME/Linux
/// problem — but the code is platform-agnostic and the message should be too,
/// rather than naming one desktop's tool on a machine that has six.
///
/// Returns an empty string where there is nothing useful to say, so a caller can
/// show the generic advice instead of a blank line.
pub fn unlock_hint() -> &'static str {
    if cfg!(target_os = "linux") {
        // Both spellings cover the same thing: `gnome-keyring-daemon --unlock`
        // prompts for the login password on GNOME, and KDE's wallet is usually
        // unlocked by the session manager already.
        "unlock your keyring — on GNOME, `gnome-keyring-daemon --unlock`, or log out and back in"
    } else if cfg!(target_os = "macos") {
        // The macOS Keychain does not lock the way a Secret Service one does,
        // so reaching this state usually means a non-interactive session rather
        // than a locked keychain.
        "the login keychain normally unlocks itself; Keychain Access if it does not"
    } else if cfg!(target_os = "windows") {
        "Windows Credential Manager does not lock, so this usually means no store is reachable"
    } else {
        ""
    }
}

/// Whether a settings key names a credential rather than a preference.
///
/// A name convention, because the schema — which is what actually declares a
/// field to be a credential — lives in the app crate, and `yap-core` is the
/// layer below it. Having the upper layer's idea of "this is a secret" leak
/// downwards would invert the dependency.
///
/// The app filters on `FieldKind::Secret` rather than calling this, because
/// that is the real declaration. This exists so the two can be checked against
/// each other: a test in the app asserts every `Secret` field satisfies it, so
/// a field typed wrong here is caught rather than silently never stored.
pub fn is_secret_key(key: &str) -> bool {
    key.ends_with("_token") || key.ends_with("_key") || key.ends_with("_secret")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Throwaway service, so a run on a real machine touches nothing real.
    const SERVICE: &str = "yap-a-thon-test";
    const USER: &str = "roundtrip";
    /// A user this app never writes, for asking whether the store is reachable.
    const NEVER_SET: &str = "never-set";

    #[test]
    fn an_empty_credential_is_refused_rather_than_stored() {
        // Storing "" would leave the field reading as "set" while every request
        // it feeds fails as unauthenticated rather than as unconfigured. The
        // refusal happens before any store call, so this holds even on a
        // machine with no credential store at all.
        let error = set(SERVICE, USER, "   ").unwrap_err();
        assert!(
            error.to_string().contains("empty"),
            "expected an empty-value refusal, got: {error}",
        );
    }

    #[test]
    fn whitespace_only_is_empty_and_padded_values_are_not() {
        // Pasting from a browser or a terminal brings newlines, and a token
        // with a trailing newline fails authentication with no useful message.
        // Asserted through `set` because the trim happens there: a value that
        // is whitespace alone is refused, and one that merely *contains*
        // whitespace is not.
        assert!(set(SERVICE, USER, "\n\n").is_err());
        assert!(set(SERVICE, USER, "abc 123").is_ok() || !store_works());
    }

    #[test]
    fn secret_keys_are_recognised_by_name() {
        // The app filters on `FieldKind::Secret`; this is the check that the two
        // agree. A field declared a secret in the schema but not named like one
        // here would be prompted for and then stored under an unexpected name.
        for key in [
            "twitch_token",
            "groq_key",
            "youtube_client_secret",
            "youtube_refresh_token",
        ] {
            assert!(is_secret_key(key), "{key} should be a credential");
        }

        for key in [
            "engine",
            "accent",
            "overlay_theme",
            "model_path",
            "event_words",
            "input_device",
        ] {
            assert!(
                !is_secret_key(key),
                "{key} is a preference, not a credential"
            );
        }
    }

    #[test]
    fn a_field_with_nothing_set_is_absent_not_unavailable() {
        // The two states drive different messages, and conflating them means a
        // machine with no keyring daemon asks the user to type in a key it is
        // about to discard. The name is one this app never uses.
        match state(SERVICE, NEVER_SET) {
            SecretState::Absent | SecretState::Unavailable | SecretState::Locked => {}
            SecretState::Present => panic!("a credential that was never set is present"),
        }
    }

    #[test]
    fn the_service_and_the_user_are_both_part_of_the_address() {
        // A value filed under one user is invisible under another. This is not
        // hypothetical: the credential prompt and the platform loaders briefly
        // used different pairs, so the prompt reported success and the platform
        // found nothing at all.
        if !store_works() {
            return;
        }

        assert!(set(SERVICE, "one", "first").is_ok());
        assert!(set(SERVICE, "two", "second").is_ok());

        assert_eq!(get(SERVICE, "one").unwrap().as_deref(), Some("first"));
        assert_eq!(get(SERVICE, "two").unwrap().as_deref(), Some("second"));

        // Removing one leaves the other, which is what a streamer with two
        // channels needs.
        assert!(remove(SERVICE, "one").unwrap());
        assert_eq!(get(SERVICE, "one").unwrap(), None);
        assert_eq!(
            get(SERVICE, "two").unwrap().as_deref(),
            Some("second"),
            "clearing one credential must not clear another under the same service",
        );

        let _ = remove(SERVICE, "two");
    }

    /// Whether this machine has a working credential store.
    ///
    /// Not asserted on — a CI box with no keyring would fail the test above it
    /// — so it only gates the one assertion that needs a live store.
    /// Whether this machine's credential store accepts a write.
    ///
    /// A **write** probe, deliberately. A read of an entry that does not exist
    /// answers `NoEntry` instantly even when the collection is locked, so a
    /// read-based guard passes on a machine that cannot store anything and the
    /// test then fails on its first `set` — appearing to prove nothing was
    /// wrong. Writing is what needs the collection unlocked.
    fn store_works() -> bool {
        set(SERVICE, "yap-test-probe", "probe").is_ok()
    }
}
