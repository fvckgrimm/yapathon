//! Persistent settings.
//!
//! Secrets are not stored here. API keys live in the OS credential store and
//! this file records only which keyring entry to look up, because a settings
//! file is the one place on a streaming machine that gets synced, backed up and
//! pasted into a support thread.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::counting::CountingRules;
use crate::engine::VadConfig;
use crate::goal::GoalMode;
use crate::session::SessionConfig;

/// Which speech recognition backend to use.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EngineChoice {
    /// whisper.cpp on this machine. Free, offline, private.
    Local {
        /// Path to a ggml model file.
        model_path: PathBuf,
        /// ggml backend. Auto probes Metal and CUDA, then falls back to CPU.
        backend: LocalBackend,
    },
    /// Groq's hosted whisper-large-v3-turbo.
    Groq {
        /// Name of the credential-store entry holding the key.
        credential: String,
    },
}

impl EngineChoice {
    /// Short label for the settings UI.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Local { .. } => "local whisper.cpp",
            Self::Groq { .. } => "Groq (cloud)",
        }
    }

    /// Whether audio leaves the machine. Shown next to the choice.
    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Groq { .. })
    }

    /// List price in US dollars per hour of audio, where known.
    pub fn cost_per_hour_usd(&self) -> Option<f64> {
        match self {
            Self::Local { .. } => None,
            // whisper-large-v3-turbo on Groq.
            Self::Groq { .. } => Some(0.04),
        }
    }

    /// The model both backends run, which keeps their counts comparable.
    pub fn model_name(&self) -> &str {
        match self {
            Self::Local { backend: _, .. } => "whisper-large-v3-turbo",
            Self::Groq { .. } => "whisper-large-v3-turbo",
        }
    }
}

/// Compute backend for the local engine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalBackend {
    /// Probe for Metal, then CUDA, then fall back to CPU.
    #[default]
    Auto,
    Metal,
    Cuda,
    /// Vulkan, where Metal and CUDA are both absent.
    Vulkan,
    Cpu,
}

impl LocalBackend {
    /// Human-readable name, for the settings row.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Auto => "auto-detect",
            Self::Metal => "Metal",
            Self::Cuda => "CUDA",
            Self::Vulkan => "Vulkan",
            Self::Cpu => "CPU",
        }
    }
}

/// A credential-store entry name, stored in settings so the key itself is not.
pub fn credential_name(service: &str) -> String {
    format!("yap-a-thon/{service}")
}

/// Everything the app remembers between runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Where to bind the HTTP server that OBS talks to.
    pub bind_address: String,
    /// Port for the same server.
    pub port: u16,

    /// Recognition backend.
    pub engine: EngineChoice,

    /// Voice activity detection.
    pub vad: VadConfig,

    /// Which microphone to listen to.
    ///
    /// Empty means the system default, which is what almost everyone wants. The
    /// name is stored rather than an index because indices are assigned by
    /// enumeration order and move when a device is plugged in — a settings file
    /// holding "input 2" means a different microphone after lunch.
    #[serde(default)]
    pub input_device: String,

    /// Counting rules applied to new sessions.
    pub rules: CountingRules,

    /// The goal a new yapathon starts with.
    pub default_mode: GoalMode,

    /// Hard limit applied to a new yapathon, if any.
    pub default_hard_limit: Option<u32>,

    /// Words granted per event kind, keyed by
    /// `<platform>:<kind>`, e.g. `twitch:sub` or `chat:command`.
    pub event_words: BTreeMap<String, u32>,

    /// Whether to keep every utterance's text on disk.
    ///
    /// On by default and worth stating plainly: this is a recording of what was
    /// said, kept locally. Turning it off keeps only the totals, which makes a
    /// disputed count unanswerable.
    pub keep_transcript: bool,

    /// Twitch EventSub settings.
    pub twitch_channel: Option<String>,
    /// Name of the credential-store entry holding the Twitch user token.
    pub twitch_credential: Option<String>,
    /// The registered Twitch application id.
    ///
    /// Public, and required by EventSub on the WebSocket URL itself, so a
    /// connection without it is refused before the token is even read. Each
    /// streamer registers their own application at
    /// `dev.twitch.tv/console/apps`.
    pub twitch_client_id: Option<String>,

    /// How often to ask YouTube for new chat messages, in seconds. Low enough to
    /// feel live, high enough to stay inside the daily quota.
    pub youtube_poll_secs: u64,
}

