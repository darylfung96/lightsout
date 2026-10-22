//! `schedule` — print the trigger, never become the trigger.
//!
//! cron, launchd, systemd or a DSH reminder decides *when* a job runs;
//! LightsOut owns the run contract and the state. This verb prints the line to
//! paste (or the plist to install), so the two decisions stay separate and a
//! crashed scheduler cannot take the run history with it.

use std::env;
use std::path::{Path, PathBuf};

use crate::registry;
use crate::repo;
use crate::state;

pub const USAGE: &str = "usage: lightsout schedule <job> [--launchd] [--at HH:MM] [--auto-repair]

print the cron line (default) or the launchd plist that runs the job; --at
overrides the job's state/expect time and --auto-repair adds the bounded
auto-repair flag to the printed command. Nothing is installed.";

pub fn run(root: &Path, argv: &[String]) -> i32 {
    let mut name: Option<String> = None;
    let mut launchd = false;
    let mut auto = false;
    let mut at: Option<String> = None;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--launchd" => launchd = true,
            "--auto-repair" => auto = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return 0;
            }
            "--at" => match argv.get(i + 1) {
                Some(value) => {
                    at = Some(value.clone());
                    i += 1;
                }
                None => {
                    eprintln!("--at needs HH:MM");
                    return 64;
                }
            },
            other if other.starts_with("--at=") => at = Some(other["--at=".len()..].to_string()),
            other if other.starts_with('-') => {
                eprintln!("schedule: unknown option: {other}");
                return 64;
            }
            other => {
                if name.is_some() {
                    eprintln!("schedule: one job at a time");
                    return 64;
                }
                name = Some(other.to_string());
            }
        }
        i += 1;
    }
    let Some(name) = name else {
        println!("{USAGE}");
        return 64;
    };
    let paths = repo::Paths::new(root, &name);
    if !paths.has_job() {
        eprintln!("no such job: {name}");
        return 64;
    }
    let (hour, minute) = match at {
        Some(text) => match parse_hm(&text) {
            Some(parsed) => parsed,
            None => {
                eprintln!("schedule: --at must be HH:MM");
                return 64;
            }
        },
        None => match state::expect_time(&paths.state) {
            Some((minutes, _label)) => ((minutes / 60) as u32, (minutes % 60) as u32),
            None => {
                eprintln!("schedule: {name} has no valid state/expect; pass --at HH:MM");
                return 64;
            }
        },
    };
    let binary = binary_path();
    if launchd {
        println!("{}", plist(&name, &paths, &binary, hour, minute, auto));
    } else {
        println!("{}", cron(&name, &binary, hour, minute, auto));
    }
    0
}

fn parse_hm(text: &str) -> Option<(u32, u32)> {
    let (hour, minute) = text.split_once(':')?;
    if hour.len() != 2 || minute.len() != 2 {
        return None;
    }
    if !hour.chars().all(|c| c.is_ascii_digit()) || !minute.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let hour: u32 = hour.parse().ok()?;
    let minute: u32 = minute.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    Some((hour, minute))
}

fn binary_path() -> PathBuf {
    env::current_exe()
        .ok()
        .and_then(|path| std::fs::canonicalize(&path).ok())
        .unwrap_or_else(|| PathBuf::from("lightsout"))
}

/// The five cron fields for a recorded time. `status <name>` prints this same
/// prefix, so a detail line cannot name a different trigger than the line
/// `schedule` prints.
pub fn cron_time(hour: u32, minute: u32) -> String {
    format!("{minute} {hour} * * *")
}

/// The cron line runs through `/bin/sh -c`, so a path with a space is quoted
/// for that shell rather than for crontab's field parser.
fn cron(name: &str, binary: &Path, hour: u32, minute: u32, auto: bool) -> String {
    format!(
        "# {name} — from state/expect ({hour:02}:{minute:02}); paste into crontab -e\n\
         {} {}",
        cron_time(hour, minute),
        cron_command(name, binary, auto)
    )
}

