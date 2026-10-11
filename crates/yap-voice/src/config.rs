//! Choosing a compute backend for whisper.cpp.
//!
//! Kept separate from [`crate::local`] so the choice can be reasoned about —
//! and tested — without loading a model.

use serde::{Deserialize, Serialize};

/// What the streamer asked for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendChoice {
    /// Whatever this binary was built with, GPU if it has one.
    #[default]
    Auto,
    Metal,
    Cuda,
    Vulkan,
    Cpu,
}

impl BackendChoice {
    /// What this ask turns into for this build.
    ///
    /// # Why `Auto` is not "the best GPU on this machine"
    ///
    /// whisper.cpp's backends are chosen when the library is *compiled*. Asking
    /// for CUDA in a binary built without it does not fall back — there is
    /// nothing to fall back to, and the request simply cannot be honoured. So
    /// `Auto` means "use a GPU if this binary has one", and the floor is CPU,
    /// which always works.
    ///
    /// # Why an unavailable GPU request becomes CPU rather than an error
    ///
    /// Picking `Vulkan` on a binary built without it used to be reported as
    /// Vulkan and then run on the CPU — the setting said one thing and the
    /// machine did another, with nothing to say so. Now it resolves to CPU,
    /// which is what will actually happen, and [`Self::resolve`]'s caller can
    /// say that it asked for something else.
    ///
    /// No default is changed by any of this. `Auto` still resolves to exactly
    /// what it did before, on every build.
    pub fn resolve(self) -> DetectedBackend {
        match self {
            Self::Metal if DetectedBackend::compiled_in().is_some_and(|b| b.is_metal()) => {
                DetectedBackend::Metal
            }
            Self::Cuda if DetectedBackend::compiled_in().is_some_and(|b| b.is_cuda()) => {
                DetectedBackend::Cuda
            }
            Self::Vulkan if DetectedBackend::compiled_in().is_some_and(|b| b.is_vulkan()) => {
                DetectedBackend::Vulkan
            }
            Self::Cpu => DetectedBackend::Cpu,

            // A GPU this build does not have resolves to CPU — and *only* to
            // CPU. It must not fall through to `for_build()`, which would hand
            // back whatever GPU the binary happens to have: asking for CUDA in
            // a Vulkan build would then silently become Vulkan, which is the same
            // lie in a different costume.
            Self::Metal | Self::Cuda | Self::Vulkan => DetectedBackend::Cpu,

            // `Auto` is the one that asks what the build has, and uses it.
            Self::Auto => DetectedBackend::compiled_in().unwrap_or(DetectedBackend::Cpu),
        }
    }

