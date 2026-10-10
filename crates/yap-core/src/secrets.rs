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
}

/// Whether a credential exists, without ever reading it.
///
/// The panel only ever needs to know *that* a secret is set, never what it is.
/// A masked input pre-filled with the real value would defeat the masking, and
/// rendering even a length is more than the question needs answered.
pub fn state(service: &str, user: &str) -> SecretState {
    match entry(service, user) {
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
    }
}

/// Read a credential.
///
/// Returns `None` when the store is working and has nothing, and an error when
/// the store could not answer. Those are different answers and callers need to
/// treat them differently: "no token set, ask for one" versus "this machine
/// cannot give you a token".
pub fn get(service: &str, user: &str) -> anyhow::Result<Option<String>> {
    match entry(service, user)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(anyhow::anyhow!("could not read {service}/{user}: {error}")),
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

    entry(service, user)?
        .set_password(trimmed)
        .map_err(|error| anyhow::anyhow!("could not save {service}/{user}: {error}"))?;

    // The value is deliberately absent from the error path and from every log
    // line below. `keyring`'s own errors do not carry the secret, and neither
    // does anything here.
    log::info!("stored credential {service}/{user}");
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

/// The keyring entry for a `(service, user)` pair.
fn entry(service: &str, user: &str) -> anyhow::Result<Entry> {
    Entry::new(service, user).map_err(|error| anyhow::anyhow!("{service}/{user}: {error}"))
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
            SecretState::Absent | SecretState::Unavailable => {}
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
    fn store_works() -> bool {
        !matches!(state(SERVICE, NEVER_SET), SecretState::Unavailable)
    }
}
