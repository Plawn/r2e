//! [`Layered<C>`] — the three configuration tiers of a plugin, merged in one
//! fixed order.

use super::ConfigProperties;

/// The code-side configuration tiers of a plugin, around the file section the
/// plugin contract hands to `build`:
///
/// **explicit (builder setter) > file section > preset > `C::default()`**
///
/// A plugin holds one `Layered<Config>` instead of a hand-copied `Option`
/// per knob, and never writes the `.or(file.x).or(preset.x)` chain itself:
///
/// ```ignore
/// pub struct FrameGuard {
///     cfg: Layered<FrameGuardConfig>,
/// }
///
/// impl FrameGuard {
///     pub fn new() -> Self { Self { cfg: Layered::new() } }
///
///     /// Shared-baseline defaults: the app's file still has the last word.
///     pub fn preset(config: FrameGuardConfig) -> Self {
///         Self { cfg: Layered::with_preset(config) }
///     }
///
///     /// "I mean exactly this": beats the file.
///     pub fn policy(mut self, policy: impl Into<String>) -> Self {
///         self.cfg.set(|c| c.policy = Some(policy.into()));
///         self
///     }
/// }
///
/// // in `Plugin::build(self, _deps, config, ctx)`:
/// let cfg = self.cfg.resolve(config);     // validate / convert *after* this
/// let policy = cfg.policy.unwrap_or_else(|| "DENY".into());
/// ```
///
/// `C` must be **layerable** — every field an `Option<T>` or a
/// `#[config(section)]` of a layerable type
/// ([`ConfigProperties::LAYERABLE`]). A plain `bool` field has no "unset", so
/// the explicit tier would always win over the file; [`resolve`](Self::resolve)
/// rejects such a `C` at compile time rather than ignoring a file key.
#[derive(Clone, Debug, Default)]
pub struct Layered<C> {
    explicit: C,
    preset: C,
}

impl<C: ConfigProperties + Default> Layered<C> {
    /// No explicit setting, no preset: the file section, then `C::default()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            explicit: C::default(),
            preset: C::default(),
        }
    }

    /// Start from a preset: values that sit **below** the file section.
    #[must_use]
    pub fn with_preset(preset: C) -> Self {
        Self {
            explicit: C::default(),
            preset,
        }
    }

    /// Start from explicit values: they sit **above** the file section.
    #[must_use]
    pub fn with_explicit(explicit: C) -> Self {
        Self {
            explicit,
            preset: C::default(),
        }
    }

    /// Mutate the explicit tier — what a builder setter does.
    pub fn set(&mut self, f: impl FnOnce(&mut C)) -> &mut Self {
        f(&mut self.explicit);
        self
    }

    /// Mutate the preset tier — a fallback the file can still override.
    pub fn set_preset(&mut self, f: impl FnOnce(&mut C)) -> &mut Self {
        f(&mut self.preset);
        self
    }

    /// Replace the preset tier wholesale.
    pub fn replace_preset(&mut self, preset: C) -> &mut Self {
        self.preset = preset;
        self
    }

    /// The explicit tier as set so far.
    pub fn explicit(&self) -> &C {
        &self.explicit
    }

    /// The preset tier as set so far.
    pub fn preset(&self) -> &C {
        &self.preset
    }

    /// Merge with the file section (`None` = section absent):
    /// explicit > `file` > preset. Fields no tier sets stay `None` — the
    /// plugin applies its built-in default and validates on the result.
    ///
    /// A `C` that is not [`LAYERABLE`](ConfigProperties::LAYERABLE) is a
    /// compile error here — post-monomorphization, so `cargo build` and
    /// `cargo test` report it while `cargo check` does not.
    #[must_use]
    pub fn resolve(self, file: Option<C>) -> C {
        const {
            assert!(
                C::LAYERABLE,
                "Layered<C>: every field of C must be an `Option<T>` or a `#[config(section)]` \
                 of a layerable type — a plain field has no \"unset\", so the builder tier would \
                 silently override the file"
            )
        };
        let merged = match file {
            Some(file) => self.explicit.overlay(file),
            None => self.explicit,
        };
        merged.overlay(self.preset)
    }
}
