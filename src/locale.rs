//! Dates in the user's own format (D45): the short date format chosen in
//! the Windows region settings, e.g. `05.11.2026` for German (Switzerland).
//! Stored and signed dates stay `YYYY-MM-DD`; only what is shown changes.

/// `YYYY-MM-DD` in the user's short date format. Anything else, or a date
/// Windows cannot format, is returned unchanged.
pub fn short_date(iso: &str) -> String {
    parse(iso)
        .and_then(|(y, m, d)| backend::short_date(y, m, d))
        .unwrap_or_else(|| iso.to_owned())
}

/// Today's local date in the user's short date format; empty where that
/// is not available.
pub fn today() -> String {
    backend::today()
}

/// The current local time in the user's time format, without seconds
/// (e.g. `14:32`), for "checked at" in the settings (D50).
pub fn time_now() -> String {
    backend::time_now()
}

fn parse(iso: &str) -> Option<(u16, u16, u16)> {
    let mut parts = iso.trim().splitn(3, '-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    ((1..=12).contains(&month) && (1..=31).contains(&day)).then_some((year, month, day))
}

#[cfg(windows)]
mod backend {
    use windows::{
        Win32::{
            Foundation::SYSTEMTIME,
            Globalization::{DATE_SHORTDATE, GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS},
        },
        core::PCWSTR,
    };

    pub fn short_date(year: u16, month: u16, day: u16) -> Option<String> {
        let date = SYSTEMTIME {
            wYear: year,
            wMonth: month,
            wDay: day,
            ..Default::default()
        };
        format_date(Some(&date))
    }

    pub fn today() -> String {
        format_date(None).unwrap_or_default()
    }

    /// `None` = the current local date (GetDateFormatEx documentation).
    fn format_date(date: Option<&SYSTEMTIME>) -> Option<String> {
        let mut buffer = [0u16; 80];
        // SAFETY: `date` and `buffer` outlive the call; the null locale name
        // is LOCALE_NAME_USER_DEFAULT, a null date the current local date, and
        // the null format and calendar mean the locale's defaults
        // (GetDateFormatEx documentation).
        let len = unsafe {
            GetDateFormatEx(
                PCWSTR::null(),
                DATE_SHORTDATE,
                date.map(|d| d as *const SYSTEMTIME),
                PCWSTR::null(),
                Some(&mut buffer),
                PCWSTR::null(),
            )
        };
        // The length includes the terminating NUL; 0 means failure.
        let len = usize::try_from(len).ok()?.checked_sub(1)?;
        Some(String::from_utf16_lossy(&buffer[..len.min(buffer.len())]))
    }

    pub fn time_now() -> String {
        let mut buffer = [0u16; 80];
        // SAFETY: `buffer` outlives the call; the null locale name is
        // LOCALE_NAME_USER_DEFAULT, the null time the current local time and
        // the null format the locale's own (GetTimeFormatEx documentation).
        let len = unsafe {
            GetTimeFormatEx(
                PCWSTR::null(),
                TIME_NOSECONDS,
                None,
                PCWSTR::null(),
                Some(&mut buffer),
            )
        };
        // 0 means failure; the count includes the terminating NUL.
        let len = usize::try_from(len).unwrap_or(0).min(buffer.len());
        let text = &buffer[..len];
        let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
        if end == 0 {
            return super::utc_time_now();
        }
        String::from_utf16_lossy(&text[..end])
    }
}

/// Other platforms keep `YYYY-MM-DD` (D32).
#[cfg(not(windows))]
mod backend {
    pub fn short_date(_year: u16, _month: u16, _day: u16) -> Option<String> {
        None
    }

    pub fn today() -> String {
        String::new()
    }

    pub fn time_now() -> String {
        super::utc_time_now()
    }
}

/// `HH:MM UTC`, when the user's format is not available.
fn utc_time_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let minutes = secs / 60 % (24 * 60);
    format!("{:02}:{:02} UTC", minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_dates_and_passes_other_text_through() {
        let shown = short_date("2026-11-05");
        assert!(shown.contains("26") && shown.contains('5'), "{shown}");
        assert_eq!(short_date("someday"), "someday");
        assert_eq!(short_date("2026-13-01"), "2026-13-01");
    }

    #[test]
    fn formats_the_current_time() {
        let time = time_now();
        assert!(time.chars().any(|c| c.is_ascii_digit()), "{time}");
        #[cfg(windows)]
        assert!(!today().is_empty());
    }
}
