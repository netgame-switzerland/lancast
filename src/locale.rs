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
            Globalization::{DATE_SHORTDATE, GetDateFormatEx},
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
        let mut buffer = [0u16; 80];
        // SAFETY: `date` and `buffer` outlive the call; the null locale name
        // is LOCALE_NAME_USER_DEFAULT and the null format and calendar mean
        // the locale's defaults (GetDateFormatEx documentation).
        let len = unsafe {
            GetDateFormatEx(
                PCWSTR::null(),
                DATE_SHORTDATE,
                Some(&date),
                PCWSTR::null(),
                Some(&mut buffer),
                PCWSTR::null(),
            )
        };
        // The length includes the terminating NUL; 0 means failure.
        let len = usize::try_from(len).ok()?.checked_sub(1)?;
        Some(String::from_utf16_lossy(&buffer[..len.min(buffer.len())]))
    }
}

/// Other platforms keep `YYYY-MM-DD` (D32).
#[cfg(not(windows))]
mod backend {
    pub fn short_date(_year: u16, _month: u16, _day: u16) -> Option<String> {
        None
    }
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
}