impl Default for Settings {
    fn default() -> Self {
        let mut event_words = BTreeMap::new();
        event_words.insert("twitch:sub".to_string(), 100);
        event_words.insert("twitch:resub".to_string(), 100);
        event_words.insert("twitch:gift_sub".to_string(), 50);
        event_words.insert("twitch:cheer".to_string(), 1);
        event_words.insert("twitch:raid".to_string(), 250);
        event_words.insert("twitch:redemption".to_string(), 100);
        event_words.insert("chat:command".to_string(), 100);

        Self {
            // Loopback only. Binding this to anything else would publish the
            // transcript and the goal over the network.
            bind_address: "127.0.0.1".to_string(),
            port: 8973,
            engine: EngineChoice::Local {
                model_path: default_model_path(),
                backend: LocalBackend::Auto,
            },
            vad: VadConfig::default(),
            // The system default, not the first device found: on a machine with
            // a webcam microphone and a USB one, "first" is enumeration order
            // and changes when something is unplugged.
            input_device: String::new(),
            rules: CountingRules::default(),
            default_mode: GoalMode::Extend { base_words: 5000 },
            default_hard_limit: Some(10_000),
            event_words,
            keep_transcript: true,
            twitch_channel: None,
            twitch_credential: Some(credential_name("twitch")),
            twitch_client_id: None,
            youtube_poll_secs: 15,
        }
    }
}

impl Settings {
    /// The session config a new yapathon starts with.
    pub fn new_session_config(&self) -> SessionConfig {
        let limit = self
            .default_hard_limit
            .map(|max| crate::goal::HardLimit { max });
        SessionConfig::new(self.default_mode.clone(), limit).with_rules(self.rules.clone())
    }

    /// Words granted by an event, looking up `<platform>:<kind>`.
    ///
    /// A kind with no configured value grants nothing rather than defaulting
    /// to something. Silently paying out for an event the streamer has not
    /// priced would move the goal without anyone agreeing to it.
    pub fn words_for_event(&self, platform: &str, kind: &str) -> Option<u32> {
        self.event_words.get(&format!("{platform}:{kind}")).copied()
    }

    /// The port to listen on, guarding against a zero port from a hand-edited
    /// file.
    pub fn effective_port(&self) -> u16 {
        if self.port == 0 { 8973 } else { self.port }
    }
}

/// Where model files are looked for.
///
/// Under the platform data directory, so it is per-OS without a hardcoded path:
/// a streamer keeps the model next to the app's own data. This is also where a
/// model downloaded by hand usually ends up, which is why discovery looks here
/// first.
pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

/// Where a model file lives when the streamer has not chosen one.
pub fn default_model_path() -> PathBuf {
    models_dir().join("ggml-large-v3-turbo.bin")
}

/// A model file found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundModel {
    pub path: PathBuf,
    /// Approximate size, for the picker.
    pub size_bytes: u64,
}

impl FoundModel {
    /// Just the file name, which is what a person recognises.
    ///
    /// Full paths in a dropdown are unreadable past the home directory, and the
    /// file name is the part that says which model it is.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// A human-readable size, so a 466 MB model and a 1.6 GB one are told apart
    /// without opening a file manager.
    pub fn size_label(&self) -> String {
        // Every unit from bytes up. Skipping kilobytes here is how a 466 MB
        // model ends up labelled "465 GB", which is the kind of small lie that
        // makes a picker untrustworthy.
        const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

        let mut value = self.size_bytes as f64;
        let mut unit = 0;

        while value >= 1024.0 && unit < UNITS.len() - 1 {
            value /= 1024.0;
            unit += 1;
        }

        if unit == 0 {
            format!("{:.0} {}", value, UNITS[unit])
        } else {
            format!("{value:.1} {}", UNITS[unit])
        }
    }
}

/// Find model files the streamer already has.
///
/// Looks in the app's own `models/` directory and in the current directory, so
/// a model dropped next to the binary or downloaded into Downloads and moved is
/// found without any configuration.
///
/// Sorted largest first: the most capable model is usually the one wanted, and
/// "which of these do I pick" is the question a list of filenames raises.
pub fn discover_models() -> Vec<FoundModel> {
    let mut found = Vec::new();

    for dir in [models_dir(), PathBuf::from(".")] {
        for model in discover_models_in(&dir) {
            if !found.contains(&model) {
                found.push(model);
            }
        }
    }

    found.sort_by(|a, b| {
        b.size_bytes
            .cmp(&a.size_bytes)
            .then(a.name().cmp(&b.name()))
    });
    found
}

