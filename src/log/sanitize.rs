use std::fmt::Write;

use anstyle::Style;
use anstyle_parse::{Params, Parser, Perform};

const RESET: &str = "\x1b[0m";

pub fn sanitize(line: &str, base: Option<Style>) -> String {
    let mut sanitizer = Sanitizer::new(base);
    let mut parser = Parser::<anstyle_parse::DefaultCharAccumulator>::new();
    for byte in line.bytes() {
        parser.advance(&mut sanitizer, byte);
    }
    sanitizer.finish()
}

struct Sanitizer {
    base: Option<Style>,
    out: String,
    sgr: String,
    pending_cr: bool,
    styled: bool,
}

impl Sanitizer {
    fn new(base: Option<Style>) -> Self {
        Self {
            base,
            out: String::new(),
            sgr: String::new(),
            pending_cr: false,
            styled: false,
        }
    }

    fn emit_text(&mut self, c: char) {
        if self.pending_cr {
            self.pending_cr = false;
            self.out.clone_from(&self.sgr);
        }
        self.out.push(c);
    }

    fn emit_sgr(&mut self, seq: &str) {
        self.out.push_str(seq);
        self.sgr.push_str(seq);
        self.styled = true;
    }

    fn finish(mut self) -> String {
        if self.styled {
            self.out.push_str(RESET);
        }
        self.out
    }
}

fn render_sgr(params: &Params) -> String {
    let mut seq = String::from("\x1b[");
    for (i, param) in params.iter().enumerate() {
        if i > 0 {
            seq.push(';');
        }
        for (j, sub) in param.iter().enumerate() {
            if j > 0 {
                seq.push(':');
            }
            let _ = write!(seq, "{sub}");
        }
    }
    seq.push('m');
    seq
}

fn is_reset(params: &Params) -> bool {
    params.is_empty() || params.iter().any(|param| param == [0])
}

impl Perform for Sanitizer {
    fn print(&mut self, c: char) {
        self.emit_text(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\t' => self.emit_text('\t'),
            b'\r' => self.pending_cr = true,
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: u8) {
        if action != b'm' || !intermediates.is_empty() || ignore {
            return;
        }

        self.emit_sgr(&render_sgr(params));

        if let Some(base) = self.base
            && is_reset(params)
        {
            self.emit_sgr(&base.render().to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use anstyle::AnsiColor;

    use super::*;

    fn clean(line: &str) -> String {
        sanitize(line, None)
    }

    #[test]
    fn plain_text_passes_through_untouched() {
        assert_eq!(clean("hello world"), "hello world");
        assert_eq!(clean("tab\tseparated"), "tab\tseparated");
        assert_eq!(clean("ünïcödé ✓"), "ünïcödé ✓");
    }

    #[test]
    fn sgr_is_kept_and_followed_by_a_reset() {
        assert_eq!(clean("\x1b[31mhi\x1b[0m"), "\x1b[31mhi\x1b[0m\x1b[0m");
        assert_eq!(clean("\x1b[1;32mbold"), "\x1b[1;32mbold\x1b[0m");
    }

    #[test]
    fn extended_colors_survive_including_subparams() {
        assert_eq!(clean("\x1b[38;5;208mx"), "\x1b[38;5;208mx\x1b[0m");
        assert_eq!(clean("\x1b[38;2;1;2;3mx"), "\x1b[38;2;1;2;3mx\x1b[0m");
        assert_eq!(clean("\x1b[38:2:1:2:3mx"), "\x1b[38:2:1:2:3mx\x1b[0m");
    }

    #[test]
    fn cursor_and_screen_control_is_removed() {
        for seq in [
            "\x1b[2J",
            "\x1b[H",
            "\x1b[1;1H",
            "\x1b[1A",
            "\x1b[K",
            "\x1b[?1049h",
            "\x1b[?25l",
            "\x1b[2;10r",
            "\x1bc",
            "\x1b7",
        ] {
            assert_eq!(clean(&format!("a{seq}b")), "ab", "sequence {seq:?} leaked");
        }
    }

    #[test]
    fn osc_and_dcs_strings_are_removed() {
        assert_eq!(clean("\x1b]0;title\x07text"), "text");
        assert_eq!(
            clean("\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\"),
            "link"
        );
        assert_eq!(clean("\x1bPq#0;2;0;0;0\x1b\\after"), "after");
    }

    #[test]
    fn stray_control_chars_are_dropped() {
        assert_eq!(clean("a\x08b\x07c\x0bd\x0ce"), "abcde");
    }

    #[test]
    fn carriage_return_keeps_the_last_segment() {
        assert_eq!(clean("10%\r50%\r100%"), "100%");
        assert_eq!(clean("done\r"), "done");
    }

    #[test]
    fn carriage_return_keeps_styling_in_effect() {
        assert_eq!(clean("\x1b[32m10%\r100%"), "\x1b[32m100%\x1b[0m");
    }

    #[test]
    fn base_style_is_reapplied_after_resets() {
        let red = Style::new().fg_color(Some(AnsiColor::Red.into()));
        let red_seq = red.render().to_string();

        let out = sanitize("a\x1b[0mb\x1b[mc\x1b[1md", Some(red));
        assert_eq!(
            out,
            format!("a\x1b[0m{red_seq}b\x1b[0m{red_seq}c\x1b[1md\x1b[0m")
        );
    }
}
