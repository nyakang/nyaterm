//! Interactive Windows sessions through the Remote Desktop Services API.

use crate::stats::LoggedInSession;
use windows_sys::Win32::System::RemoteDesktop::{
    WTS_CURRENT_SERVER_HANDLE, WTS_SESSION_INFOW, WTSEnumerateSessionsW, WTSFreeMemory, WTSINFOW,
    WTSQuerySessionInformationW, WTSSessionInfo,
};

fn utf16_until_nul(units: &[u16]) -> String {
    let len = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..len])
}

/// `FILETIME` ticks (100 ns since 1601-01-01 UTC) as `YYYY-MM-DD HH:MM UTC`.
/// The zone is spelled out: no time-zone database is consulted here.
pub fn format_filetime(ticks: i64) -> String {
    if ticks <= 0 {
        return String::new();
    }
    const UNIX_EPOCH_AS_FILETIME_SECS: i64 = 11_644_473_600;
    let unix = ticks / 10_000_000 - UNIX_EPOCH_AS_FILETIME_SECS;
    let (days, secs) = (unix.div_euclid(86_400), unix.rem_euclid(86_400));
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        secs / 3600,
        secs % 3600 / 60
    )
}

pub fn sessions() -> anyhow::Result<Vec<LoggedInSession>> {
    let mut info: *mut WTS_SESSION_INFOW = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: out-pointers are valid; on success `info` holds `count` records
    // that must be released with WTSFreeMemory.
    let ok =
        unsafe { WTSEnumerateSessionsW(WTS_CURRENT_SERVER_HANDLE, 0, 1, &mut info, &mut count) };
    if ok == 0 {
        anyhow::bail!("could not enumerate sessions");
    }
    let mut result = Vec::new();
    // SAFETY: WTSEnumerateSessionsW returned `count` valid records at `info`.
    let records = unsafe { std::slice::from_raw_parts(info, count as usize) };
    for record in records {
        let mut buffer: *mut u16 = std::ptr::null_mut();
        let mut bytes = 0u32;
        // SAFETY: valid out-pointers; the returned buffer is freed below.
        let ok = unsafe {
            WTSQuerySessionInformationW(
                WTS_CURRENT_SERVER_HANDLE,
                record.SessionId,
                WTSSessionInfo,
                &mut buffer,
                &mut bytes,
            )
        };
        if ok == 0 || buffer.is_null() {
            continue;
        }
        if bytes as usize >= std::mem::size_of::<WTSINFOW>() {
            // SAFETY: the API returned a WTSINFOW of at least that size.
            let details = unsafe { &*buffer.cast::<WTSINFOW>() };
            let name = utf16_until_nul(&details.UserName);
            // Session 0 and the listener have no user: not a login.
            if !name.is_empty() {
                result.push(LoggedInSession {
                    name,
                    terminal: utf16_until_nul(&details.WinStationName),
                    login_time: format_filetime(details.LogonTime),
                    source: None,
                });
            }
        }
        // SAFETY: `buffer` was allocated by WTSQuerySessionInformationW.
        unsafe { WTSFreeMemory(buffer.cast()) };
    }
    // SAFETY: `info` was allocated by WTSEnumerateSessionsW.
    unsafe { WTSFreeMemory(info.cast()) };
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::format_filetime;

    #[test]
    fn filetime_ticks_format_as_utc_dates() {
        // 2026-11-08 13:33 UTC.
        let ticks = (1_794_144_780i64 + 11_644_473_600) * 10_000_000;
        assert_eq!(format_filetime(ticks), "2026-11-08 13:33 UTC");
        assert_eq!(format_filetime(0), "");
    }
}
