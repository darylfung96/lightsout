//! Local time without a date crate.
//!
//! The shell manager used `date`, `date -Iseconds` and `date +%H:%M`. The
//! binary keeps the same formats by asking libc's `localtime_r`/`strftime`,
//! so it stays a single static binary and behaves like the machine's clock.

use std::ffi::CString;
use std::os::raw::{c_char, c_long, c_void};
use std::time::{SystemTime, UNIX_EPOCH};

#[repr(C, align(16))]
struct TmBuf {
    bytes: [u8; 256],
}

extern "C" {
    fn localtime_r(timep: *const c_long, result: *mut c_void) -> *mut c_void;
    fn strftime(s: *mut c_char, max: usize, format: *const c_char, tm: *const c_void) -> usize;
}

pub const ISO: &str = "%Y-%m-%dT%H:%M:%S%z";

pub fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn format_epoch(epoch: i64, fmt: &str) -> String {
    let mut buf = TmBuf { bytes: [0; 256] };
    let stamp: c_long = epoch as c_long;
    unsafe {
        let tm = localtime_r(&stamp, &mut buf as *mut TmBuf as *mut c_void);
        if tm.is_null() {
            return String::new();
        }
        let Ok(cfmt) = CString::new(fmt) else {
            return String::new();
        };
        let mut out = [0 as c_char; 256];
        let n = strftime(out.as_mut_ptr(), out.len(), cfmt.as_ptr(), tm);
        if n == 0 {
            return String::new();
        }
        let bytes: Vec<u8> = out[..n].iter().map(|c| *c as u8).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

pub fn today() -> String {
    format_epoch(now_epoch(), "%Y-%m-%d")
}

pub fn now_hm() -> String {
    format_epoch(now_epoch(), "%H:%M")
}

pub fn iso(epoch: i64) -> String {
    format_epoch(epoch, ISO)
}

pub fn iso_now() -> String {
    iso(now_epoch())
}

pub fn log_stamp(epoch: i64) -> String {
    format_epoch(epoch, "%Y-%m-%d %H:%M:%S")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Civil {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// Seconds east of UTC. `None` is a naive local timestamp.
    pub offset: Option<i32>,
}

impl Civil {
    /// Days since 1970-01-01 (Howard Hinnant's civil calendar arithmetic).
    fn days(&self) -> i64 {
        let mut y = self.year as i64;
        let m = self.month as i64;
        if m <= 2 {
            y -= 1;
        }
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let mp = (m + 9) % 12;
        let doy = (153 * mp + 2) / 5 + self.day as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146097 + doe - 719468
    }

    /// Seconds on the civil clock, ignoring any offset.
    pub fn naive_seconds(&self) -> i64 {
        self.days() * 86400 + self.hour as i64 * 3600 + self.minute as i64 * 60 + self.second as i64
    }

    /// UTC epoch seconds for a timestamp that carries an offset.
    pub fn epoch(&self) -> Option<i64> {
        self.offset.map(|off| self.naive_seconds() - off as i64)
    }
}

/// Parse the ISO forms the manager writes (`date -Iseconds`, with or without
/// a colon) and the log's `YYYY-MM-DD HH:MM:SS` form.
pub fn parse_civil(raw: &str) -> Option<Civil> {
    let s = raw.trim();
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let num = |a: usize, n: usize| -> Option<i64> { s.get(a..a + n)?.parse::<i64>().ok() };
    if b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b't' && b[10] != b' ') {
        return None;
    }
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let year = num(0, 4)? as i32;
    let month = num(5, 2)? as u32;
    let day = num(8, 2)? as u32;
    let hour = num(11, 2)? as u32;
    let minute = num(14, 2)? as u32;
    let second = num(17, 2)? as u32;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }

    let mut pos = 19;
    if b.get(pos) == Some(&b'.') {
        pos += 1;
        while b.get(pos).is_some_and(|c| c.is_ascii_digit()) {
            pos += 1;
        }
    }
    let rest = &s[pos..];
    let offset = if rest.is_empty() {
        None
    } else if rest == "Z" || rest == "z" {
        Some(0)
    } else {
        let sign = match rest.as_bytes()[0] {
            b'+' => 1,
            b'-' => -1,
            _ => return None,
        };
        let digits: String = rest[1..].chars().filter(|c| c.is_ascii_digit()).collect();
        match digits.len() {
            2 => Some(sign * digits.parse::<i32>().ok()? * 3600),
            4 => {
                let h: i32 = digits[0..2].parse().ok()?;
                let m: i32 = digits[2..4].parse().ok()?;
                Some(sign * (h * 3600 + m * 60))
            }
            _ => return None,
        }
    };
    Some(Civil {
        year,
        month,
        day,
        hour,
        minute,
        second,
        offset,
    })
}

/// The local wall-clock time as seconds, used to compare a block with log
/// lines that carry no offset.
pub fn now_naive_seconds() -> i64 {
    let stamp = format_epoch(now_epoch(), "%Y-%m-%d %H:%M:%S");
    parse_civil(&stamp)
        .map(|c| c.naive_seconds())
        .unwrap_or_else(now_epoch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_round_trips() {
        let civil = parse_civil("2026-10-20T18:25:01-05:00").unwrap();
        assert_eq!(civil.epoch(), Some(1792538701));
        let naive = parse_civil("2026-10-20 18:25:01").unwrap();
        assert_eq!(naive.offset, None);
        assert_eq!(naive.naive_seconds(), civil.naive_seconds());
    }
}