/// Find model files in one directory.
///
/// Split out so the discovery rules are testable without writing into the real
/// data directory.
pub fn discover_models_in(dir: &Path) -> Vec<FoundModel> {
    let mut found: Vec<FoundModel> = Vec::new();

    let Ok(entries) = std::fs::read_dir(dir) else {
        // A missing directory is the normal case on first run, not an error.
        return found;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if !is_model_file(&path) {
            continue;
        }

        // Canonicalised before comparing, so the same file reached through two
        // paths is not offered twice.
        let Ok(canonical) = path.canonicalize() else {
            continue;
        };

        if found.iter().any(|m| m.path == canonical) {
            continue;
        }

        let size_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
        found.push(FoundModel {
            path: canonical,
            size_bytes,
        });
    }

    found.sort_by(|a, b| {
        b.size_bytes
            .cmp(&a.size_bytes)
            .then(a.name().cmp(&b.name()))
    });
    found
}

/// Whether a path looks like a ggml model file.
/// `ggml` in the name and a `.bin` extension, which is how every published
/// whisper.cpp model is named. Deliberately loose: rejecting a model because it
/// was renamed is worse than listing one that turns out to be something else.
fn is_model_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    path.is_file() && name.contains("ggml") && name.ends_with(".bin")
}

/// The first discovered model, preferring one that exists over the default.
///
/// The default path is returned when nothing is found, so a caller that just
/// wants "a sensible model path" gets the configured one unchanged rather than
/// an empty string it has to special-case.
pub fn discover_model_or_default(configured: &Path) -> PathBuf {
    let candidates = discover_models();

    if let Some(found) = candidates
        .iter()
        .find(|m| m.path == configured)
        .or_else(|| candidates.first())
    {
        return found.path.clone();
    }

    configured.to_path_buf()
}

/// Load settings from `path`, falling back to defaults if absent.
///
/// A malformed file is an error rather than a silent reset: losing a streamer's
/// event pricing because of a stray character would be worse than a startup
/// error they can fix.
///
/// A local engine whose configured model is missing is pointed at one that was
/// found on disk. Not silently — [`load_repaired`] says what was swapped — but
/// automatic, because the alternative is a streamer who downloaded a model, put
/// it in the documented place, and gets "no model" until they find a settings
/// screen that has no way to browse for a file.
pub fn load(path: &Path) -> Result<Settings, SettingsError> {
    load_repaired(path).map(|(settings, _)| settings)
}

/// A model the loader swapped out, and what for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSubstitution {
    /// The path the settings asked for.
    pub wanted: PathBuf,
    /// The path actually in use.
    pub using: PathBuf,
}

/// [`load`], and what it had to change on the way in.
///
/// The substitution has to be reported by the loader rather than worked out by
/// the caller afterwards. The alternative — comparing the model in use against
/// the built-in default name — reports a warning for every install that is not
/// running the exact model the defaults happen to name, which is every install.
/// A warning that is always true teaches a streamer to ignore warnings, and the
/// one time it matters is the one they have stopped reading.
///
/// `None` means the settings were used exactly as written, which is the case
/// that should be silent.
pub fn load_repaired(path: &Path) -> Result<(Settings, Option<ModelSubstitution>), SettingsError> {
    let settings = load_raw(path)?;
    let (settings, substitution) = repair_model_path_reporting(settings, &discover_models());

    if let Some(sub) = &substitution {
        log::info!(
            "model {} does not exist; using {} instead",
            sub.wanted.display(),
            sub.using.display(),
        );
    }

    Ok((settings, substitution))
}

/// Point a local engine at a model that actually exists, reporting the swap.
pub fn repair_model_path_reporting(
    mut settings: Settings,
    candidates: &[FoundModel],
) -> (Settings, Option<ModelSubstitution>) {
    let EngineChoice::Local { model_path, .. } = &settings.engine else {
        // A cloud engine has no local model, so nothing to repair.
        return (settings, None);
    };

    if model_path.is_file() {
        return (settings, None);
    }

    let Some(found) = candidates
        .iter()
        .find(|m| m.path == *model_path)
        .or_else(|| candidates.first())
    else {
        // Nothing on disk. The caller reports this, naming the directory.
        return (settings, None);
    };

    let substitution = ModelSubstitution {
        wanted: model_path.clone(),
        using: found.path.clone(),
    };

    let backend = match &settings.engine {
        EngineChoice::Local { backend, .. } => *backend,
        // Unreachable: the early return above rules out a cloud engine.
        EngineChoice::Groq { .. } => LocalBackend::default(),
    };

    settings.engine = EngineChoice::Local {
        model_path: found.path.clone(),
        backend,
    };

    (settings, Some(substitution))
}

