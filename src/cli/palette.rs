use ratatui::style::Color;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorDepth {
    Truecolor,
    Xterm256,
    Ansi16,
}

/// Decide the color depth from environment values, in precedence order.
///
/// `tmux_rgb` is whether tmux can pass 24-bit color through to the attached
/// client's terminal (`None` = not running inside tmux). Inside tmux the
/// environment lies: `$COLORTERM` is inherited from whatever terminal the tmux
/// *server* was started under, but every escape we emit is filtered by tmux,
/// which drops `38;2` SGR unless the outer terminal advertises RGB/Tc. So a
/// truecolor verdict earned from the environment is capped at `Xterm256` when
/// tmux reports no RGB passthrough. An explicit `$DAEMONEYE_COLOR` override is
/// exempt — it is the user's escape hatch and always wins.
pub fn detect_color_depth(
    override_var: Option<&str>, // $DAEMONEYE_COLOR
    colorterm: Option<&str>,    // $COLORTERM
    term: Option<&str>,         // $TERM
    tmux_rgb: Option<bool>,     // inside tmux: can it pass RGB through?
) -> ColorDepth {
    // 1. Explicit override wins
    if let Some(v) = override_var {
        match v.to_lowercase().as_str() {
            "truecolor" | "24bit" => return ColorDepth::Truecolor,
            "256" => return ColorDepth::Xterm256,
            "16" | "basic" => return ColorDepth::Ansi16,
            _ => { /* unrecognized → fall through */ }
        }
    }

    let cap_for_tmux = |depth: ColorDepth| {
        if depth == ColorDepth::Truecolor && tmux_rgb == Some(false) {
            ColorDepth::Xterm256
        } else {
            depth
        }
    };

    // 2. COLORTERM containing "truecolor" or "24bit" (case-insensitive) → Truecolor
    if let Some(ct) = colorterm {
        let ct_low = ct.to_lowercase();
        if ct_low.contains("truecolor") || ct_low.contains("24bit") {
            return cap_for_tmux(ColorDepth::Truecolor);
        }
    }

    // 3. TERM containing "direct" → Truecolor (xterm-direct family)
    if let Some(t) = term {
        let t_low = t.to_lowercase();
        if t_low.contains("direct") {
            return cap_for_tmux(ColorDepth::Truecolor);
        }
    }

    // 4. TERM containing "256color" → Xterm256
    if let Some(t) = term
        && t.to_lowercase().contains("256color")
    {
        return ColorDepth::Xterm256;
    }

    // 5. Otherwise → Ansi16
    ColorDepth::Ansi16
}

/// Read the real environment. The only place the env is consulted.
///
/// The tmux RGB probe shells out to the tmux server, so its result is cached
/// for the life of the process.
pub fn detect_from_env() -> ColorDepth {
    let ov = std::env::var("DAEMONEYE_COLOR").ok();
    let ct = std::env::var("COLORTERM").ok();
    let t = std::env::var("TERM").ok();
    let tmux_rgb = std::env::var_os("TMUX").is_some().then(tmux_rgb_cached);
    detect_color_depth(ov.as_deref(), ct.as_deref(), t.as_deref(), tmux_rgb)
}

fn tmux_rgb_cached() -> bool {
    static TMUX_RGB: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TMUX_RGB.get_or_init(crate::tmux::client_supports_rgb)
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub depth: ColorDepth,
}

impl Palette {
    pub fn for_depth(depth: ColorDepth) -> Self {
        Self { depth }
    }

    pub fn from_env() -> Self {
        Self::for_depth(detect_from_env())
    }

    /// The DaemonEye blood-red (borders, logo body, spinner frame).
    pub fn red(&self) -> Color {
        match self.depth {
            ColorDepth::Truecolor => Color::Rgb(180, 0, 0),
            ColorDepth::Xterm256 => Color::Indexed(124),
            ColorDepth::Ansi16 => Color::Red,
        }
    }

    /// The DaemonEye deep-yellow (eye highlight, titles, spinner glyph).
    pub fn yellow(&self) -> Color {
        match self.depth {
            ColorDepth::Truecolor => Color::Rgb(220, 160, 0),
            ColorDepth::Xterm256 => Color::Indexed(178),
            ColorDepth::Ansi16 => Color::Yellow,
        }
    }
}

