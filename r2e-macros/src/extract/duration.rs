//! Pure duration-string parser (`"5m"`, `"1h30m"`, `"500ms"` → milliseconds).
//!
//! Kept in its own self-contained file so the integration tests under `tests/`
//! can pull it in via `#[path = "../src/extract/duration.rs"]` — proc-macro
//! crates cannot expose ordinary `pub fn` items to external tests.

/// Parse a duration string like "5s", "2m", "1h30m", "500ms", "1m30s" into milliseconds.
///
/// Supported suffixes: `ms`, `s`, `m`, `h`, `d`.
/// Multiple segments can be combined: `"1h30m"`, `"2m30s"`, `"1h15m30s"`.
pub fn parse_duration_ms(input: &str) -> Result<u64, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("empty duration string".to_string());
    }

    let mut total_ms: u64 = 0;
    let mut current_num = String::new();
    let mut chars = s.chars().peekable();
    let mut found_any = false;

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
                _ => return Err(format!("unknown duration suffix '{}' — use ms, s, m, h, or d", suffix)),
            };

            total_ms += num * multiplier;
            current_num.clear();
            found_any = true;
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

    if !found_any {
        return Err("no duration segments found".to_string());
    }

    if total_ms == 0 {
        return Err("duration must be greater than zero".to_string());
    }

    Ok(total_ms)
}