/// Read the settings file with no repair applied.
///
/// Used by tests that assert a file round-trips exactly, and by the settings
/// panel's reload, which must show what is *on disk* rather than what the app
/// substituted for it.
pub fn load_raw(path: &Path) -> Result<Settings, SettingsError> {
    if !path.exists() {
        return Ok(Settings::default());
    }

    let text = std::fs::read_to_string(path).map_err(|source| SettingsError::Read {
        path: path.to_path_buf(),
        source,
    })?;

    toml::from_str(&text).map_err(|source| SettingsError::Parse {
        path: path.to_path_buf(),
        message: source.to_string(),
    })
}

/// Point a local engine at a model that actually exists.
pub fn repair_model_path(settings: Settings) -> Settings {
    let candidates = discover_models();
    repair_model_path_with(settings, &candidates)
}

/// The repair, given the candidates.
///
/// Split from [`repair_model_path`] so it can be tested without depending on
/// whatever models happen to exist on the machine running the tests — which is
/// the difference between a test that means something and one that passes on a
/// developer laptop and fails in CI.
fn repair_model_path_with(mut settings: Settings, candidates: &[FoundModel]) -> Settings {
    let EngineChoice::Local { model_path, .. } = &settings.engine else {
        // A cloud engine has no local model, so nothing to repair.
        return settings;
    };

    if model_path.is_file() {
        return settings;
    }

    let Some(found) = candidates
        .iter()
        .find(|m| m.path == *model_path)
        .or_else(|| candidates.first())
    else {
        // Nothing on disk. The caller reports this, naming the directory.
        return settings;
    };

    log::info!(
        "model {} does not exist; using {} instead",
        model_path.display(),
        found.path.display()
    );

    settings.engine = EngineChoice::Local {
        model_path: found.path.clone(),
        backend: match &settings.engine {
            EngineChoice::Local { backend, .. } => *backend,
            _ => unreachable!("checked above"),
        },
    };

    settings
}

/// Write settings to `path`, creating parent directories.
///
/// Written to a temporary file and renamed, so a crash mid-write cannot leave a
/// truncated file that fails to parse on next launch.
pub fn save(path: &Path, settings: &Settings) -> Result<(), SettingsError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| SettingsError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let text = toml::to_string_pretty(settings).map_err(|source| SettingsError::Parse {
        path: path.to_path_buf(),
        message: source.to_string(),
    })?;

    let temp = path.with_extension("toml.tmp");
    std::fs::write(&temp, text).map_err(|source| SettingsError::Write {
        path: temp.clone(),
        source,
    })?;
    std::fs::rename(&temp, path).map_err(|source| SettingsError::Write {
        path: path.to_path_buf(),
        source,
    })
}

/// Write the settings out if there is no file yet.
///
/// Called once at startup, so the file exists from the first launch rather than
/// after the first save. That matters more than it sounds: a streamer who is
/// told "edit your settings file" should find one, and a settings panel whose
/// first action is "create the file I am about to save into" has a window where
/// a crash loses the defaults they never got to see.
///
/// Never overwrites. An existing file that failed to parse is left exactly as it
/// is — the startup warning is the whole point of it — so this only ever runs
/// on a path that does not exist.
///
/// Returns `true` if a file was created.
pub fn create_if_missing(path: &Path) -> Result<bool, SettingsError> {
    if path.exists() {
        return Ok(false);
    }

    // Repaired, not raw. `Settings::default()` names a model most people do not
    // have, and writing that to disk creates a file that is wrong from the
    // moment it is born: every launch would then load it, fail to find it,
    // substitute a working one, and show a warning about a model the streamer
    // never asked for — a warning that can never be cleared except by opening
    // Settings and hitting Save.
    let (settings, _) = repair_model_path_reporting(Settings::default(), &discover_models());

    save(path, &settings)?;
    log::info!(
        "settings: no file yet, wrote the defaults to {}",
        path.display()
    );
    Ok(true)
}

/// The config directory this app uses.
pub fn config_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "yap-a-thon")
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The directory holding the running executable.
///
/// `None` when the platform will not say, which is not worth failing over:
/// the config directory is still a perfectly good place to keep settings.
pub fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// Every place the settings file is looked for, most specific first.
///
/// Two candidates, because two installation styles are real and the second one
/// is invisible if only the first is supported:
///
/// 1. **Next to the executable.** A portable install — the binary and its
///    settings travel together, so moving the folder moves the configuration.
/// 2. **The platform config directory.** `~/.config/yap-a-thon` on Linux, the
///    normal place for a program that was installed rather than dropped
///    somewhere.
///
/// The executable directory is tried first because a settings file sitting
/// next to a binary is a deliberate act: nobody lands one there by accident.
/// Getting the order backwards would mean a portable install silently ran on
/// the machine-wide defaults instead of its own configuration.
///
/// The two are never merged. Whichever exists first is the settings, in full,
/// including any values it does not mention — so there is one file in effect
/// and it is the one that gets written back. Merging would mean a streamer
/// editing half their settings and not knowing which half took.
pub fn settings_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(dir) = exe_dir() {
        candidates.push(dir.join(SETTINGS_FILE));
    }

    // Guarded against being the same file twice: a binary run from its own
    // config directory would otherwise list one path twice and log it twice.
    let in_config = config_dir().join(SETTINGS_FILE);
    if !candidates.contains(&in_config) {
        candidates.push(in_config);
    }

    candidates
}

