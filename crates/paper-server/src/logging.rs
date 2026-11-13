//! Console and file logging in the format Paper administrators are used to:
//! `[12:34:56 INFO]: message`.

use log::{Level, LevelFilter, Log, Metadata, Record};
use parking_lot::Mutex;
use std::fs::{self, File, OpenOptions};
use std::io::{IsTerminal, Write};
use std::path::Path;

struct ConsoleLogger {
    file: Mutex<Option<File>>,
    color: bool,
}

impl Log for ConsoleLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        // Dependencies log under their own targets; keep their debug chatter out.
        let target = record.target();
        let ours = target.starts_with("paper") || target == "plugin" || target == "console";
        if !ours && record.level() > Level::Warn {
            return;
        }
        let time = chrono::Local::now().format("%H:%M:%S");
        let level = match record.level() {
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        };
        let message = record.args().to_string();
        let plain = format!("[{time} {level}]: {message}");
        {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            let result = if self.color {
                match record.level() {
                    Level::Error => writeln!(out, "\x1b[91m{plain}\x1b[0m"),
                    Level::Warn => writeln!(out, "\x1b[93m{plain}\x1b[0m"),
                    _ => writeln!(out, "{plain}"),
                }
            } else {
                writeln!(out, "{}", strip_ansi(&plain))
            };
            let _ = result;
        }
        if let Some(file) = self.file.lock().as_mut() {
            let _ = writeln!(file, "{}", strip_ansi(&plain));
        }
    }

    fn flush(&self) {
        let _ = std::io::stdout().flush();
        if let Some(file) = self.file.lock().as_mut() {
            let _ = file.flush();
        }
    }
}

/// Removes ANSI colour escapes (plugin and component output may contain them).
pub fn strip_ansi(s: &str) -> String {
    if !s.contains('\x1b') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip a CSI sequence: ESC [ ... final byte in @..~
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Installs the logger. `log_dir` receives `latest.log` (pass `None` for console only).
/// Safe to call more than once; later calls are ignored.
pub fn init(log_dir: Option<&Path>, level: LevelFilter) {
    let file = log_dir.and_then(|dir| {
        fs::create_dir_all(dir).ok()?;
        OpenOptions::new().create(true).write(true).truncate(true).open(dir.join("latest.log")).ok()
    });
    let logger = ConsoleLogger { file: Mutex::new(file), color: std::io::stdout().is_terminal() };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(level);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_escapes() {
        assert_eq!(strip_ansi("\x1b[0;91mred\x1b[0m plain"), "red plain");
        assert_eq!(strip_ansi("no escapes"), "no escapes");
        assert_eq!(strip_ansi("\x1b[38;2;1;2;3mrgb"), "rgb");
    }
}
