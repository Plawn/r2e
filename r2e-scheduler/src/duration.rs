//! Runtime duration-string parser (`"5m"`, `"1h30m"`, `"500ms"` → [`PositiveDuration`]).
//!
//! Twin of the compile-time parser in `r2e-macros/src/extract/duration.rs`
//! (proc-macro crates cannot export ordinary functions). Keep the grammars in
//! sync: suffixes `ms`, `s`, `m`, `h`, `d`; segments can be combined.

use std::time::Duration;

/// A [`Duration`] guaranteed to be strictly greater than zero.
///
/// Scheduler intervals must be positive — a zero interval would busy-loop the
/// driver. Encoding the invariant in the type removes defensive zero-checks at
/// every call site: once you hold a `PositiveDuration`, it is non-zero *by
/// construction*, and an illegal (zero) interval is simply unrepresentable.
///
/// `PositiveDuration` derefs to the underlying [`Duration`], so all `Duration`
/// accessors (`as_secs`, `as_millis`, …) are available directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PositiveDuration(Duration);

impl PositiveDuration {
    /// Wrap a [`Duration`], returning `None` if it is zero.
    pub const fn new(d: Duration) -> Option<Self> {
        if d.is_zero() {
            None
        } else {
            Some(Self(d))
        }
    }

    /// Construct from milliseconds, returning `None` if `ms == 0`.
    pub const fn from_millis(ms: u64) -> Option<Self> {
        Self::new(Duration::from_millis(ms))
    }

    /// Construct from whole seconds, returning `None` if `secs == 0`.
    pub const fn from_secs(secs: u64) -> Option<Self> {
        Self::new(Duration::from_secs(secs))
    }

    /// The underlying [`Duration`] (always non-zero).
    pub const fn get(self) -> Duration {
        self.0
    }
}

impl std::ops::Deref for PositiveDuration {
    type Target = Duration;

    fn deref(&self) -> &Duration {
        &self.0
    }
}

impl std::fmt::Display for PositiveDuration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

impl From<PositiveDuration> for Duration {
    fn from(p: PositiveDuration) -> Duration {
        p.0
    }
}

/// Parse a duration string like `"5s"`, `"2m"`, `"1h30m"`, `"500ms"` into a
/// [`PositiveDuration`].
///
/// Supported suffixes: `ms`, `s`, `m`, `h`, `d`.
/// Multiple segments can be combined: `"1h30m"`, `"2m30s"`, `"1h15m30s"`.
///
/// A zero total (e.g. `"0s"`) is rejected — a schedule interval must be
/// positive (see [`PositiveDuration`]).
pub fn parse_duration(input: &str) -> Result<PositiveDuration, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("empty duration string".to_string());
    }

    let mut total_ms: u64 = 0;
    let mut current_num = String::new();
    let mut chars = s.chars().peekable();

    while let Some(&ch) = chars.peek() {
        if ch.is_ascii_digit() {
            current_num.push(ch);
            chars.next();
        } else if ch.is_ascii_alphabetic() {
            if current_num.is_empty() {
                return Err(format!("unexpected '{}' without a preceding number", ch));
            }

            let mut suffix = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_ascii_alphabetic() {
                    suffix.push(c);
                    chars.next();
                } else {
                    break;
                }
            }

            let num: u64 = current_num
                .parse()
                .map_err(|_| format!("invalid number: '{}'", current_num))?;

            let multiplier = match suffix.as_str() {
                "ms" => 1,
                "s" => 1_000,
                "m" => 60_000,
                "h" => 3_600_000,
                "d" => 86_400_000,
                _ => {
                    return Err(format!(
                        "unknown duration suffix '{}' — use ms, s, m, h, or d",
                        suffix
                    ))
                }
            };

            total_ms = num
                .checked_mul(multiplier)
                .and_then(|segment| total_ms.checked_add(segment))
                .ok_or_else(|| format!("duration too large: '{}' overflows", s))?;
            current_num.clear();
        } else {
            return Err(format!("unexpected character '{}' in duration", ch));
        }
    }

    if !current_num.is_empty() {
        return Err(format!(
            "trailing number '{}' without a unit suffix (ms, s, m, h, d)",
            current_num
        ));
    }

    // `found_any` is not tracked: any non-empty input either returns early
    // above or leaves a trailing number (rejected just above), so reaching here
    // means at least one segment was parsed. A zero total is the only remaining
    // invalid case, caught by the `PositiveDuration` constructor.
    PositiveDuration::from_millis(total_ms)
        .ok_or_else(|| "duration must be greater than zero".to_string())
}