/// The settings file name.
pub const SETTINGS_FILE: &str = "settings.toml";

/// Where the settings file actually is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsSource {
    /// The file to read from and write to.
    pub path: PathBuf,
    /// Whether it was there already.
    ///
    /// False means the app is about to create it, which is worth saying out
    /// loud: a streamer who edits a settings file and sees no change needs to
    /// know they edited the wrong one, or a different one from the one that
    /// exists.
    pub existed: bool,
}

/// Resolve the settings file from the candidates in [`settings_candidates`].
///
/// Split out from [`locate_settings`] so the precedence can be tested against
/// a directory that does not depend on where the test binary happens to be.
pub fn locate_settings_in(candidates: &[PathBuf]) -> SettingsSource {
    match candidates.iter().find(|path| path.is_file()) {
        Some(path) => SettingsSource {
            path: path.clone(),
            existed: true,
        },
        // Nothing found: fall back to the config directory, which is the one
        // that exists on every machine and is the right place to create a file
        // that did not exist a moment ago.
        None => SettingsSource {
            path: config_dir().join(SETTINGS_FILE),
            existed: false,
        },
    }
}

/// Resolve the settings file for this run.
///
/// The result is always logged with both the chosen file and what else was
/// considered, because "the app ignored my settings file" is otherwise
/// indistinguishable from "the app ignored my settings".
pub fn locate_settings() -> SettingsSource {
    let candidates = settings_candidates();
    let source = locate_settings_in(&candidates);

    if source.existed {
        log::info!("settings: reading {}", source.path.display());
    } else {
        log::info!(
            "settings: none found (looked in {}); starting from defaults and using {} \
             for anything saved",
            candidates
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            source.path.display(),
        );
    }

    if candidates.len() > 1 {
        let ignored: Vec<String> = candidates
            .iter()
            .filter(|path| **path != source.path)
            .map(|path| path.display().to_string())
            .collect();

        if !ignored.is_empty() {
            log::info!(
                "settings: ignoring {} while {} is in use",
                ignored.join(", "),
                source.path.display(),
            );
        }
    }

    source
}

