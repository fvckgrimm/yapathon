//! Choosing a compute backend for whisper.cpp.
//!
//! Kept separate from [`crate::local`] so the probe can be reasoned about —
//! and tested — without loading a model.

use serde::{Deserialize, Serialize};

/// What the streamer asked for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendChoice {
    /// Metal on Apple silicon, then CUDA, then Vulkan, then CPU.
    #[default]
    Auto,
    Metal,
    Cuda,
    Vulkan,
    Cpu,
}

impl BackendChoice {
    /// What `Auto` settled on for this build.
    ///
    /// The probe is compile-time rather than runtime because whisper.cpp's
    /// backends are selected when the library is compiled: asking for CUDA in a
    /// binary built without it does not fall back, it produces no output. So
    /// `Auto` picks the best backend this binary can actually use, and CPU is
    /// always available as the floor.
    pub fn resolve(self) -> DetectedBackend {
        match self {
            Self::Metal => DetectedBackend::Metal,
            Self::Cuda => DetectedBackend::Cuda,
            Self::Vulkan => DetectedBackend::Vulkan,
            Self::Cpu => DetectedBackend::Cpu,
            Self::Auto => DetectedBackend::for_build(),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Metal => "Metal",
            Self::Cuda => "CUDA",
            Self::Vulkan => "Vulkan",
            Self::Cpu => "CPU",
        }
    }
}

/// A backend that is actually in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedBackend {
    Metal,
    Cuda,
    Vulkan,
    Cpu,
}

impl DetectedBackend {
    /// The best backend this build supports.
    ///
    /// Mirrors how the binary is compiled. A build with the `metal` feature on
    /// Apple silicon reports Metal; one with `cuda` reports CUDA; everything
    /// else falls through to CPU, which always works and is fast enough for
    /// short silence-delimited utterances.
    pub fn for_build() -> Self {
        #[cfg(all(target_os = "macos", feature = "whisper-metal"))]
        {
            Self::Metal
        }

        #[cfg(all(not(target_os = "macos"), feature = "whisper-cuda"))]
        {
            Self::Cuda
        }

        #[cfg(all(
            not(target_os = "macos"),
            not(feature = "whisper-cuda"),
            feature = "whisper-vulkan"
        ))]
        {
            Self::Vulkan
        }

        #[cfg(all(
            not(target_os = "macos"),
            not(feature = "whisper-cuda"),
            not(feature = "whisper-vulkan")
        ))]
        {
            Self::Cpu
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Metal => "Metal",
            Self::Cuda => "CUDA",
            Self::Vulkan => "Vulkan",
            Self::Cpu => "CPU",
        }
    }

    /// Whether this backend runs on the GPU.
    pub fn is_gpu(self) -> bool {
        !matches!(self, Self::Cpu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_choices_are_respected() {
        assert_eq!(BackendChoice::Cpu.resolve(), DetectedBackend::Cpu);
        assert_eq!(BackendChoice::Cuda.resolve(), DetectedBackend::Cuda);
        assert_eq!(BackendChoice::Metal.resolve(), DetectedBackend::Metal);
    }

    #[test]
    fn auto_resolves_to_something_that_works() {
        // Whatever Auto picks must be usable, or first run silently produces no
        // transcripts.
        let resolved = BackendChoice::Auto.resolve();
        assert!(matches!(
            resolved,
            DetectedBackend::Metal
                | DetectedBackend::Cuda
                | DetectedBackend::Vulkan
                | DetectedBackend::Cpu
        ));
    }

    #[test]
    fn auto_on_a_plain_build_is_cpu() {
        // This crate is built without any GPU feature, so Auto cannot claim
        // Metal or CUDA: whisper.cpp would produce nothing.
        assert_eq!(BackendChoice::Auto.resolve(), DetectedBackend::Cpu);
    }

    #[test]
    fn gpu_backends_are_labelled_as_such() {
        assert!(!DetectedBackend::Cpu.is_gpu());
        assert!(DetectedBackend::Metal.is_gpu());
        assert!(DetectedBackend::Cuda.is_gpu());
    }
}
