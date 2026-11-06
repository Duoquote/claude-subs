//! Reset times as the tray and popover show them: the clock follows the Windows short-time setting
//! (12- or 24-hour), the next two days read "today" / "tomorrow", and a close reset is a countdown.
//!
//! * under 6 h: `in 42m`, `in 3h 05m` (a session window is at most 5 h)
//! * today / tomorrow: `today 18:30`, `tomorrow 11:00`
//! * this week: `Sat 11:00`; further: `12 Nov 11:00`
//!
//! The exact form (for hover) is `today 14:30`, `tomorrow 11:00`, `Sat 7 Nov, 11:00`.

use chrono::{DateTime, Local, TimeZone};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clock {
    pub h24: bool,
    /// Two-digit hours (`HH` / `hh`).
    pub pad: bool,
}

impl Default for Clock {
    fn default() -> Self {
        Self { h24: true, pad: true }
    }
}

impl Clock {
    /// The user's Windows setting (Region → short time), read each time: a change applies at once.
    pub fn system() -> Self {
        short_time_pattern().map_or_else(Self::default, |p| Self::from_pattern(&p))
    }

    /// From a Windows time pattern: `H`/`HH` is 24-hour, `h`/`hh` 12-hour; quoted text is literal.
    pub fn from_pattern(pattern: &str) -> Self {
        let mut quoted = false;
        let mut chars = pattern.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\'' => quoted = !quoted,
                'H' | 'h' if !quoted => {
                    let pad = chars.peek() == Some(&c);
                    return Self { h24: c == 'H', pad };
                }
                _ => {}
            }
        }
        Self::default()
    }

    pub fn time<Tz: TimeZone>(&self, t: &DateTime<Tz>) -> String
    where
        Tz::Offset: std::fmt::Display,
    {
        let pattern = match (self.h24, self.pad) {
            (true, true) => "%H:%M",
            (true, false) => "%-H:%M",
            (false, true) => "%I:%M %p",
            (false, false) => "%-I:%M %p",
        };
        t.format(pattern).to_string()
    }
}

#[cfg(windows)]
fn short_time_pattern() -> Option<String> {
    use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_SSHORTTIME};
    use windows::core::PCWSTR;
    let mut buf = [0u16; 80];
    // SAFETY: a null locale name is the user default; `buf` is a valid output buffer.
    let len = unsafe { GetLocaleInfoEx(PCWSTR::null(), LOCALE_SSHORTTIME, Some(&mut buf)) };
    let len = usize::try_from(len).ok().filter(|&n| n > 0)?;
    Some(String::from_utf16_lossy(&buf[..len - 1]))
}

#[cfg(not(windows))]
fn short_time_pattern() -> Option<String> {
    None
}

/// Local time, rounded to the nearest minute: the API answers resets like `07:59:59.9Z`, which is
/// the 08:00 reset.
fn local(ms: i64) -> Option<DateTime<Local>> {
    let rounded = (ms + 30_000).div_euclid(60_000) * 60_000;
    Local.timestamp_millis_opt(rounded).single()
}

fn day_form(at: &DateTime<Local>, now: &DateTime<Local>, clock: Clock, exact: bool) -> String {
    let time = clock.time(at);
    match (at.date_naive() - now.date_naive()).num_days() {
        ..=0 => format!("today {time}"),
        1 => format!("tomorrow {time}"),
        2..=6 if exact => format!("{}, {time}", at.format("%a %-d %b")),
        2..=6 => format!("{} {time}", at.format("%a")),
        _ if exact => format!("{}, {time}", at.format("%a %-d %b")),
        _ => format!("{} {time}", at.format("%-d %b")),
    }
}

/// The short form shown beside a meter and in the tooltip.
pub fn reset_label(at_ms: i64, now_ms: i64, clock: Clock) -> String {
    let mins = (at_ms - now_ms).max(0) / 60_000;
    if mins < 60 {
        return format!("in {mins}m");
    }
    if mins < 6 * 60 {
        return format!("in {}h {:02}m", mins / 60, mins % 60);
    }
    match (local(at_ms), local(now_ms)) {
        (Some(at), Some(now)) => day_form(&at, &now, clock, false),
        _ => String::new(),
    }
}

/// The exact form, for hover: always the clock time, with the day.
pub fn reset_exact(at_ms: i64, now_ms: i64, clock: Clock) -> String {
    match (local(at_ms), local(now_ms)) {
        (Some(at), Some(now)) => day_form(&at, &now, clock, true),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H24: Clock = Clock { h24: true, pad: true };
    const H12: Clock = Clock { h24: false, pad: false };

    fn ms(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap().timestamp_millis()
    }

    #[test]
    fn windows_patterns() {
        assert_eq!(Clock::from_pattern("HH:mm"), Clock { h24: true, pad: true });
        assert_eq!(Clock::from_pattern("H:mm"), Clock { h24: true, pad: false });
        assert_eq!(Clock::from_pattern("h:mm tt"), Clock { h24: false, pad: false });
        assert_eq!(Clock::from_pattern("tt hh:mm"), Clock { h24: false, pad: true });
        assert_eq!(Clock::from_pattern("'h'H.mm"), Clock { h24: true, pad: false }, "quoted text is skipped");
    }

    #[test]
    fn labels_by_distance() {
        // Wednesday 4 Nov 2026, 10:00 local.
        let now = ms(2026, 11, 4, 10, 0);
        assert_eq!(reset_label(now + 42 * 60_000, now, H24), "in 42m");
        assert_eq!(reset_label(now + 185 * 60_000, now, H24), "in 3h 05m");
        assert_eq!(reset_label(ms(2026, 11, 4, 18, 30), now, H24), "today 18:30");
        assert_eq!(reset_label(ms(2026, 11, 5, 11, 0), now, H24), "tomorrow 11:00");
        assert_eq!(reset_label(ms(2026, 11, 7, 11, 0), now, H24), "Sat 11:00");
        assert_eq!(reset_label(ms(2026, 11, 12, 9, 5), now, H24), "12 Nov 09:05");
        assert_eq!(reset_label(ms(2026, 11, 5, 23, 0), now, H12), "tomorrow 11:00 PM");
        // A close reset still shows its exact time on hover.
        assert_eq!(reset_exact(now + 185 * 60_000, now, H24), "today 13:05");
        assert_eq!(reset_exact(ms(2026, 11, 5, 2, 0), now, H24), "tomorrow 02:00");
        assert_eq!(reset_exact(ms(2026, 11, 7, 11, 0), now, H12), "Sat 7 Nov, 11:00 AM");
    }

    #[test]
    fn a_reset_just_before_the_hour_reads_as_the_hour() {
        let now = ms(2026, 11, 4, 10, 0);
        let almost = ms(2026, 11, 7, 11, 0) - 100;
        assert_eq!(reset_label(almost, now, H24), "Sat 11:00");
        assert_eq!(reset_label(ms(2026, 11, 4, 23, 59) + 45_000, now, H24), "tomorrow 00:00");
    }
}