fn cron_command(name: &str, binary: &Path, auto: bool) -> String {
    let mut parts = Vec::new();
    if let Ok(home) = env::var("LIGHTSOUT_HOME") {
        if !home.is_empty() {
            parts.push(format!("LIGHTSOUT_HOME={}", shell_quote(&home)));
        }
    }
    parts.push(shell_quote(&binary.to_string_lossy()));
    parts.push("run".into());
    parts.push(shell_quote(name));
    if auto {
        parts.push("--auto-repair".into());
    }
    parts.join(" ")
}

fn plist(
    name: &str,
    paths: &repo::Paths,
    binary: &Path,
    hour: u32,
    minute: u32,
    auto: bool,
) -> String {
    let home = registry::home();
    let log = home.join("logs").join(format!("{name}.log"));
    let workdir = paths.workdir.clone().unwrap_or_else(|| paths.dir.clone());
    let auto_arg = if auto {
        "    <string>--auto-repair</string>\n"
    } else {
        ""
    };
    let mut environment = format!(
        "    <key>HOME</key>\n    <string>{}</string>\n    <key>PATH</key>\n    <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>\n",
        xml_escape(&env::var("HOME").unwrap_or_default())
    );
    environment.push_str(&format!(
        "    <key>LIGHTSOUT_HOME</key>\n    <string>{}</string>\n",
        xml_escape(&home.to_string_lossy())
    ));
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \x20 <key>Label</key>\n\
         \x20 <string>com.lightsout.{name}</string>\n\
         \x20 <key>ProgramArguments</key>\n\
         \x20 <array>\n\
         \x20   <string>{binary}</string>\n\
         \x20   <string>run</string>\n\
         \x20   <string>{name}</string>\n\
         {auto_arg}\x20 </array>\n\
         \x20 <key>WorkingDirectory</key>\n\
         \x20 <string>{workdir}</string>\n\
         \x20 <key>StartCalendarInterval</key>\n\
         \x20 <dict>\n\
         \x20   <key>Hour</key>\n\
         \x20   <integer>{hour}</integer>\n\
         \x20   <key>Minute</key>\n\
         \x20   <integer>{minute}</integer>\n\
         \x20 </dict>\n\
         \x20 <key>StandardOutPath</key>\n\
         \x20 <string>{log}</string>\n\
         \x20 <key>StandardErrorPath</key>\n\
         \x20 <string>{log}</string>\n\
         \x20 <key>EnvironmentVariables</key>\n\
         \x20 <dict>\n\
         {environment}\
         \x20 </dict>\n\
         </dict>\n\
         </plist>",
        binary = xml_escape(&binary.to_string_lossy()),
        workdir = xml_escape(&workdir.to_string_lossy()),
        log = xml_escape(&log.to_string_lossy()),
        auto_arg = auto_arg,
    )
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_-./:@".contains(c))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clock_is_strict() {
        assert_eq!(parse_hm("09:05"), Some((9, 5)));
        assert_eq!(parse_hm("23:59"), Some((23, 59)));
        assert_eq!(parse_hm("24:00"), None);
        assert_eq!(parse_hm("9:05"), None);
        assert_eq!(parse_hm("09:5"), None);
    }

    #[test]
    fn the_cron_line_is_one_shell_command() {
        let line = cron(
            "backup",
            Path::new("/usr/local/bin/lightsout"),
            2,
            30,
            false,
        );
        assert!(line.contains("30 2 * * *"));
        assert!(line.contains("/usr/local/bin/lightsout run backup"));
        assert!(!line.contains("--workspace"));
        assert!(!line.contains("--auto-repair"));

        let auto = cron("backup", Path::new("/usr/local/bin/lightsout"), 2, 30, true);
        assert!(auto.contains("/usr/local/bin/lightsout run backup --auto-repair"));
    }

    #[test]
    fn the_plist_can_enable_auto_repair() {
        let paths = repo::Paths::new(Path::new("/tmp/office"), "backup");
        let binary = Path::new("/usr/local/bin/lightsout");

        let plain = plist("backup", &paths, binary, 2, 30, false);
        assert!(!plain.contains("--auto-repair"));

        let armed = plist("backup", &paths, binary, 2, 30, true);
        assert!(armed.contains("<string>--auto-repair</string>"));
    }
}
