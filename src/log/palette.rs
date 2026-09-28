use std::collections::HashMap;

use anstyle::{AnsiColor, Style};

const COLORS: [AnsiColor; 10] = [
    AnsiColor::Cyan,
    AnsiColor::Magenta,
    AnsiColor::Yellow,
    AnsiColor::Green,
    AnsiColor::Blue,
    AnsiColor::BrightCyan,
    AnsiColor::BrightMagenta,
    AnsiColor::BrightYellow,
    AnsiColor::BrightGreen,
    AnsiColor::BrightBlue,
];

#[derive(Default)]
pub struct Palette {
    assigned: HashMap<String, Style>,
}

impl Palette {
    pub fn style_for(&mut self, sender: &str) -> Style {
        if let Some(style) = self.assigned.get(sender) {
            return *style;
        }

        let color = COLORS[self.assigned.len() % COLORS.len()];
        let style = Style::new().fg_color(Some(color.into())).bold();
        self.assigned.insert(sender.to_owned(), style);
        style
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use anstyle::Color;

    use super::*;

    #[test]
    fn senders_get_distinct_styles_until_the_palette_runs_out() {
        let mut palette = Palette::default();
        let styles: HashSet<_> = (0..COLORS.len())
            .map(|i| palette.style_for(&format!("srv-{i}")))
            .collect();
        assert_eq!(styles.len(), COLORS.len());
    }

    #[test]
    fn a_sender_keeps_its_style() {
        let mut palette = Palette::default();
        let first = palette.style_for("api");
        palette.style_for("db");
        assert_eq!(palette.style_for("api"), first);
    }

    #[test]
    fn assignment_follows_first_seen_order() {
        let mut a = Palette::default();
        let mut b = Palette::default();
        for sender in ["api", "db", "web"] {
            assert_eq!(a.style_for(sender), b.style_for(sender));
        }
    }

    #[test]
    fn red_is_never_handed_out() {
        let mut palette = Palette::default();
        for i in 0..COLORS.len() * 2 {
            let fg = palette.style_for(&format!("srv-{i}")).get_fg_color();
            assert!(!matches!(
                fg,
                Some(Color::Ansi(AnsiColor::Red | AnsiColor::BrightRed))
            ));
        }
    }
}
