//! yap-a-thon: a yapathon counter for OBS.
//!
//! The app owns a microphone, a speech recogniser, a session's word count, and a
//! local server that OBS reads. Everything that decides whether a count is
//! honest lives in `yap-core`; this crate is the shell around it.
//!
//! GPUI is the desktop app rather than an OBS plugin on purpose. A native OBS
//! plugin is a per-platform build against Qt, and none of the counting needs
//! Qt. The app serves a dock and an overlay that OBS loads over loopback, so
//! one Rust codebase covers Windows, macOS and Linux with nothing OBS-specific
//! to build or sign.
//!
//! # Module map
//!
//! | Module | Responsibility |
//! | ------ | -------------- |
//! | [`runtime`] | The tokio runtime for audio, HTTP and recognition |
//! | [`hub`] | The seam between tokio tasks and the GPUI thread |
//! | [`model`] | The state entity the window renders from |
//! | [`view`] | Drawing the window |
//! | [`segment`] | The one exclusive-choice button every row of them uses |
//! | [`session`] | Commands from the UI to the shared state |
//! | [`app`] | Startup, the window, and command wiring */

pub mod app;
pub mod hub;
pub mod icons;
pub mod model;
pub mod runtime;
pub mod segment;
pub mod session;
pub mod settings;
pub mod settings_view;
pub mod theme;
pub mod view;

pub use app::{APP_ID, open_main_window, start_server};
pub use hub::Hub;
pub use model::{ModelMessage, SessionEntity, SessionModel, TranscriptLine};
pub use session::StartRequest;
pub use settings::{Field, FieldKind, SETTINGS_SECTIONS, Section};
pub use theme::Appearance;