/// Foreground-color escape for the given depth. `basic` is a full SGR code
/// (e.g. 96 = bright cyan), not a color index.
pub fn sgr_fg(depth: ColorDepth, rgb: (u8, u8, u8), idx256: u8, basic: u8) -> String {
    match depth {
        ColorDepth::Truecolor => format!("\x1b[38;2;{};{};{}m", rgb.0, rgb.1, rgb.2),
        ColorDepth::Xterm256 => format!("\x1b[38;5;{idx256}m"),
        ColorDepth::Ansi16 => format!("\x1b[{basic}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_override_wins() {
        assert_eq!(
            detect_color_depth(Some("256"), Some("truecolor"), Some("xterm-direct"), None),
            ColorDepth::Xterm256
        );
    }

    #[test]
    fn detect_colorterm_truecolor() {
        assert_eq!(
            detect_color_depth(None, Some("truecolor"), Some("xterm-256color"), None),
            ColorDepth::Truecolor
        );
    }

    #[test]
    fn detect_tmux_256color_is_not_truecolor() {
        assert_eq!(
            detect_color_depth(None, None, Some("tmux-256color"), None),
            ColorDepth::Xterm256
        );
    }

    #[test]
    fn detect_term_direct_is_truecolor() {
        assert_eq!(
            detect_color_depth(None, None, Some("xterm-direct"), None),
            ColorDepth::Truecolor
        );
    }

    #[test]
    fn detect_colorterm_yes_is_not_truecolor() {
        assert_eq!(
            detect_color_depth(None, Some("yes"), Some("xterm"), None),
            ColorDepth::Ansi16
        );
    }

    #[test]
    fn detect_all_unset_is_ansi16() {
        assert_eq!(
            detect_color_depth(None, None, None, None),
            ColorDepth::Ansi16
        );
    }

    #[test]
    fn detect_tmux_without_rgb_caps_colorterm_truecolor_at_256() {
        // The pinky-over-ssh case: COLORTERM=truecolor leaks into the tmux
        // session but tmux cannot pass 38;2 SGR through — emit 256-color.
        assert_eq!(
            detect_color_depth(None, Some("truecolor"), Some("tmux-256color"), Some(false)),
            ColorDepth::Xterm256
        );
    }

    #[test]
    fn detect_tmux_without_rgb_caps_term_direct_at_256() {
        assert_eq!(
            detect_color_depth(None, None, Some("tmux-direct"), Some(false)),
            ColorDepth::Xterm256
        );
    }

    #[test]
    fn detect_tmux_with_rgb_keeps_truecolor() {
        assert_eq!(
            detect_color_depth(None, Some("truecolor"), Some("tmux-256color"), Some(true)),
            ColorDepth::Truecolor
        );
    }

    #[test]
    fn detect_override_beats_tmux_downgrade() {
        assert_eq!(
            detect_color_depth(Some("truecolor"), None, Some("tmux-256color"), Some(false)),
            ColorDepth::Truecolor
        );
    }

    #[test]
    fn detect_tmux_without_rgb_leaves_lower_depths_alone() {
        assert_eq!(
            detect_color_depth(None, None, Some("screen"), Some(false)),
            ColorDepth::Ansi16
        );
    }

    #[test]
    fn palette_256_red_is_indexed_124() {
        let p = Palette::for_depth(ColorDepth::Xterm256);
        assert_eq!(p.red(), Color::Indexed(124));
        assert_eq!(p.yellow(), Color::Indexed(178));
    }

    #[test]
    fn palette_truecolor_matches_legacy_rgb() {
        let p = Palette::for_depth(ColorDepth::Truecolor);
        assert_eq!(p.red(), Color::Rgb(180, 0, 0));
        assert_eq!(p.yellow(), Color::Rgb(220, 160, 0));
    }

    #[test]
    fn sgr_fg_emits_escape_per_depth() {
        let rgb = (100u8, 210, 255);
        assert_eq!(
            sgr_fg(ColorDepth::Truecolor, rgb, 117, 96),
            "\x1b[38;2;100;210;255m"
        );
        assert_eq!(sgr_fg(ColorDepth::Xterm256, rgb, 117, 96), "\x1b[38;5;117m");
        assert_eq!(sgr_fg(ColorDepth::Ansi16, rgb, 117, 96), "\x1b[96m");
    }
}
