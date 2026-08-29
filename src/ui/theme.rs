use ratatui::style::Color;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorMode {
    TrueColor,
    Ansi256,
    Ansi,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub background: Color,
    pub foreground: Color,
    pub muted: Color,
    pub primary: Color,
    pub secondary: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
    pub info: Color,
    pub selection_bg: Color,
    pub selection_fg: Color,
    pub border: Color,
    pub border_active: Color,
    pub github: Color,
    pub gitlab: Color,
    pub forgejo: Color,
    pub draft: Color,
    pub review_requested: Color,
    pub approved: Color,
    pub changes_requested: Color,
    pub ci_running: Color,
    pub ci_success: Color,
    pub ci_failure: Color,
}

impl Theme {
    pub fn detect() -> Self {
        let mode = match std::env::var("COLORTERM").ok().as_deref() {
            Some("truecolor") | Some("24bit") => ColorMode::TrueColor,
            _ if std::env::var("TERM")
                .unwrap_or_default()
                .contains("256color") =>
            {
                ColorMode::Ansi256
            }
            _ => ColorMode::Ansi,
        };
        Self::for_mode(mode)
    }
    pub fn for_mode(mode: ColorMode) -> Self {
        let rgb = |r, g, b, fallback| match mode {
            ColorMode::TrueColor => Color::Rgb(r, g, b),
            ColorMode::Ansi256 => Color::Indexed(fallback),
            ColorMode::Ansi => Color::Indexed(fallback.min(15)),
        };
        Self {
            background: rgb(13, 17, 25, 0),
            foreground: rgb(210, 220, 235, 15),
            muted: rgb(110, 125, 148, 244),
            primary: rgb(91, 201, 255, 81),
            secondary: rgb(161, 120, 255, 141),
            success: rgb(96, 218, 151, 78),
            warning: rgb(246, 190, 86, 220),
            danger: rgb(247, 100, 112, 203),
            info: rgb(91, 201, 255, 81),
            selection_bg: rgb(33, 55, 76, 24),
            selection_fg: rgb(239, 248, 255, 15),
            border: rgb(45, 62, 82, 239),
            border_active: rgb(91, 201, 255, 81),
            github: rgb(180, 190, 205, 252),
            gitlab: rgb(252, 109, 38, 202),
            forgejo: rgb(113, 187, 100, 71),
            draft: rgb(126, 143, 174, 103),
            review_requested: rgb(203, 130, 255, 177),
            approved: rgb(96, 218, 151, 78),
            changes_requested: rgb(247, 100, 112, 203),
            ci_running: rgb(91, 201, 255, 81),
            ci_success: rgb(96, 218, 151, 78),
            ci_failure: rgb(247, 100, 112, 203),
        }
    }
}
