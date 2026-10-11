pub mod palette;
mod sanitize;

use std::io::Write;

use anstyle::{AnsiColor, Style};

use self::{palette::Palette, sanitize::sanitize};

const ORCHESTRATOR: &str = "orchestrator";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Notice,
    Error,
}

pub struct Logger<W: Write> {
    out: W,
    palette: Palette,
    label_width: usize,
}

impl Logger<anstream::Stdout> {
    pub fn stdout() -> Self {
        Self::new(anstream::stdout())
    }
}

impl<W: Write> Logger<W> {
    pub fn new(out: W) -> Self {
        let mut logger = Self {
            out,
            palette: Palette::default(),
            label_width: 0,
        };
        logger.widen(ORCHESTRATOR);
        logger
    }

    pub fn register<'a>(&mut self, senders: impl IntoIterator<Item = &'a str>) {
        let mut senders: Vec<_> = senders.into_iter().collect();
        senders.sort_unstable();
        for sender in senders {
            self.palette.style_for(sender);
            self.widen(sender);
        }
    }

    pub fn log(&mut self, sender: &str, level: Level, msg: &str) {
        let (label_style, body_style) = if sender == ORCHESTRATOR {
            (Style::new().dimmed(), Style::new().dimmed())
        } else {
            (self.palette.style_for(sender), Style::new())
        };
        self.write(sender, label_style, body_style, level, msg);
    }

    pub fn orchestrator(&mut self, level: Level, msg: &str) {
        self.log(ORCHESTRATOR, level, msg);
    }

    fn widen(&mut self, sender: &str) {
        self.label_width = self.label_width.max(label_width(sender));
    }

    fn write(
        &mut self,
        sender: &str,
        label_style: Style,
        body_style: Style,
        level: Level,
        msg: &str,
    ) {
        self.widen(sender);

        let red = Some(AnsiColor::Red.into());
        let (prefix, body) = match level {
            Level::Info => (label_style, body_style),
            Level::Notice => (Style::new().fg_color(red), body_style),
            Level::Error => (
                Style::new()
                    .fg_color(Some(AnsiColor::BrightWhite.into()))
                    .bg_color(red)
                    .bold(),
                Style::new().fg_color(red),
            ),
        };
        let base = (body != Style::new()).then_some(body);
        let pad = self.label_width - label_width(sender);

        for line in msg.split('\n') {
            let text = sanitize(line, base);
            let _ = writeln!(
                self.out,
                "{prefix}[{sender}]{prefix:#}{:pad$} {body}{text}{body:#}",
                ""
            );
        }
    }
}

fn label_width(sender: &str) -> usize {
    sender.chars().count() + 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(f: impl FnOnce(&mut Logger<&mut Vec<u8>>)) -> String {
        let mut buf = Vec::new();
        f(&mut Logger::new(&mut buf));
        String::from_utf8(buf).unwrap()
    }

    /// Output as it would look with colors stripped.
    fn plain(s: &str) -> String {
        let mut buf = Vec::new();
        let mut stream = anstream::StripStream::new(&mut buf);
        stream.write_all(s.as_bytes()).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn lines_are_prefixed_and_padded_to_the_widest_sender() {
        let out = output(|log| {
            log.register(["api", "a-much-longer-name"]);
            log.log("api", Level::Info, "hello");
            log.orchestrator(Level::Info, "started");
        });
        assert_eq!(
            plain(&out),
            "[api]                hello\n[orchestrator]       started\n"
        );
    }

    #[test]
    fn orchestrator_sets_the_minimum_width() {
        let out = output(|log| log.log("db", Level::Info, "up"));
        assert_eq!(plain(&out), "[db]           up\n");
    }

    #[test]
    fn senders_get_their_own_label_color() {
        let out = output(|log| {
            log.register(["api", "db"]);
            log.log("api", Level::Info, "x");
            log.log("db", Level::Info, "x");
        });
        let lines: Vec<_> = out.lines().collect();
        let label = |line: &str| line[..line.find("m[").unwrap()].to_owned();
        assert_ne!(label(lines[0]), label(lines[1]));
    }

    #[test]
    fn multi_line_messages_prefix_every_line() {
        let out = output(|log| log.log("api", Level::Info, "one\ntwo"));
        assert_eq!(plain(&out), "[api]          one\n[api]          two\n");
    }

    #[test]
    fn errors_have_a_red_background_prefix_and_a_red_body() {
        let out = output(|log| log.orchestrator(Level::Error, "boom"));
        assert!(
            out.starts_with("\x1b[1m\x1b[97m\x1b[41m[orchestrator]"),
            "{out:?}"
        );
        assert!(out.contains("\x1b[31mboom\x1b[0m"), "{out:?}");
    }

    #[test]
    fn notices_tint_only_the_prefix() {
        let out = output(|log| log.log("api", Level::Notice, "warn"));
        assert!(out.starts_with("\x1b[31m[api]\x1b[0m"), "{out:?}");
        assert!(out.ends_with(" warn\n"), "{out:?}");
    }

    #[test]
    fn orchestrator_info_is_dimmed() {
        let out = output(|log| log.orchestrator(Level::Info, "started"));
        assert!(out.starts_with("\x1b[2m[orchestrator]\x1b[0m"), "{out:?}");
        assert!(out.contains("\x1b[2mstarted\x1b[0m"), "{out:?}");
    }

    #[test]
    fn orchestrator_notices_keep_the_red_prefix_with_a_dimmed_body() {
        let out = output(|log| log.orchestrator(Level::Notice, "skipping"));
        assert!(out.starts_with("\x1b[31m[orchestrator]\x1b[0m"), "{out:?}");
        assert!(out.contains("\x1b[2mskipping\x1b[0m"), "{out:?}");
    }

    #[test]
    fn service_escapes_are_sanitized() {
        let out = output(|log| log.log("api", Level::Info, "a\x1b[2Jb\x1b[32mc"));
        assert!(out.ends_with(" ab\x1b[32mc\x1b[0m\n"), "{out:?}");
    }
}