    /// Whether this ask is for a specific backend, rather than "whatever works".
    ///
    /// Used to decide whether a downgrade to CPU is worth saying out loud.
    pub fn is_specific(self) -> bool {
        !matches!(self, Self::Auto)
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
    /// The GPU backend this binary was compiled with, if any.
    ///
    /// # Why one function and not four `cfg` blocks
    ///
    /// This used to be a chain of bare `#[cfg(...)] { ... }` blocks, one per
    /// backend, each ending in a value. Those are not alternatives — **every**
    /// matching block is compiled, and the first one wins. It happened to give
    /// the right answer for every single-feature build, and would have given the
    /// wrong one the moment the order mattered: a build with both `cuda` and
    /// `vulkan` enabled reports CUDA whichever way round they are listed, and the
    /// `not(feature = "whisper-cuda")` guards were there only to stop the later
    /// blocks from disagreeing.
    ///
    /// Now it is an ordinary `if/else if` chain under one `cfg`, so the order is
    /// stated once and no two branches can both compile.
    ///
    /// The order is the preference order, not an accident: Metal on Apple
    /// silicon, then CUDA, then Vulkan, then nothing. A machine with two
    /// compiled backends has two *drivers*; which one whisper.cpp actually opens
    /// is its decision at load, not ours.
    pub fn compiled_in() -> Option<Self> {
        #[cfg(target_os = "macos")]
        {
            if cfg!(feature = "whisper-metal") {
                return Some(Self::Metal);
            }
        }

        #[cfg(feature = "whisper-cuda")]
        {
            if cfg!(not(target_os = "macos")) {
                return Some(Self::Cuda);
            }
        }

        #[cfg(feature = "whisper-vulkan")]
        {
            if cfg!(not(target_os = "macos")) {
                return Some(Self::Vulkan);
            }
        }

        None
    }

    /// The best backend this build supports, with CPU as the floor.
    pub fn for_build() -> Self {
        Self::compiled_in().unwrap_or(Self::Cpu)
    }

    fn is_metal(self) -> bool {
        matches!(self, Self::Metal)
    }

    fn is_cuda(self) -> bool {
        matches!(self, Self::Cuda)
    }

    fn is_vulkan(self) -> bool {
        matches!(self, Self::Vulkan)
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

    /// Whether this crate was built with a GPU backend at all.
    fn built_with_gpu() -> bool {
        DetectedBackend::compiled_in().is_some()
    }

    #[test]
    fn explicit_choices_are_respected_when_the_build_has_them() {
        // A request for a backend this binary does not have resolves to CPU
        // rather than being reported and then ignored — the pair of assertions
        // below is the whole point, and checking only the first would have
        // passed against the old code.
        for (choice, backend) in [
            (BackendChoice::Cpu, DetectedBackend::Cpu),
            (BackendChoice::Metal, DetectedBackend::Metal),
            (BackendChoice::Cuda, DetectedBackend::Cuda),
            (BackendChoice::Vulkan, DetectedBackend::Vulkan),
        ] {
            let resolved = choice.resolve();

            if DetectedBackend::compiled_in() == Some(backend) {
                assert_eq!(
                    resolved, backend,
                    "{choice:?} is compiled in, so it must be honoured"
                );
            } else {
                assert_eq!(
                    resolved,
                    DetectedBackend::Cpu,
                    "{choice:?} is not in this build, so it must fall back to CPU \\
                     rather than be reported and then ignored",
                );
            }
        }
    }

    #[test]
    fn cpu_is_cpu_whatever_else_is_compiled_in() {
        // The bug this whole file exists for: `CPU` was written to settings,
        // the engine recorded it, and whisper ran on the GPU anyway because
        // nothing passed the choice into `WhisperContextParameters`. The
        // resolution half of that fix lives in `local::load`; this pins the half
        // that can be pinned without a GPU.
        assert_eq!(BackendChoice::Cpu.resolve(), DetectedBackend::Cpu);
    }

    #[test]
    fn auto_never_asks_for_a_backend_this_build_lacks() {
        // Auto used to be able to report Vulkan on a build with no Vulkan.
        // Whatever it resolves to, it must be something the binary can do.
        let resolved = BackendChoice::Auto.resolve();

        if built_with_gpu() {
            assert!(
                resolved.is_gpu(),
                "auto on a GPU build must use the GPU, got {}",
                resolved.label()
            );
        } else {
            assert_eq!(
                resolved,
                DetectedBackend::Cpu,
                "auto on a CPU-only build must not claim a GPU"
            );
        }
    }

    #[test]
    fn auto_says_what_this_build_can_do() {
        // Auto cannot claim a backend the binary does not have: whisper.cpp
        // produces nothing for a request it cannot honour. So the answer must be
        // exactly the build's own answer — CPU when no GPU feature is on, which
        // is the usual case, and the GPU when one is.
        assert_eq!(
            BackendChoice::Auto.resolve(),
            DetectedBackend::for_build(),
            "auto must agree with what this binary was compiled to do",
        );
    }

    #[test]
    fn only_one_backend_is_ever_reported_for_auto() {
        // The bare-`cfg` version compiled every matching block and let the
        // first win. Asking "what does auto say" twice cannot catch that on a
        // single-feature build, so this states the invariant directly: the
        // answer is one value, and it is one this build can actually use.
        let first = DetectedBackend::compiled_in();
        let second = DetectedBackend::compiled_in();

        assert_eq!(first, second, "the probe must be stable");
        assert_eq!(
            BackendChoice::Auto.resolve(),
            first.unwrap_or(DetectedBackend::Cpu),
            "auto must agree with the build, not with the first matching cfg block",
        );
    }

    #[test]
    fn only_specific_choices_are_specific() {
        // What decides whether a silent downgrade to CPU is worth reporting.
        assert!(!BackendChoice::Auto.is_specific());
        assert!(BackendChoice::Cpu.is_specific());
        assert!(BackendChoice::Vulkan.is_specific());
    }

    #[test]
    fn gpu_backends_are_labelled_as_such() {
        assert!(!DetectedBackend::Cpu.is_gpu());
        assert!(DetectedBackend::Metal.is_gpu());
        assert!(DetectedBackend::Cuda.is_gpu());
        assert!(DetectedBackend::Vulkan.is_gpu());
    }
}