/// Where finished sessions and their transcripts are kept.
pub fn data_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "yap-a-thon")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Failures from reading or writing the settings file.
#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot parse {path}: {message}")]
    Parse { path: PathBuf, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway directory, removed when the test ends.
    ///
    /// `TempDir` rather than a fixed name under `/tmp`: two test binaries
    /// running at once would otherwise read each other's "missing" file and
    /// pass or fail for reasons that have nothing to do with the code.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "yap-a-thon-settings-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id(),
            ));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).expect("temp dir");
            Self(base)
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }

        fn write(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.file(name);
            std::fs::write(&path, contents).expect("write settings");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_settings_file_beside_the_binary_wins_over_the_config_directory() {
        // The whole reason for the exe directory coming first: a portable
        // install ships its settings with the binary, and the reverse order
        // would run it on the machine-wide settings with no visible sign.
        let dir = TempDir::new("precedence-both");
        let portable = dir.write("portable.toml", "portable = true");
        let installed = dir.write("installed.toml", "installed = true");

        let found = locate_settings_in(&[portable.clone(), installed]);
        assert_eq!(found.path, portable);
        assert!(found.existed);
    }

    #[test]
    fn the_config_directory_is_used_when_the_binary_has_none() {
        let dir = TempDir::new("precedence-one");
        let portable = dir.file("portable.toml");
        let installed = dir.write("installed.toml", "installed = true");

        let found = locate_settings_in(&[portable, installed.clone()]);
        assert_eq!(found.path, installed);
        assert!(found.existed);
    }

    #[test]
    fn the_first_file_is_the_whole_settings_and_never_merged() {
        // The portable file is silent about the installed file's values, and
        // those must not bleed in. Merging would mean editing half your
        // settings and not knowing which half took effect.
        let dir = TempDir::new("no-merge");
        // Only the ignored file mentions the port, so picking it up would be
        // merging two files.
        let portable = dir.write("portable.toml", "keep_transcript = false");
        let installed = dir.write("installed.toml", "port = 1234");

        let found = locate_settings_in(&[portable, installed]);
        let loaded = load_raw(&found.path).expect("load");

        assert!(!loaded.keep_transcript, "the chosen file's value is used");
        assert_eq!(
            loaded.port,
            Settings::default().port,
            "a value only the ignored file mentions must not bleed in",
        );
    }

    #[test]
    fn no_file_anywhere_falls_back_to_the_config_directory() {
        let dir = TempDir::new("none");
        let source = locate_settings_in(&[dir.file("portable.toml"), dir.file("installed.toml")]);

        assert!(
            !source.existed,
            "nothing was there, and that must be reported"
        );
        assert_eq!(source.path, config_dir().join(SETTINGS_FILE));
    }

    #[test]
    fn a_directory_named_like_the_settings_file_is_not_a_settings_file() {
        // `is_file` rather than `exists`: a directory at that path means
        // something else went wrong, and parsing it would report a confusing
        // "cannot parse" for what is really a layout problem.
        let dir = TempDir::new("is-a-dir");
        let decoy = dir.file("settings.toml");
        std::fs::create_dir_all(&decoy).expect("mkdir");
        let real = dir.write("real.toml", "");

        let found = locate_settings_in(&[decoy, real.clone()]);
        assert_eq!(found.path, real);
    }

    #[test]
    fn the_candidate_list_never_names_one_file_twice() {
        // A binary run out of its own config directory would otherwise list the
        // same path twice, and the startup log would claim it considered it
        // twice.
        let candidates = settings_candidates();
        let mut unique = candidates.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), candidates.len());
    }

    #[test]
    fn the_example_settings_file_parses_and_matches_the_defaults() {
        // `settings.example.toml` is the only place every key is written down,
        // and it is hand-maintained. A hand-maintained list of keys rots: someone
        // renames a field and the example keeps documenting the old name, which
        // is worse than no example at all because it still looks authoritative.
        //
        // So it is parsed and compared against the real defaults rather than
        // believed. It also had `[voice]` and `[counting]` sections the app never
        // wrote, which this is what caught.
        let example: PathBuf = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("settings.example.toml");

        let text = std::fs::read_to_string(&example)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", example.display()));

        let parsed: Settings = toml::from_str(&text)
            .unwrap_or_else(|error| panic!("settings.example.toml does not parse: {error}"));

        let defaults = Settings::default();

        assert_eq!(parsed.port, defaults.port, "port");
        assert_eq!(parsed.bind_address, defaults.bind_address, "bind address");
        assert_eq!(parsed.input_device, defaults.input_device, "microphone");
        assert_eq!(
            parsed.default_hard_limit, defaults.default_hard_limit,
            "hard limit"
        );
        assert_eq!(
            parsed.keep_transcript, defaults.keep_transcript,
            "transcript"
        );
        assert_eq!(parsed.rules, defaults.rules, "counting rules");
        assert_eq!(parsed.vad, defaults.vad, "voice activity detection");
        assert_eq!(parsed.event_words, defaults.event_words, "event pricing");
    }

    #[test]
    fn the_settings_file_exists_after_the_first_launch() {
        let dir = TempDir::new("create");
        let path = dir.file("settings.toml");

        assert!(!path.exists());
        assert!(create_if_missing(&path).expect("create"));
        assert!(path.is_file());

        // And it is readable as settings, rather than being a file that only
        // exists to be looked at.
        assert!(load_raw(&path).is_ok());
    }

    #[test]
    fn creating_the_file_never_overwrites_one_that_is_there() {
        let dir = TempDir::new("create-existing");
        let path = dir.write("settings.toml", "port = 4321");

        assert!(!create_if_missing(&path).expect("create"));
        assert_eq!(load_raw(&path).expect("load").port, 4321);
    }

    #[test]
    fn a_malformed_file_is_never_replaced_with_the_defaults() {
        // Overwriting here would turn "your file has a typo, here it is" into
        // "your settings are gone" with no way back.
        let dir = TempDir::new("create-malformed");
        let path = dir.write("settings.toml", "this is not toml =");

        assert!(!create_if_missing(&path).expect("create"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "this is not toml =",
            "the file must be left exactly as it was found",
        );
    }

    #[test]
    fn defaults_are_private_and_local() {
        let s = Settings::default();
        assert_eq!(s.bind_address, "127.0.0.1");
        assert!(!s.engine.is_remote());
        assert!(s.keep_transcript);
    }

    #[test]
    fn default_backend_probes_before_falling_back() {
        let s = Settings::default();
        match &s.engine {
            EngineChoice::Local { backend, .. } => assert_eq!(*backend, LocalBackend::Auto),
            other => panic!("expected local, got {other:?}"),
        }
    }

    #[test]
    fn events_grant_nothing_without_a_configured_price() {
        let s = Settings::default();
        assert_eq!(s.words_for_event("twitch", "sub"), Some(100));
        // Not priced, so not granted.
        assert_eq!(s.words_for_event("twitch", "follow"), None);
        assert_eq!(s.words_for_event("youtube", "sub"), None);
    }

    #[test]
    fn new_session_uses_the_configured_goal() {
        let s = Settings::default();
        let config = s.new_session_config();
        assert_eq!(config.mode, GoalMode::Extend { base_words: 5000 });
        assert_eq!(config.hard_limit.map(|l| l.max), Some(10_000));
    }

    #[test]
    fn a_zero_port_falls_back_rather_than_binding_a_privileged_one() {
        let s = Settings {
            port: 0,
            ..Settings::default()
        };
        assert_eq!(s.effective_port(), 8973);
    }

    #[test]
    fn round_trips_through_toml() {
        let original = Settings::default();
        let text = toml::to_string_pretty(&original).expect("serialise");
        let parsed: Settings = toml::from_str(&text).expect("deserialise");
        assert_eq!(original, parsed);
    }

    #[test]
    fn missing_file_yields_defaults() {
        // `load_raw`, not `load`: `load` repairs a missing model against
        // whatever is on this machine, which would make this test depend on the
        // developer's downloads directory.
        let path = Path::new("/nonexistent/yap-a-thon/settings.toml");
        assert_eq!(load_raw(path).expect("defaults"), Settings::default());
    }

    #[test]
    fn a_missing_model_is_replaced_with_one_that_exists() {
        // The "I downloaded a model and it says no model" case. Given a candidate
        // list rather than the real disk, so the test means the same thing
        // everywhere.
        let found = FoundModel {
            path: PathBuf::from("/models/ggml-small.en.bin"),
            size_bytes: 487_614_201,
        };

        let settings = repair_model_path_with(Settings::default(), std::slice::from_ref(&found));

        match settings.engine {
            EngineChoice::Local { model_path, .. } => {
                assert_eq!(model_path, found.path);
            }
            other => panic!("expected local, got {other:?}"),
        }
    }

    #[test]
    fn the_backend_survives_a_model_repair() {
        // The repair rewrites the engine, and dropping the backend while doing it
        // would silently move someone off their GPU onto CPU.
        let settings = Settings {
            engine: EngineChoice::Local {
                model_path: PathBuf::from("/models/does-not-exist.bin"),
                backend: LocalBackend::Cuda,
            },
            ..Settings::default()
        };

        let found = FoundModel {
            path: PathBuf::from("/models/ggml-small.en.bin"),
            size_bytes: 1,
        };

        let repaired = repair_model_path_with(settings, &[found]);
        match repaired.engine {
            EngineChoice::Local { backend, .. } => assert_eq!(backend, LocalBackend::Cuda),
            other => panic!("expected local, got {other:?}"),
        }
    }

    #[test]
    fn no_models_on_disk_leaves_the_settings_alone() {
        // Nothing to offer, so nothing is changed and the caller reports the
        // missing file rather than silently getting something else.
        let before = Settings::default();
        let after = repair_model_path_with(before.clone(), &[]);
        assert_eq!(after, before);
    }

    #[test]
    fn a_malformed_file_is_an_error_not_a_silent_reset() {
        let dir = std::env::temp_dir().join("yap-a-thon-settings-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("settings.toml");
        std::fs::write(&path, "port = \"not a number\"").expect("write");

        // A streamer who fat-fingers a value should see an error rather than
        // silently losing their event pricing.
        assert!(matches!(load(&path), Err(SettingsError::Parse { .. })));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_then_load_is_stable() {
        let dir = std::env::temp_dir().join("yap-a-thon-settings-roundtrip");
        let path = dir.join("settings.toml");

        let original = Settings {
            port: 9000,
            twitch_channel: Some("someone".to_string()),
            ..Settings::default()
        };
        save(&path, &original).expect("save");
        // `load_raw` for the identity assertion: `load` may repair the model path
        // against the machine's disk, which would make this fail on a developer
        // who has a model and pass on one who does not.
        let loaded = load_raw(&path).expect("load");

        assert_eq!(loaded.port, 9000);
        assert_eq!(loaded.twitch_channel.as_deref(), Some("someone"));
        assert_eq!(original, loaded);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn models_default_under_the_data_directory() {
        // The path a streamer is told to put a downloaded model in has to be the
        // one discovery actually looks at, or "I put it there and it still says
        // no model" is the result.
        let dir = default_model_path();
        let root = data_dir();
        assert!(
            dir.starts_with(&root),
            "{} is not under {}",
            dir.display(),
            root.display()
        );
        let models = models_dir();
        assert!(dir.starts_with(&models));
        assert!(dir.to_string_lossy().contains("ggml"));
    }

    #[test]
    fn discovery_finds_a_model_and_ignores_other_files() {
        let dir = std::env::temp_dir().join("yap-core-discover-models");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");

        std::fs::write(dir.join("ggml-small.en.bin"), vec![0u8; 2048]).expect("write");
        std::fs::write(dir.join("ggml-base.en.bin"), vec![0u8; 4096]).expect("write");
        // Neither of these is a model, and listing them would be noise.
        std::fs::write(dir.join("notes.txt"), b"hello").expect("write");
        std::fs::write(dir.join("ggml-not-a-model.txt"), b"hello").expect("write");

        let found = super::discover_models_in(&dir);

        assert_eq!(found.len(), 2, "found: {found:?}");
        // Largest first, so the most capable model is offered first.
        assert_eq!(found[0].name(), "ggml-base.en.bin");
        assert_eq!(found[1].name(), "ggml-small.en.bin");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discovery_does_not_offer_the_same_file_twice() {
        let dir = std::env::temp_dir().join("yap-core-discover-dupes");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("ggml-small.en.bin"), vec![0u8; 64]).expect("write");

        let found = super::discover_models_in(&dir);
        assert_eq!(found.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_directory_is_not_an_error() {
        // First run has no models directory at all, and that must not look like
        // something went wrong.
        assert!(super::discover_models_in(Path::new("/nonexistent/yap/models")).is_empty());
    }

    #[test]
    fn sizes_are_labelled_for_a_person() {
        let label = |bytes: u64| {
            FoundModel {
                path: PathBuf::from("/models/ggml-small.en.bin"),
                size_bytes: bytes,
            }
            .size_label()
        };

        // Exact per unit. A table missing a unit — kilobytes, once — put a 466 MB
        // model at "465 GB", which is wrong by three orders of magnitude and
        // makes the whole picker untrustworthy.
        assert_eq!(label(900), "900 B");
        assert_eq!(label(2_048), "2.0 KB");
        assert_eq!(label(487_614_201), "465.0 MB");
        assert_eq!(label(1_600_000_000), "1.5 GB");
    }

    #[test]
    fn a_real_model_size_reads_as_megabytes_not_gigabytes() {
        // The actual ggml-small.en.bin, which is what a first-time user has.
        let label = FoundModel {
            path: PathBuf::from("/models/ggml-small.en.bin"),
            size_bytes: 487_614_201,
        }
        .size_label();

        assert_eq!(label, "465.0 MB");
    }

    #[test]
    fn an_existing_model_path_is_never_overridden() {
        // The streamer chose it and may be pointing at something deliberately —
        // another drive, a symlink. Only a path that does not exist is repaired.
        let existing = std::env::temp_dir().join("yap-settings-existing-model.bin");
        std::fs::write(&existing, b"not really a model").expect("write");

        let settings = Settings {
            engine: EngineChoice::Local {
                model_path: existing.clone(),
                backend: LocalBackend::Cpu,
            },
            ..Settings::default()
        };

        let repaired = repair_model_path(settings);
        match repaired.engine {
            EngineChoice::Local { model_path, .. } => {
                assert_eq!(model_path, existing, "an existing path must be kept");
            }
            other => panic!("expected local, got {other:?}"),
        }

        let _ = std::fs::remove_file(&existing);
    }

    #[test]
    fn a_cloud_engine_is_left_alone() {
        // There is no local model to repair, and pointing a cloud engine at a
        // ggml file would be nonsense.
        let settings = Settings {
            engine: EngineChoice::Groq {
                credential: "yap-a-thon/groq".to_string(),
            },
            ..Settings::default()
        };

        let repaired = repair_model_path(settings);
        assert!(matches!(repaired.engine, EngineChoice::Groq { .. }));
    }

    #[test]
    fn both_engines_report_the_same_model() {
        let local = EngineChoice::Local {
            model_path: PathBuf::from("m.bin"),
            backend: LocalBackend::Cpu,
        };
        let cloud = EngineChoice::Groq {
            credential: "k".to_string(),
        };
        // Same model on both sides keeps the counts comparable when a streamer
        // switches backends mid-project.
        assert_eq!(local.model_name(), cloud.model_name());
        assert!(!local.is_remote());
        assert!(cloud.is_remote());
        assert_eq!(cloud.cost_per_hour_usd(), Some(0.04));
        assert_eq!(local.cost_per_hour_usd(), None);
    }
}
