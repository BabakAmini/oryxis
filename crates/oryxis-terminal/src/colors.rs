use iced::Color;
use alacritty_terminal::vte::ansi::{self, NamedColor};

/// Terminal color theme name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalTheme {
    OryxisDark,
    OryxisLight,
    Termius,
    Darcula,
    IslandsDark,
    Dracula,
    Monokai,
    HackerGreen,
    OneDark,
    GruvboxDark,
    Nord,
    NordLight,
    SolarizedDark,
    SolarizedLight,
    NightOwl,
    NightOwlLight,
    PaperLight,
    AyuDark,
    AyuLight,
    CatppuccinLatte,
    CatppuccinMocha,
    EverforestDark,
    GithubDark,
    GithubLight,
    GruvboxLight,
    Horizon,
    Kanagawa,
    OneLight,
    RosePine,
    TokyoNight,
    Zenburn,
}

impl TerminalTheme {
    /// Picker order: the dark group first, then the light group,
    /// alphabetical by display name within each (the h3 curation
    /// decision; `list_order_is_dark_then_light_alphabetical` enforces
    /// it). Nothing else may depend on this order: themes persist by
    /// NAME everywhere.
    pub const ALL: &[TerminalTheme] = &[
        // Dark
        Self::AyuDark,
        Self::CatppuccinMocha,
        Self::Darcula,
        Self::Dracula,
        Self::EverforestDark,
        Self::GithubDark,
        Self::GruvboxDark,
        Self::HackerGreen,
        Self::Horizon,
        Self::IslandsDark,
        Self::Kanagawa,
        Self::Monokai,
        Self::NightOwl,
        Self::Nord,
        Self::OneDark,
        Self::OryxisDark,
        Self::RosePine,
        Self::SolarizedDark,
        Self::Termius,
        Self::TokyoNight,
        Self::Zenburn,
        // Light
        Self::AyuLight,
        Self::CatppuccinLatte,
        Self::GithubLight,
        Self::GruvboxLight,
        Self::NightOwlLight,
        Self::NordLight,
        Self::OneLight,
        Self::OryxisLight,
        Self::PaperLight,
        Self::SolarizedLight,
    ];

    pub fn name(&self) -> &'static str {
        match self {
            Self::OryxisDark => "Oryxis Dark",
            Self::OryxisLight => "Oryxis Light",
            Self::Termius => "Termius",
            Self::Darcula => "Darcula",
            Self::IslandsDark => "Islands Dark",
            Self::Dracula => "Dracula",
            Self::Monokai => "Monokai",
            Self::HackerGreen => "Hacker Green",
            Self::OneDark => "One Dark",
            Self::GruvboxDark => "Gruvbox Dark",
            Self::Nord => "Nord",
            Self::NordLight => "Nord Light",
            Self::SolarizedDark => "Solarized Dark",
            Self::SolarizedLight => "Solarized Light",
            Self::NightOwl => "Night Owl",
            Self::NightOwlLight => "Night Owl Light",
            Self::PaperLight => "Paper Light",
            Self::AyuDark => "Ayu Dark",
            Self::AyuLight => "Ayu Light",
            Self::CatppuccinLatte => "Catppuccin Latte",
            Self::CatppuccinMocha => "Catppuccin Mocha",
            Self::EverforestDark => "Everforest Dark",
            Self::GithubDark => "GitHub Dark",
            Self::GithubLight => "GitHub Light",
            Self::GruvboxLight => "Gruvbox Light",
            Self::Horizon => "Horizon",
            Self::Kanagawa => "Kanagawa",
            Self::OneLight => "One Light",
            Self::RosePine => "Rosé Pine",
            Self::TokyoNight => "Tokyo Night",
            Self::Zenburn => "Zenburn",
        }
    }

    pub fn palette(&self) -> TerminalPalette {
        match self {
            Self::OryxisDark => TerminalPalette::oryxis_dark(),
            Self::OryxisLight => TerminalPalette::oryxis_light(),
            Self::Termius => TerminalPalette::termius(),
            Self::Darcula => TerminalPalette::darcula(),
            Self::IslandsDark => TerminalPalette::islands_dark(),
            Self::Dracula => TerminalPalette::dracula(),
            Self::Monokai => TerminalPalette::monokai(),
            Self::HackerGreen => TerminalPalette::hacker_green(),
            Self::OneDark => TerminalPalette::one_dark(),
            Self::GruvboxDark => TerminalPalette::gruvbox_dark(),
            Self::Nord => TerminalPalette::nord(),
            Self::NordLight => TerminalPalette::nord_light(),
            Self::SolarizedDark => TerminalPalette::solarized_dark(),
            Self::SolarizedLight => TerminalPalette::solarized_light(),
            Self::NightOwl => TerminalPalette::night_owl(),
            Self::NightOwlLight => TerminalPalette::night_owl_light(),
            Self::PaperLight => TerminalPalette::paper_light(),
            Self::AyuDark => TerminalPalette::ayu_dark(),
            Self::AyuLight => TerminalPalette::ayu_light(),
            Self::CatppuccinLatte => TerminalPalette::catppuccin_latte(),
            Self::CatppuccinMocha => TerminalPalette::catppuccin_mocha(),
            Self::EverforestDark => TerminalPalette::everforest_dark(),
            Self::GithubDark => TerminalPalette::github_dark(),
            Self::GithubLight => TerminalPalette::github_light(),
            Self::GruvboxLight => TerminalPalette::gruvbox_light(),
            Self::Horizon => TerminalPalette::horizon(),
            Self::Kanagawa => TerminalPalette::kanagawa(),
            Self::OneLight => TerminalPalette::one_light(),
            Self::RosePine => TerminalPalette::rose_pine(),
            Self::TokyoNight => TerminalPalette::tokyo_night(),
            Self::Zenburn => TerminalPalette::zenburn(),
        }
    }
}

/// Terminal color palette.
#[derive(Debug, Clone)]
pub struct TerminalPalette {
    pub foreground: Color,
    pub background: Color,
    pub cursor: Color,
    pub ansi: [Color; 16],
}

impl Default for TerminalPalette {
    fn default() -> Self {
        Self::oryxis_dark()
    }
}

impl TerminalPalette {
    /// Oryxis Dark, like Termius Dark: white text, teal cursor/accent, vivid ANSI colors
    pub fn oryxis_dark() -> Self {
        Self {
            foreground: Color::from_rgb(0.133, 0.60, 0.569), // teal (like Termius Dark green)
            background: Color::from_rgb(0.055, 0.071, 0.067),
            cursor: Color::from_rgb(0.133, 0.60, 0.569),     // teal cursor
            ansi: [
                Color::from_rgb(0.18, 0.20, 0.19),    // 0 Black
                Color::from_rgb(0.95, 0.40, 0.42),    // 1 Red (vivid)
                Color::from_rgb(0.30, 0.82, 0.55),    // 2 Green (vivid)
                Color::from_rgb(0.95, 0.78, 0.30),    // 3 Yellow (vivid)
                Color::from_rgb(0.45, 0.65, 0.95),    // 4 Blue (vivid)
                Color::from_rgb(0.75, 0.55, 0.90),    // 5 Magenta
                Color::from_rgb(0.20, 0.75, 0.70),    // 6 Cyan (teal)
                Color::from_rgb(0.80, 0.82, 0.80),    // 7 White
                Color::from_rgb(0.40, 0.42, 0.40),    // 8 Bright Black
                Color::from_rgb(1.0, 0.55, 0.55),     // 9 Bright Red
                Color::from_rgb(0.40, 0.90, 0.65),    // 10 Bright Green
                Color::from_rgb(1.0, 0.88, 0.45),     // 11 Bright Yellow
                Color::from_rgb(0.55, 0.75, 1.0),     // 12 Bright Blue
                Color::from_rgb(0.85, 0.68, 0.98),    // 13 Bright Magenta
                Color::from_rgb(0.33, 0.85, 0.78),    // 14 Bright Cyan
                Color::from_rgb(0.93, 0.94, 0.93),    // 15 Bright White
            ],
        }
    }

    /// Hacker Green, classic green-on-black
    pub fn hacker_green() -> Self {
        Self {
            foreground: Color::from_rgb(0.0, 0.87, 0.0),
            background: Color::from_rgb(0.0, 0.04, 0.0),
            cursor: Color::from_rgb(0.0, 1.0, 0.0),
            ansi: [
                Color::from_rgb(0.0, 0.0, 0.0),       // Black
                Color::from_rgb(0.80, 0.0, 0.0),       // Red
                Color::from_rgb(0.0, 0.80, 0.0),       // Green
                Color::from_rgb(0.80, 0.80, 0.0),      // Yellow
                Color::from_rgb(0.0, 0.50, 0.0),       // Blue → dark green
                Color::from_rgb(0.50, 0.0, 0.50),      // Magenta
                Color::from_rgb(0.0, 0.80, 0.50),      // Cyan
                Color::from_rgb(0.75, 0.75, 0.75),     // White
                Color::from_rgb(0.30, 0.30, 0.30),     // Bright Black
                Color::from_rgb(1.0, 0.0, 0.0),        // Bright Red
                Color::from_rgb(0.0, 1.0, 0.0),        // Bright Green
                Color::from_rgb(1.0, 1.0, 0.0),        // Bright Yellow
                Color::from_rgb(0.0, 0.70, 0.0),       // Bright Blue → green
                Color::from_rgb(0.70, 0.0, 0.70),      // Bright Magenta
                Color::from_rgb(0.0, 1.0, 0.70),       // Bright Cyan
                Color::from_rgb(1.0, 1.0, 1.0),        // Bright White
            ],
        }
    }

    /// Dracula
    pub fn dracula() -> Self {
        Self {
            foreground: Color::from_rgb8(248, 248, 242),
            background: Color::from_rgb8(40, 42, 54),
            cursor: Color::from_rgb8(248, 248, 242),
            ansi: [
                Color::from_rgb8(33, 34, 44),     // Black
                Color::from_rgb8(255, 85, 85),    // Red
                Color::from_rgb8(80, 250, 123),   // Green
                Color::from_rgb8(241, 250, 140),  // Yellow
                Color::from_rgb8(189, 147, 249),  // Blue (purple)
                Color::from_rgb8(255, 121, 198),  // Magenta (pink)
                Color::from_rgb8(139, 233, 253),  // Cyan
                Color::from_rgb8(248, 248, 242),  // White
                Color::from_rgb8(98, 114, 164),   // Bright Black
                Color::from_rgb8(255, 110, 110),  // Bright Red
                Color::from_rgb8(105, 255, 148),  // Bright Green
                Color::from_rgb8(255, 255, 165),  // Bright Yellow
                Color::from_rgb8(210, 170, 255),  // Bright Blue
                Color::from_rgb8(255, 146, 213),  // Bright Magenta
                Color::from_rgb8(164, 255, 255),  // Bright Cyan
                Color::from_rgb8(255, 255, 255),  // Bright White
            ],
        }
    }

    /// One Dark, the Atom editor classic (canonical terminal port from
    /// iTerm2-Color-Schemes; bright slots repeat the base hues, as
    /// upstream does).
    pub fn one_dark() -> Self {
        Self {
            foreground: Color::from_rgb8(171, 178, 191),  // #abb2bf
            background: Color::from_rgb8(40, 44, 52),     // #282c34
            cursor: Color::from_rgb8(82, 139, 255),       // #528bff
            ansi: [
                Color::from_rgb8(30, 33, 39),     // Black
                Color::from_rgb8(224, 108, 117),  // Red
                Color::from_rgb8(152, 195, 121),  // Green
                Color::from_rgb8(209, 154, 102),  // Yellow (orange)
                Color::from_rgb8(97, 175, 239),   // Blue
                Color::from_rgb8(198, 120, 221),  // Magenta
                Color::from_rgb8(86, 182, 194),   // Cyan
                Color::from_rgb8(171, 178, 191),  // White
                Color::from_rgb8(92, 99, 112),    // Bright Black
                Color::from_rgb8(224, 108, 117),  // Bright Red
                Color::from_rgb8(152, 195, 121),  // Bright Green
                Color::from_rgb8(229, 192, 123),  // Bright Yellow
                Color::from_rgb8(97, 175, 239),   // Bright Blue
                Color::from_rgb8(198, 120, 221),  // Bright Magenta
                Color::from_rgb8(86, 182, 194),   // Bright Cyan
                Color::from_rgb8(255, 255, 255),  // Bright White
            ],
        }
    }

    /// Gruvbox Dark, Pavel Pertsev's retro-warm classic (canonical
    /// medium-contrast palette).
    pub fn gruvbox_dark() -> Self {
        Self {
            foreground: Color::from_rgb8(235, 219, 178),  // #ebdbb2
            background: Color::from_rgb8(40, 40, 40),     // #282828
            cursor: Color::from_rgb8(235, 219, 178),
            ansi: [
                Color::from_rgb8(40, 40, 40),     // Black
                Color::from_rgb8(204, 36, 29),    // Red
                Color::from_rgb8(152, 151, 26),   // Green
                Color::from_rgb8(215, 153, 33),   // Yellow
                Color::from_rgb8(69, 133, 136),   // Blue
                Color::from_rgb8(177, 98, 134),   // Magenta
                Color::from_rgb8(104, 157, 106),  // Cyan
                Color::from_rgb8(168, 153, 132),  // White
                Color::from_rgb8(146, 131, 116),  // Bright Black
                Color::from_rgb8(251, 73, 52),    // Bright Red
                Color::from_rgb8(184, 187, 38),   // Bright Green
                Color::from_rgb8(250, 189, 47),   // Bright Yellow
                Color::from_rgb8(131, 165, 152),  // Bright Blue
                Color::from_rgb8(211, 134, 155),  // Bright Magenta
                Color::from_rgb8(142, 192, 124),  // Bright Cyan
                Color::from_rgb8(235, 219, 178),  // Bright White
            ],
        }
    }

    /// Solarized Dark
    pub fn solarized_dark() -> Self {
        Self {
            foreground: Color::from_rgb8(131, 148, 150),
            background: Color::from_rgb8(0, 43, 54),
            cursor: Color::from_rgb8(131, 148, 150),
            ansi: [
                Color::from_rgb8(7, 54, 66),      // Black
                Color::from_rgb8(220, 50, 47),    // Red
                Color::from_rgb8(133, 153, 0),    // Green
                Color::from_rgb8(181, 137, 0),    // Yellow
                Color::from_rgb8(38, 139, 210),   // Blue
                Color::from_rgb8(211, 54, 130),   // Magenta
                Color::from_rgb8(42, 161, 152),   // Cyan
                Color::from_rgb8(238, 232, 213),  // White
                Color::from_rgb8(0, 43, 54),      // Bright Black
                Color::from_rgb8(203, 75, 22),    // Bright Red (orange)
                Color::from_rgb8(88, 110, 117),   // Bright Green
                Color::from_rgb8(101, 123, 131),  // Bright Yellow
                Color::from_rgb8(131, 148, 150),  // Bright Blue
                Color::from_rgb8(108, 113, 196),  // Bright Magenta (violet)
                Color::from_rgb8(147, 161, 161),  // Bright Cyan
                Color::from_rgb8(253, 246, 227),  // Bright White
            ],
        }
    }

    /// Monokai
    pub fn monokai() -> Self {
        Self {
            foreground: Color::from_rgb8(248, 248, 242),
            background: Color::from_rgb8(39, 40, 34),
            cursor: Color::from_rgb8(248, 248, 240),
            ansi: [
                Color::from_rgb8(39, 40, 34),     // Black
                Color::from_rgb8(249, 38, 114),   // Red (pink)
                Color::from_rgb8(166, 226, 46),   // Green
                Color::from_rgb8(244, 191, 117),  // Yellow
                Color::from_rgb8(102, 217, 239),  // Blue (cyan)
                Color::from_rgb8(174, 129, 255),  // Magenta (purple)
                Color::from_rgb8(161, 239, 228),  // Cyan
                Color::from_rgb8(248, 248, 242),  // White
                Color::from_rgb8(117, 113, 94),   // Bright Black
                Color::from_rgb8(249, 38, 114),   // Bright Red
                Color::from_rgb8(166, 226, 46),   // Bright Green
                Color::from_rgb8(244, 191, 117),  // Bright Yellow
                Color::from_rgb8(102, 217, 239),  // Bright Blue
                Color::from_rgb8(174, 129, 255),  // Bright Magenta
                Color::from_rgb8(161, 239, 228),  // Bright Cyan
                Color::from_rgb8(248, 248, 242),  // Bright White
            ],
        }
    }

    /// Nord
    pub fn nord() -> Self {
        Self {
            foreground: Color::from_rgb8(216, 222, 233),
            background: Color::from_rgb8(46, 52, 64),
            cursor: Color::from_rgb8(216, 222, 233),
            ansi: [
                Color::from_rgb8(59, 66, 82),     // Black
                Color::from_rgb8(191, 97, 106),   // Red
                Color::from_rgb8(163, 190, 140),  // Green
                Color::from_rgb8(235, 203, 139),  // Yellow
                Color::from_rgb8(129, 161, 193),  // Blue
                Color::from_rgb8(180, 142, 173),  // Magenta
                Color::from_rgb8(136, 192, 208),  // Cyan
                Color::from_rgb8(229, 233, 240),  // White
                Color::from_rgb8(76, 86, 106),    // Bright Black
                Color::from_rgb8(191, 97, 106),   // Bright Red
                Color::from_rgb8(163, 190, 140),  // Bright Green
                Color::from_rgb8(235, 203, 139),  // Bright Yellow
                Color::from_rgb8(129, 161, 193),  // Bright Blue
                Color::from_rgb8(180, 142, 173),  // Bright Magenta
                Color::from_rgb8(143, 188, 187),  // Bright Cyan
                Color::from_rgb8(236, 239, 244),  // Bright White
            ],
        }
    }

    /// Oryxis Light, light counterpart of `oryxis_dark`. White paper
    /// surface, deep teal foreground, slightly desaturated ANSI so
    /// the colours don't strobe against the bright background.
    pub fn oryxis_light() -> Self {
        Self {
            foreground: Color::from_rgb8(33, 56, 66),     // deep teal-grey
            background: Color::from_rgb8(248, 250, 250),
            cursor: Color::from_rgb8(34, 153, 144),        // teal accent
            ansi: [
                Color::from_rgb8(60, 64, 64),     // Black
                Color::from_rgb8(193, 60, 60),    // Red
                Color::from_rgb8(46, 138, 87),    // Green
                Color::from_rgb8(170, 124, 22),   // Yellow / amber
                Color::from_rgb8(45, 102, 168),   // Blue
                Color::from_rgb8(140, 90, 175),   // Magenta
                Color::from_rgb8(33, 142, 134),   // Cyan / teal
                Color::from_rgb8(214, 217, 215),  // White
                Color::from_rgb8(110, 116, 116),  // Bright Black
                Color::from_rgb8(220, 90, 90),    // Bright Red
                Color::from_rgb8(70, 165, 110),   // Bright Green
                Color::from_rgb8(200, 152, 50),   // Bright Yellow
                Color::from_rgb8(75, 132, 198),   // Bright Blue
                Color::from_rgb8(170, 120, 200),  // Bright Magenta
                Color::from_rgb8(64, 174, 166),   // Bright Cyan
                Color::from_rgb8(244, 246, 244),  // Bright White
            ],
        }
    }

    /// Termius, neutral dark navy with cyan accent matching the app
    /// theme of the same name.
    pub fn termius() -> Self {
        Self {
            foreground: Color::from_rgb8(224, 229, 237),
            background: Color::from_rgb8(22, 26, 33),
            cursor: Color::from_rgb8(43, 194, 208),       // Termius cyan
            ansi: [
                Color::from_rgb8(38, 44, 56),
                Color::from_rgb8(232, 98, 98),
                Color::from_rgb8(95, 211, 101),
                Color::from_rgb8(231, 171, 82),
                Color::from_rgb8(91, 162, 232),
                Color::from_rgb8(178, 130, 220),
                Color::from_rgb8(43, 194, 208),
                Color::from_rgb8(207, 213, 222),
                Color::from_rgb8(70, 78, 92),
                Color::from_rgb8(255, 121, 121),
                Color::from_rgb8(120, 230, 130),
                Color::from_rgb8(255, 197, 102),
                Color::from_rgb8(120, 184, 250),
                Color::from_rgb8(206, 162, 240),
                Color::from_rgb8(80, 214, 226),
                Color::from_rgb8(237, 240, 245),
            ],
        }
    }

    /// Darcula, JetBrains' classic dark editor palette: bg `#2B2B2B`,
    /// orange keywords, green strings, blue selection.
    pub fn darcula() -> Self {
        Self {
            foreground: Color::from_rgb8(169, 183, 198),
            background: Color::from_rgb8(43, 43, 43),
            cursor: Color::from_rgb8(187, 181, 159),
            ansi: [
                Color::from_rgb8(43, 43, 43),
                Color::from_rgb8(207, 91, 86),
                Color::from_rgb8(106, 135, 89),    // string green
                Color::from_rgb8(204, 120, 50),    // keyword orange
                Color::from_rgb8(104, 151, 187),
                Color::from_rgb8(155, 110, 165),
                Color::from_rgb8(96, 156, 156),
                Color::from_rgb8(169, 183, 198),
                Color::from_rgb8(89, 89, 89),
                Color::from_rgb8(229, 130, 124),
                Color::from_rgb8(149, 174, 124),
                Color::from_rgb8(255, 198, 109),
                Color::from_rgb8(151, 195, 232),
                Color::from_rgb8(199, 159, 209),
                Color::from_rgb8(135, 195, 195),
                Color::from_rgb8(232, 232, 232),
            ],
        }
    }

    /// Islands Dark, JetBrains' New UI variant. Cooler outer frame,
    /// brighter foreground than Darcula, blue accent.
    pub fn islands_dark() -> Self {
        Self {
            foreground: Color::from_rgb8(223, 225, 229),
            background: Color::from_rgb8(30, 31, 34),
            cursor: Color::from_rgb8(117, 163, 255),
            ansi: [
                Color::from_rgb8(46, 48, 53),
                Color::from_rgb8(221, 92, 92),
                Color::from_rgb8(98, 174, 108),
                Color::from_rgb8(233, 174, 76),
                Color::from_rgb8(117, 163, 255),
                Color::from_rgb8(189, 147, 249),
                Color::from_rgb8(96, 196, 196),
                Color::from_rgb8(206, 209, 214),
                Color::from_rgb8(80, 84, 92),
                Color::from_rgb8(244, 124, 124),
                Color::from_rgb8(125, 198, 135),
                Color::from_rgb8(255, 200, 110),
                Color::from_rgb8(140, 180, 255),
                Color::from_rgb8(208, 175, 255),
                Color::from_rgb8(135, 215, 215),
                Color::from_rgb8(238, 240, 245),
            ],
        }
    }

    /// Nord Light, Snow Storm base. Light counterpart of `nord()`,
    /// keeps the same Frost / Aurora hues but on a near-white surface.
    pub fn nord_light() -> Self {
        Self {
            foreground: Color::from_rgb8(46, 52, 64),
            background: Color::from_rgb8(236, 239, 244),
            cursor: Color::from_rgb8(94, 129, 172),       // Frost blue
            ansi: [
                Color::from_rgb8(59, 66, 82),
                Color::from_rgb8(191, 97, 106),
                Color::from_rgb8(163, 190, 140),
                Color::from_rgb8(208, 165, 86),
                Color::from_rgb8(94, 129, 172),
                Color::from_rgb8(180, 142, 173),
                Color::from_rgb8(136, 192, 208),
                Color::from_rgb8(216, 222, 233),
                Color::from_rgb8(76, 86, 106),
                Color::from_rgb8(208, 116, 124),
                Color::from_rgb8(180, 205, 162),
                Color::from_rgb8(220, 178, 100),
                Color::from_rgb8(129, 161, 193),
                Color::from_rgb8(196, 162, 188),
                Color::from_rgb8(143, 188, 187),
                Color::from_rgb8(229, 233, 240),
            ],
        }
    }

    /// Solarized Light, Ethan Schoonover's bright variant. Same
    /// accent ramp as Solarized Dark, mirrored against the cream
    /// `#FDF6E3` paper.
    pub fn solarized_light() -> Self {
        Self {
            foreground: Color::from_rgb8(101, 123, 131),    // base00
            background: Color::from_rgb8(253, 246, 227),    // base3
            cursor: Color::from_rgb8(101, 123, 131),
            ansi: [
                Color::from_rgb8(7, 54, 66),       // base02
                Color::from_rgb8(220, 50, 47),     // red
                Color::from_rgb8(133, 153, 0),     // green
                Color::from_rgb8(181, 137, 0),     // yellow
                Color::from_rgb8(38, 139, 210),    // blue
                Color::from_rgb8(211, 54, 130),    // magenta
                Color::from_rgb8(42, 161, 152),    // cyan
                Color::from_rgb8(238, 232, 213),   // base2
                Color::from_rgb8(0, 43, 54),       // base03
                Color::from_rgb8(203, 75, 22),     // orange
                Color::from_rgb8(88, 110, 117),    // base01
                Color::from_rgb8(101, 123, 131),   // base00
                Color::from_rgb8(131, 148, 150),   // base0
                Color::from_rgb8(108, 113, 196),   // violet
                Color::from_rgb8(147, 161, 161),   // base1
                Color::from_rgb8(253, 246, 227),   // base3
            ],
        }
    }

    /// Night Owl, Sarah Drasner's theme (the palette Termius ships),
    /// taken from the upstream `Night Owl-color-theme.json`.
    ///
    /// The dark variant declares no `terminal.background` /
    /// `terminal.foreground`, so those come from `editor.background` /
    /// `editor.foreground`, which is what the VS Code terminal falls back
    /// to and what every port of this theme uses. Its light sibling DOES
    /// declare them, so the two are read from different keys on purpose.
    /// Cursor is `editorCursor.foreground`, matching how the rest of this
    /// file gives each palette its own accent cursor.
    pub fn night_owl() -> Self {
        Self {
            foreground: Color::from_rgb8(214, 222, 235),   // #d6deeb
            background: Color::from_rgb8(1, 22, 39),       // #011627
            cursor: Color::from_rgb8(128, 164, 194),       // #80a4c2
            ansi: [
                Color::from_rgb8(1, 22, 39),       // Black   #011627
                Color::from_rgb8(239, 83, 80),     // Red     #EF5350
                Color::from_rgb8(34, 218, 110),    // Green   #22da6e
                Color::from_rgb8(197, 228, 120),   // Yellow  #c5e478
                Color::from_rgb8(130, 170, 255),   // Blue    #82AAFF
                Color::from_rgb8(199, 146, 234),   // Magenta #C792EA
                Color::from_rgb8(33, 199, 168),    // Cyan    #21c7a8
                Color::from_rgb8(255, 255, 255),   // White   #ffffff
                Color::from_rgb8(87, 86, 86),      // Bright Black   #575656
                Color::from_rgb8(239, 83, 80),     // Bright Red     #EF5350
                Color::from_rgb8(34, 218, 110),    // Bright Green   #22da6e
                Color::from_rgb8(255, 235, 149),   // Bright Yellow  #ffeb95
                Color::from_rgb8(130, 170, 255),   // Bright Blue    #82AAFF
                Color::from_rgb8(199, 146, 234),   // Bright Magenta #C792EA
                Color::from_rgb8(127, 219, 202),   // Bright Cyan    #7fdbca
                Color::from_rgb8(255, 255, 255),   // Bright White   #ffffff
            ],
        }
    }

    /// Night Owl Light, the daylight sibling from the same theme (the
    /// README calls it "Light Owl"; the theme file names it "Night Owl
    /// Light", which is what the picker shows so it sorts beside its dark
    /// half like Nord Light and Solarized Light do).
    ///
    /// Upstream sets bright black to the same value as black and bright
    /// blue to the same value as blue. Kept faithful: with
    /// `bold_is_bright` on, bold black text stays exactly as legible as
    /// plain black text, which is the author's intent for a light theme,
    /// and deviating would make this palette not-Night-Owl.
    pub fn night_owl_light() -> Self {
        Self {
            foreground: Color::from_rgb8(64, 63, 83),      // #403f53
            background: Color::from_rgb8(246, 246, 246),   // #F6F6F6
            cursor: Color::from_rgb8(144, 167, 178),       // #90A7B2
            ansi: [
                Color::from_rgb8(64, 63, 83),      // Black   #403f53
                Color::from_rgb8(222, 61, 59),     // Red     #de3d3b
                Color::from_rgb8(8, 145, 106),     // Green   #08916a
                Color::from_rgb8(224, 175, 2),     // Yellow  #E0AF02
                Color::from_rgb8(40, 142, 215),    // Blue    #288ed7
                Color::from_rgb8(214, 67, 138),    // Magenta #d6438a
                Color::from_rgb8(42, 162, 152),    // Cyan    #2AA298
                Color::from_rgb8(147, 161, 161),   // White   #93A1A1
                Color::from_rgb8(64, 63, 83),      // Bright Black   #403f53
                Color::from_rgb8(222, 61, 59),     // Bright Red     #de3d3b
                Color::from_rgb8(8, 145, 106),     // Bright Green   #08916a
                Color::from_rgb8(218, 170, 1),     // Bright Yellow  #daaa01
                Color::from_rgb8(40, 142, 215),    // Bright Blue    #288ed7
                Color::from_rgb8(214, 67, 138),    // Bright Magenta #d6438a
                Color::from_rgb8(42, 162, 152),    // Bright Cyan    #2AA298
                Color::from_rgb8(147, 161, 161),   // Bright White   #93A1A1
            ],
        }
    }

    /// Paper Light, neutral high-contrast light theme. Pure-ish
    /// paper background, near-black text, restrained ANSI for
    /// long-form readability (matches the app's `Paper Light` UI).
    pub fn paper_light() -> Self {
        Self {
            foreground: Color::from_rgb8(34, 34, 34),
            background: Color::from_rgb8(250, 250, 250),
            cursor: Color::from_rgb8(34, 34, 34),
            ansi: [
                Color::from_rgb8(34, 34, 34),
                Color::from_rgb8(170, 50, 50),
                Color::from_rgb8(50, 130, 80),
                Color::from_rgb8(160, 110, 30),
                Color::from_rgb8(45, 95, 165),
                Color::from_rgb8(140, 90, 175),
                Color::from_rgb8(40, 130, 130),
                Color::from_rgb8(230, 230, 230),
                Color::from_rgb8(90, 90, 90),
                Color::from_rgb8(200, 70, 70),
                Color::from_rgb8(70, 160, 100),
                Color::from_rgb8(190, 140, 50),
                Color::from_rgb8(70, 125, 195),
                Color::from_rgb8(170, 120, 200),
                Color::from_rgb8(60, 160, 160),
                Color::from_rgb8(245, 245, 245),
            ],
        }
    }

    /// Catppuccin Mocha, from the org's own alacritty port
    /// (catppuccin/alacritty). Bright 9-14 REPEAT normal 1-6 upstream;
    /// only the black/white pairs differ (surface1/surface2,
    /// subtext1/subtext0; bright white is DARKER than normal white by
    /// design). Cursor is rosewater.
    pub fn catppuccin_mocha() -> Self {
        Self {
            foreground: Color::from_rgb8(205, 214, 244), // text #cdd6f4
            background: Color::from_rgb8(30, 30, 46),    // base #1e1e2e
            cursor: Color::from_rgb8(245, 224, 220),     // rosewater #f5e0dc
            ansi: [
                Color::from_rgb8(69, 71, 90),     // Black          #45475a (surface1)
                Color::from_rgb8(243, 139, 168),  // Red            #f38ba8
                Color::from_rgb8(166, 227, 161),  // Green          #a6e3a1
                Color::from_rgb8(249, 226, 175),  // Yellow         #f9e2af
                Color::from_rgb8(137, 180, 250),  // Blue           #89b4fa
                Color::from_rgb8(245, 194, 231),  // Magenta        #f5c2e7 (pink)
                Color::from_rgb8(148, 226, 213),  // Cyan           #94e2d5 (teal)
                Color::from_rgb8(186, 194, 222),  // White          #bac2de (subtext1)
                Color::from_rgb8(88, 91, 112),    // Bright Black   #585b70 (surface2)
                Color::from_rgb8(243, 139, 168),  // Bright Red     #f38ba8
                Color::from_rgb8(166, 227, 161),  // Bright Green   #a6e3a1
                Color::from_rgb8(249, 226, 175),  // Bright Yellow  #f9e2af
                Color::from_rgb8(137, 180, 250),  // Bright Blue    #89b4fa
                Color::from_rgb8(245, 194, 231),  // Bright Magenta #f5c2e7
                Color::from_rgb8(148, 226, 213),  // Bright Cyan    #94e2d5
                Color::from_rgb8(166, 173, 200),  // Bright White   #a6adc8 (subtext0)
            ],
        }
    }

    /// Catppuccin Latte, the light flavour, same source and the same
    /// bright-repeats-normal pattern as Mocha; being a light theme the
    /// "black" slots are light greys and the "white" slots are dark.
    pub fn catppuccin_latte() -> Self {
        Self {
            foreground: Color::from_rgb8(76, 79, 105),   // text #4c4f69
            background: Color::from_rgb8(239, 241, 245), // base #eff1f5
            cursor: Color::from_rgb8(220, 138, 120),     // rosewater #dc8a78
            ansi: [
                Color::from_rgb8(188, 192, 204),  // Black          #bcc0cc (surface1)
                Color::from_rgb8(210, 15, 57),    // Red            #d20f39
                Color::from_rgb8(64, 160, 43),    // Green          #40a02b
                Color::from_rgb8(223, 142, 29),   // Yellow         #df8e1d
                Color::from_rgb8(30, 102, 245),   // Blue           #1e66f5
                Color::from_rgb8(234, 118, 203),  // Magenta        #ea76cb (pink)
                Color::from_rgb8(23, 146, 153),   // Cyan           #179299 (teal)
                Color::from_rgb8(92, 95, 119),    // White          #5c5f77 (subtext1)
                Color::from_rgb8(172, 176, 190),  // Bright Black   #acb0be (surface2)
                Color::from_rgb8(210, 15, 57),    // Bright Red     #d20f39
                Color::from_rgb8(64, 160, 43),    // Bright Green   #40a02b
                Color::from_rgb8(223, 142, 29),   // Bright Yellow  #df8e1d
                Color::from_rgb8(30, 102, 245),   // Bright Blue    #1e66f5
                Color::from_rgb8(234, 118, 203),  // Bright Magenta #ea76cb
                Color::from_rgb8(23, 146, 153),   // Bright Cyan    #179299
                Color::from_rgb8(108, 111, 133),  // Bright White   #6c6f85 (subtext0)
            ],
        }
    }

    /// Tokyo Night, the default "night" style, from folke's own
    /// alacritty extra (agrees with the ghostty extra on all 16 slots).
    /// The brights are genuinely brightened variants; bright white is
    /// the foreground.
    pub fn tokyo_night() -> Self {
        Self {
            foreground: Color::from_rgb8(192, 202, 245), // #c0caf5
            background: Color::from_rgb8(26, 27, 38),    // #1a1b26
            cursor: Color::from_rgb8(192, 202, 245),     // = foreground (ghostty extra)
            ansi: [
                Color::from_rgb8(21, 22, 30),     // Black          #15161e
                Color::from_rgb8(247, 118, 142),  // Red            #f7768e
                Color::from_rgb8(158, 206, 106),  // Green          #9ece6a
                Color::from_rgb8(224, 175, 104),  // Yellow         #e0af68
                Color::from_rgb8(122, 162, 247),  // Blue           #7aa2f7
                Color::from_rgb8(187, 154, 247),  // Magenta        #bb9af7
                Color::from_rgb8(125, 207, 255),  // Cyan           #7dcfff
                Color::from_rgb8(169, 177, 214),  // White          #a9b1d6
                Color::from_rgb8(65, 72, 104),    // Bright Black   #414868
                Color::from_rgb8(255, 137, 157),  // Bright Red     #ff899d
                Color::from_rgb8(159, 224, 68),   // Bright Green   #9fe044
                Color::from_rgb8(250, 186, 74),   // Bright Yellow  #faba4a
                Color::from_rgb8(141, 176, 255),  // Bright Blue    #8db0ff
                Color::from_rgb8(199, 169, 255),  // Bright Magenta #c7a9ff
                Color::from_rgb8(164, 218, 255),  // Bright Cyan    #a4daff
                Color::from_rgb8(192, 202, 245),  // Bright White   #c0caf5
            ],
        }
    }

    /// Rosé Pine (main), from the org's own alacritty dist. The slot
    /// semantics are deliberately unusual upstream: "green" is pine (a
    /// blue-teal), "blue" is foam (light cyan) and "cyan" is rose
    /// (pinkish); do not "fix" them. Bright 9-15 repeat normal 1-7;
    /// only black differs (overlay vs muted).
    pub fn rose_pine() -> Self {
        Self {
            foreground: Color::from_rgb8(224, 222, 244), // text #e0def4
            background: Color::from_rgb8(25, 23, 36),    // base #191724
            cursor: Color::from_rgb8(82, 79, 103),       // highlight-high #524f67
            ansi: [
                Color::from_rgb8(38, 35, 58),     // Black          #26233a (overlay)
                Color::from_rgb8(235, 111, 146),  // Red            #eb6f92 (love)
                Color::from_rgb8(49, 116, 143),   // Green          #31748f (pine)
                Color::from_rgb8(246, 193, 119),  // Yellow         #f6c177 (gold)
                Color::from_rgb8(156, 207, 216),  // Blue           #9ccfd8 (foam)
                Color::from_rgb8(196, 167, 231),  // Magenta        #c4a7e7 (iris)
                Color::from_rgb8(235, 188, 186),  // Cyan           #ebbcba (rose)
                Color::from_rgb8(224, 222, 244),  // White          #e0def4 (text)
                Color::from_rgb8(110, 106, 134),  // Bright Black   #6e6a86 (muted)
                Color::from_rgb8(235, 111, 146),  // Bright Red     #eb6f92
                Color::from_rgb8(49, 116, 143),   // Bright Green   #31748f
                Color::from_rgb8(246, 193, 119),  // Bright Yellow  #f6c177
                Color::from_rgb8(156, 207, 216),  // Bright Blue    #9ccfd8
                Color::from_rgb8(196, 167, 231),  // Bright Magenta #c4a7e7
                Color::from_rgb8(235, 188, 186),  // Bright Cyan    #ebbcba
                Color::from_rgb8(224, 222, 244),  // Bright White   #e0def4
            ],
        }
    }

    /// Kanagawa (wave), from rebelot's kitty extra, cross-checked with
    /// the ghostty extra (the alacritty extra's normal black 0x090618
    /// is stale; 2 of 3 official exports and the sumiInk0 palette say
    /// #16161d). Some brights are deliberately DULLER than their
    /// normal slot (bright magenta is springViolet). Cursor is
    /// oldWhite.
    pub fn kanagawa() -> Self {
        Self {
            foreground: Color::from_rgb8(220, 215, 186), // fujiWhite #dcd7ba
            background: Color::from_rgb8(31, 31, 40),    // sumiInk3 #1f1f28
            cursor: Color::from_rgb8(200, 192, 147),     // oldWhite #c8c093
            ansi: [
                Color::from_rgb8(22, 22, 29),     // Black          #16161d (sumiInk0)
                Color::from_rgb8(195, 64, 67),    // Red            #c34043
                Color::from_rgb8(118, 148, 106),  // Green          #76946a
                Color::from_rgb8(192, 163, 110),  // Yellow         #c0a36e
                Color::from_rgb8(126, 156, 216),  // Blue           #7e9cd8
                Color::from_rgb8(149, 127, 184),  // Magenta        #957fb8
                Color::from_rgb8(106, 149, 137),  // Cyan           #6a9589
                Color::from_rgb8(200, 192, 147),  // White          #c8c093 (oldWhite)
                Color::from_rgb8(114, 113, 105),  // Bright Black   #727169
                Color::from_rgb8(232, 36, 36),    // Bright Red     #e82424
                Color::from_rgb8(152, 187, 108),  // Bright Green   #98bb6c
                Color::from_rgb8(230, 195, 132),  // Bright Yellow  #e6c384
                Color::from_rgb8(127, 180, 202),  // Bright Blue    #7fb4ca
                Color::from_rgb8(147, 138, 169),  // Bright Magenta #938aa9 (springViolet)
                Color::from_rgb8(122, 168, 159),  // Bright Cyan    #7aa89f
                Color::from_rgb8(220, 215, 186),  // Bright White   #dcd7ba
            ],
        }
    }

    /// Everforest Dark (medium background), from the scheme's own
    /// palette.md plus its vim terminal mapping (the only official
    /// terminal export). Upstream repeats every normal slot into its
    /// bright twin, and defines no cursor (vim uses reverse video), so
    /// the cursor is the foreground.
    pub fn everforest_dark() -> Self {
        Self {
            foreground: Color::from_rgb8(211, 198, 170), // fg #d3c6aa
            background: Color::from_rgb8(45, 53, 59),    // bg0 #2d353b
            cursor: Color::from_rgb8(211, 198, 170),     // = foreground
            ansi: [
                Color::from_rgb8(71, 82, 88),     // Black          #475258 (bg3)
                Color::from_rgb8(230, 126, 128),  // Red            #e67e80
                Color::from_rgb8(167, 192, 128),  // Green          #a7c080
                Color::from_rgb8(219, 188, 127),  // Yellow         #dbbc7f
                Color::from_rgb8(127, 187, 179),  // Blue           #7fbbb3
                Color::from_rgb8(214, 153, 182),  // Magenta        #d699b6 (purple)
                Color::from_rgb8(131, 192, 146),  // Cyan           #83c092 (aqua)
                Color::from_rgb8(211, 198, 170),  // White          #d3c6aa (fg)
                Color::from_rgb8(71, 82, 88),     // Bright Black   #475258
                Color::from_rgb8(230, 126, 128),  // Bright Red     #e67e80
                Color::from_rgb8(167, 192, 128),  // Bright Green   #a7c080
                Color::from_rgb8(219, 188, 127),  // Bright Yellow  #dbbc7f
                Color::from_rgb8(127, 187, 179),  // Bright Blue    #7fbbb3
                Color::from_rgb8(214, 153, 182),  // Bright Magenta #d699b6
                Color::from_rgb8(131, 192, 146),  // Bright Cyan    #83c092
                Color::from_rgb8(211, 198, 170),  // Bright White   #d3c6aa
            ],
        }
    }

    /// Ayu Dark, from the ayu-theme org's own VSCode theme
    /// (terminal.ansi* keys; the widely-shipped alacritty port drifts
    /// slightly on the normal slots, the org values win). Cursor is
    /// the ayu accent.
    pub fn ayu_dark() -> Self {
        Self {
            foreground: Color::from_rgb8(191, 189, 182), // #bfbdb6
            background: Color::from_rgb8(13, 16, 23),    // #0d1017
            cursor: Color::from_rgb8(230, 180, 80),      // accent #e6b450
            ansi: [
                Color::from_rgb8(27, 31, 41),     // Black          #1b1f29
                Color::from_rgb8(240, 107, 115),  // Red            #f06b73
                Color::from_rgb8(112, 191, 86),   // Green          #70bf56
                Color::from_rgb8(253, 176, 76),   // Yellow         #fdb04c
                Color::from_rgb8(79, 191, 255),   // Blue           #4fbfff
                Color::from_rgb8(208, 161, 255),  // Magenta        #d0a1ff
                Color::from_rgb8(147, 226, 200),  // Cyan           #93e2c8
                Color::from_rgb8(199, 199, 199),  // White          #c7c7c7
                Color::from_rgb8(104, 104, 104),  // Bright Black   #686868
                Color::from_rgb8(240, 113, 120),  // Bright Red     #f07178
                Color::from_rgb8(170, 217, 76),   // Bright Green   #aad94c
                Color::from_rgb8(255, 180, 84),   // Bright Yellow  #ffb454
                Color::from_rgb8(89, 194, 255),   // Bright Blue    #59c2ff
                Color::from_rgb8(210, 166, 255),  // Bright Magenta #d2a6ff
                Color::from_rgb8(149, 230, 203),  // Bright Cyan    #95e6cb
                Color::from_rgb8(255, 255, 255),  // Bright White   #ffffff
            ],
        }
    }

    /// Ayu Light, same source as Ayu Dark. Bright white is DARKER than
    /// the near-white background by light-theme convention. The cursor
    /// is the accent the org's VSCode theme actually ships (#f29718).
    pub fn ayu_light() -> Self {
        Self {
            foreground: Color::from_rgb8(92, 97, 102),   // #5c6166
            background: Color::from_rgb8(248, 249, 250), // #f8f9fa
            cursor: Color::from_rgb8(242, 151, 24),      // accent #f29718
            ansi: [
                Color::from_rgb8(0, 0, 0),        // Black          #000000
                Color::from_rgb8(240, 107, 108),  // Red            #f06b6c
                Color::from_rgb8(108, 191, 67),   // Green          #6cbf43
                Color::from_rgb8(231, 161, 0),    // Yellow         #e7a100
                Color::from_rgb8(33, 161, 226),   // Blue           #21a1e2
                Color::from_rgb8(161, 118, 203),  // Magenta        #a176cb
                Color::from_rgb8(74, 188, 150),   // Cyan           #4abc96
                Color::from_rgb8(199, 199, 199),  // White          #c7c7c7
                Color::from_rgb8(104, 104, 104),  // Bright Black   #686868
                Color::from_rgb8(240, 113, 113),  // Bright Red     #f07171
                Color::from_rgb8(134, 179, 0),    // Bright Green   #86b300
                Color::from_rgb8(235, 164, 0),    // Bright Yellow  #eba400
                Color::from_rgb8(34, 164, 230),   // Bright Blue    #22a4e6
                Color::from_rgb8(163, 122, 204),  // Bright Magenta #a37acc
                Color::from_rgb8(76, 191, 153),   // Bright Cyan    #4cbf99
                Color::from_rgb8(209, 209, 209),  // Bright White   #d1d1d1
            ],
        }
    }

    /// GitHub Dark (Default), from primer/github-vscode-theme's
    /// terminal.ansi* keys (primitives 7.10, with the theme's own
    /// foreground override). No cursor upstream, so the foreground.
    pub fn github_dark() -> Self {
        Self {
            foreground: Color::from_rgb8(230, 237, 243), // #e6edf3
            background: Color::from_rgb8(13, 17, 23),    // #0d1117
            cursor: Color::from_rgb8(230, 237, 243),     // = foreground
            ansi: [
                Color::from_rgb8(72, 79, 88),     // Black          #484f58
                Color::from_rgb8(255, 123, 114),  // Red            #ff7b72
                Color::from_rgb8(63, 185, 80),    // Green          #3fb950
                Color::from_rgb8(210, 153, 34),   // Yellow         #d29922
                Color::from_rgb8(88, 166, 255),   // Blue           #58a6ff
                Color::from_rgb8(188, 140, 255),  // Magenta        #bc8cff
                Color::from_rgb8(57, 197, 207),   // Cyan           #39c5cf
                Color::from_rgb8(177, 186, 196),  // White          #b1bac4
                Color::from_rgb8(110, 118, 129),  // Bright Black   #6e7681
                Color::from_rgb8(255, 161, 152),  // Bright Red     #ffa198
                Color::from_rgb8(86, 211, 100),   // Bright Green   #56d364
                Color::from_rgb8(227, 179, 65),   // Bright Yellow  #e3b341
                Color::from_rgb8(121, 192, 255),  // Bright Blue    #79c0ff
                Color::from_rgb8(210, 168, 255),  // Bright Magenta #d2a8ff
                Color::from_rgb8(86, 212, 221),   // Bright Cyan    #56d4dd
                Color::from_rgb8(255, 255, 255),  // Bright White   #ffffff
            ],
        }
    }

    /// GitHub Light (Default), same source. Several "bright" slots are
    /// deliberately DARKER than their normal twin (bright-as-bold must
    /// stay readable on white), the yellows are dark browns, and both
    /// whites are mid greys: faithful upstream, not an error.
    pub fn github_light() -> Self {
        Self {
            foreground: Color::from_rgb8(31, 35, 40),    // #1f2328
            background: Color::from_rgb8(255, 255, 255), // #ffffff
            cursor: Color::from_rgb8(31, 35, 40),        // = foreground
            ansi: [
                Color::from_rgb8(36, 41, 47),     // Black          #24292f
                Color::from_rgb8(207, 34, 46),    // Red            #cf222e
                Color::from_rgb8(17, 99, 41),     // Green          #116329
                Color::from_rgb8(77, 45, 0),      // Yellow         #4d2d00
                Color::from_rgb8(9, 105, 218),    // Blue           #0969da
                Color::from_rgb8(130, 80, 223),   // Magenta        #8250df
                Color::from_rgb8(27, 124, 131),   // Cyan           #1b7c83
                Color::from_rgb8(110, 119, 129),  // White          #6e7781
                Color::from_rgb8(87, 96, 106),    // Bright Black   #57606a
                Color::from_rgb8(164, 14, 38),    // Bright Red     #a40e26
                Color::from_rgb8(26, 127, 55),    // Bright Green   #1a7f37
                Color::from_rgb8(99, 60, 1),      // Bright Yellow  #633c01
                Color::from_rgb8(33, 139, 255),   // Bright Blue    #218bff
                Color::from_rgb8(164, 117, 249),  // Bright Magenta #a475f9
                Color::from_rgb8(49, 146, 170),   // Bright Cyan    #3192aa
                Color::from_rgb8(140, 149, 159),  // Bright White   #8c959f
            ],
        }
    }

    /// One Light, Atom's light half, from the canonical terminal port's
    /// own hex table (nathanbuchar/atom-one-dark-terminal COLORS, which
    /// is verbatim atom/one-light-syntax; its .itermcolors drifts a few
    /// units through colorspace-less floats and the mbadolato port is a
    /// different, cruder palette). Bright 9-14 repeat normal 1-6;
    /// bright black is the foreground.
    pub fn one_light() -> Self {
        Self {
            foreground: Color::from_rgb8(56, 58, 66),    // mono-1 #383a42
            background: Color::from_rgb8(249, 249, 249), // #f9f9f9
            cursor: Color::from_rgb8(56, 58, 66),        // = foreground (explicit upstream)
            ansi: [
                Color::from_rgb8(0, 0, 0),        // Black          #000000
                Color::from_rgb8(228, 86, 73),    // Red            #e45649
                Color::from_rgb8(80, 161, 79),    // Green          #50a14f
                Color::from_rgb8(152, 104, 1),    // Yellow         #986801
                Color::from_rgb8(64, 120, 242),   // Blue           #4078f2
                Color::from_rgb8(166, 38, 164),   // Magenta        #a626a4
                Color::from_rgb8(1, 132, 188),    // Cyan           #0184bc
                Color::from_rgb8(160, 161, 167),  // White          #a0a1a7
                Color::from_rgb8(56, 58, 66),     // Bright Black   #383a42
                Color::from_rgb8(228, 86, 73),    // Bright Red     #e45649
                Color::from_rgb8(80, 161, 79),    // Bright Green   #50a14f
                Color::from_rgb8(152, 104, 1),    // Bright Yellow  #986801
                Color::from_rgb8(64, 120, 242),   // Bright Blue    #4078f2
                Color::from_rgb8(166, 38, 164),   // Bright Magenta #a626a4
                Color::from_rgb8(1, 132, 188),    // Bright Cyan    #0184bc
                Color::from_rgb8(255, 255, 255),  // Bright White   #ffffff
            ],
        }
    }

    /// Gruvbox Light (medium contrast), from morhetz's own xresources
    /// export next to the vim palette. The light-mode inversion is
    /// upstream-intended: slot 0 "black" is a cream tone, slot 15
    /// "bright white" is the DARK foreground, and the bright 9-14 row
    /// is the faded_* set, darker than the neutral row. Slot 6/14 are
    /// gruvbox "aqua" serving as cyan. No cursor upstream, so the
    /// foreground.
    pub fn gruvbox_light() -> Self {
        Self {
            foreground: Color::from_rgb8(60, 56, 54),    // dark1 #3c3836
            background: Color::from_rgb8(251, 241, 199), // light0 #fbf1c7
            cursor: Color::from_rgb8(60, 56, 54),        // = foreground
            ansi: [
                Color::from_rgb8(253, 244, 193),  // Black          #fdf4c1
                Color::from_rgb8(204, 36, 29),    // Red            #cc241d
                Color::from_rgb8(152, 151, 26),   // Green          #98971a
                Color::from_rgb8(215, 153, 33),   // Yellow         #d79921
                Color::from_rgb8(69, 133, 136),   // Blue           #458588
                Color::from_rgb8(177, 98, 134),   // Magenta        #b16286
                Color::from_rgb8(104, 157, 106),  // Cyan           #689d6a (aqua)
                Color::from_rgb8(124, 111, 100),  // White          #7c6f64 (dark4)
                Color::from_rgb8(146, 131, 116),  // Bright Black   #928374 (gray)
                Color::from_rgb8(157, 0, 6),      // Bright Red     #9d0006 (faded)
                Color::from_rgb8(121, 116, 14),   // Bright Green   #79740e (faded)
                Color::from_rgb8(181, 118, 20),   // Bright Yellow  #b57614 (faded)
                Color::from_rgb8(7, 102, 120),    // Bright Blue    #076678 (faded)
                Color::from_rgb8(143, 63, 113),   // Bright Magenta #8f3f71 (faded)
                Color::from_rgb8(66, 123, 88),    // Bright Cyan    #427b58 (faded)
                Color::from_rgb8(60, 56, 54),     // Bright White   #3c3836 (dark1)
            ],
        }
    }

    /// Zenburn, via the mbadolato/iTerm2-Color-Schemes alacritty export
    /// (the de-facto terminal mapping; the original vim scheme's bg/fg
    /// agree). Bright black is Zenburn's famous green-grey and the
    /// bright green/yellow are muted tans: faithful, not an error.
    /// The port's #73635a cursor measures 1.8:1 against the background,
    /// under this crate's own visibility floor, and the ORIGINAL scheme
    /// leaves the cursor to reverse video, so the foreground stands in.
    pub fn zenburn() -> Self {
        Self {
            foreground: Color::from_rgb8(220, 220, 204), // #dcdccc
            background: Color::from_rgb8(63, 63, 63),    // #3f3f3f
            cursor: Color::from_rgb8(220, 220, 204),     // = foreground (see above)
            ansi: [
                Color::from_rgb8(77, 77, 77),     // Black          #4d4d4d
                Color::from_rgb8(125, 93, 93),    // Red            #7d5d5d
                Color::from_rgb8(96, 180, 138),   // Green          #60b48a
                Color::from_rgb8(240, 223, 175),  // Yellow         #f0dfaf
                Color::from_rgb8(93, 109, 125),   // Blue           #5d6d7d
                Color::from_rgb8(220, 140, 195),  // Magenta        #dc8cc3
                Color::from_rgb8(140, 208, 211),  // Cyan           #8cd0d3
                Color::from_rgb8(220, 220, 204),  // White          #dcdccc
                Color::from_rgb8(112, 144, 128),  // Bright Black   #709080
                Color::from_rgb8(220, 163, 163),  // Bright Red     #dca3a3
                Color::from_rgb8(195, 191, 159),  // Bright Green   #c3bf9f
                Color::from_rgb8(224, 207, 159),  // Bright Yellow  #e0cf9f
                Color::from_rgb8(148, 191, 243),  // Bright Blue    #94bff3
                Color::from_rgb8(236, 147, 211),  // Bright Magenta #ec93d3
                Color::from_rgb8(147, 224, 227),  // Bright Cyan    #93e0e3
                Color::from_rgb8(255, 255, 255),  // Bright White   #ffffff
            ],
        }
    }

    /// Horizon (dark), from the official VSCode theme's terminal keys.
    /// Upstream defines only the 12 coloured slots; the black/white
    /// four come from the faithful Windows Terminal extension of the
    /// same values (black = background, whites = foreground; there is
    /// no #ffffff anywhere in Horizon). The "yellow" slots are
    /// peach/salmon by design. The upstream cursor is a translucent
    /// grey; its opaque base colour is used here.
    pub fn horizon() -> Self {
        Self {
            foreground: Color::from_rgb8(213, 216, 218), // #d5d8da
            background: Color::from_rgb8(28, 30, 38),    // #1c1e26
            cursor: Color::from_rgb8(108, 111, 147),     // #6c6f93
            ansi: [
                Color::from_rgb8(28, 30, 38),     // Black          #1c1e26 (= background)
                Color::from_rgb8(233, 86, 120),   // Red            #e95678
                Color::from_rgb8(41, 211, 152),   // Green          #29d398
                Color::from_rgb8(250, 183, 149),  // Yellow         #fab795
                Color::from_rgb8(38, 187, 217),   // Blue           #26bbd9
                Color::from_rgb8(238, 100, 172),  // Magenta        #ee64ac
                Color::from_rgb8(89, 225, 227),   // Cyan           #59e1e3
                Color::from_rgb8(213, 216, 218),  // White          #d5d8da (= foreground)
                Color::from_rgb8(46, 48, 62),     // Bright Black   #2e303e
                Color::from_rgb8(236, 106, 136),  // Bright Red     #ec6a88
                Color::from_rgb8(63, 218, 164),   // Bright Green   #3fdaa4
                Color::from_rgb8(251, 195, 167),  // Bright Yellow  #fbc3a7
                Color::from_rgb8(63, 196, 222),   // Bright Blue    #3fc4de
                Color::from_rgb8(240, 117, 181),  // Bright Magenta #f075b5
                Color::from_rgb8(107, 228, 230),  // Bright Cyan    #6be4e6
                Color::from_rgb8(213, 216, 218),  // Bright White   #d5d8da
            ],
        }
    }

    /// Resolve an alacritty Color to an iced Color.
    pub fn resolve(
        &self,
        color: &ansi::Color,
        term_colors: &alacritty_terminal::term::color::Colors,
    ) -> Color {
        match color {
            ansi::Color::Named(named) => self.resolve_named(*named, term_colors),
            ansi::Color::Spec(rgb) => Color::from_rgb8(rgb.r, rgb.g, rgb.b),
            ansi::Color::Indexed(idx) => {
                if let Some(rgb) = term_colors[*idx as usize] {
                    Color::from_rgb8(rgb.r, rgb.g, rgb.b)
                } else if (*idx as usize) < 16 {
                    self.ansi[*idx as usize]
                } else {
                    self.color_from_256(*idx)
                }
            }
        }
    }

    fn resolve_named(
        &self,
        named: NamedColor,
        term_colors: &alacritty_terminal::term::color::Colors,
    ) -> Color {
        let idx = named as usize;
        if let Some(rgb) = term_colors[idx] {
            return Color::from_rgb8(rgb.r, rgb.g, rgb.b);
        }

        match named {
            NamedColor::Black => self.ansi[0],
            NamedColor::Red => self.ansi[1],
            NamedColor::Green => self.ansi[2],
            NamedColor::Yellow => self.ansi[3],
            NamedColor::Blue => self.ansi[4],
            NamedColor::Magenta => self.ansi[5],
            NamedColor::Cyan => self.ansi[6],
            NamedColor::White => self.ansi[7],
            NamedColor::BrightBlack => self.ansi[8],
            NamedColor::BrightRed => self.ansi[9],
            NamedColor::BrightGreen => self.ansi[10],
            NamedColor::BrightYellow => self.ansi[11],
            NamedColor::BrightBlue => self.ansi[12],
            NamedColor::BrightMagenta => self.ansi[13],
            NamedColor::BrightCyan => self.ansi[14],
            NamedColor::BrightWhite => self.ansi[15],
            NamedColor::Foreground | NamedColor::BrightForeground => self.foreground,
            NamedColor::Background => self.background,
            NamedColor::Cursor => self.cursor,
            NamedColor::DimBlack => dim(self.ansi[0]),
            NamedColor::DimRed => dim(self.ansi[1]),
            NamedColor::DimGreen => dim(self.ansi[2]),
            NamedColor::DimYellow => dim(self.ansi[3]),
            NamedColor::DimBlue => dim(self.ansi[4]),
            NamedColor::DimMagenta => dim(self.ansi[5]),
            NamedColor::DimCyan => dim(self.ansi[6]),
            NamedColor::DimWhite => dim(self.ansi[7]),
            _ => self.foreground,
        }
    }

    fn color_from_256(&self, idx: u8) -> Color {
        if idx < 16 {
            return self.ansi[idx as usize];
        }
        if idx >= 232 {
            let value = ((idx - 232) as f32 * 10.0 + 8.0) / 255.0;
            return Color::from_rgb(value, value, value);
        }
        let idx = idx - 16;
        let r = (idx / 36) % 6;
        let g = (idx / 6) % 6;
        let b = idx % 6;
        let to_f = |v: u8| if v == 0 { 0.0 } else { (v as f32 * 40.0 + 55.0) / 255.0 };
        Color::from_rgb(to_f(r), to_f(g), to_f(b))
    }
}

fn dim(color: Color) -> Color {
    Color::from_rgba(color.r * 0.66, color.g * 0.66, color.b * 0.66, color.a)
}


#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance, and the contrast ratio built on it.
    /// Local to the tests: the app has its own copy for UI colours
    /// (`oryxis-app/src/theme.rs`), and this crate has no other use for
    /// it, so a second five-line helper beats a new dependency edge.
    fn luminance(c: Color) -> f32 {
        let channel = |v: f32| {
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
    }

    fn contrast(a: Color, b: Color) -> f32 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// Plain text has to be readable in every palette we ship.
    ///
    /// SECOND CONSUMER: `scripts/gen_theme_index.py` measures community
    /// submissions against this same 4.0 (and the 2.0 below), except
    /// that it LABELS rather than refuses. The numbers are duplicated
    /// across a language boundary; moving one means moving the other.
    ///
    /// The bar is 4.0, just under WCAG AA (4.5), on purpose: canonical
    /// Solarized Light lands at 4.13 and is shipped faithfully rather
    /// than "corrected", so a 4.5 bar would either fail a palette we
    /// deliberately keep authentic or push us into editing someone
    /// else's design. What this catches is the real failure mode, a new
    /// palette whose text is genuinely hard to read (or whose foreground
    /// and background got typed the wrong way round).
    #[test]
    fn text_is_readable_in_every_builtin_palette() {
        for theme in TerminalTheme::ALL {
            let p = theme.palette();
            let ratio = contrast(p.foreground, p.background);
            assert!(
                ratio >= 4.0,
                "{}: foreground on background is only {ratio:.2}:1",
                theme.name()
            );
        }
    }

    /// The cursor is a filled block, so it needs far less contrast than
    /// text to be spotted, but it cannot vanish into the background.
    /// Night Owl Light sits lowest at 2.33:1, which reads fine as a
    /// block; the bar sits below it so upstream palettes stay faithful
    /// and an actually invisible cursor still fails.
    #[test]
    fn the_cursor_is_visible_in_every_builtin_palette() {
        for theme in TerminalTheme::ALL {
            let p = theme.palette();
            let ratio = contrast(p.cursor, p.background);
            assert!(
                ratio >= 2.0,
                "{}: cursor on background is only {ratio:.2}:1",
                theme.name()
            );
        }
    }

    /// Note deliberately NOT tested: contrast of the 16 ANSI slots
    /// against the background. Slot 0 is meant to be near-black on a
    /// dark theme and slot 15 near-white on a light one, so most
    /// palettes measure ~1:1 there by design. Asserting on those would
    /// fail every faithful port in the set.
    ///
    /// Names are the real coupling: settings persist the theme as a
    /// STRING (`terminal_theme`, the per-host override, the `.cast`
    /// exporter), and `terminal_palette_for_name` resolves it by
    /// scanning this list, so two entries sharing a name would make the
    /// user's saved choice ambiguous.
    #[test]
    fn builtin_names_are_unique_and_non_empty() {
        let mut seen: Vec<&str> = Vec::new();
        for theme in TerminalTheme::ALL {
            let name = theme.name();
            assert!(!name.trim().is_empty(), "{theme:?} has a blank name");
            assert!(
                !seen.contains(&name),
                "two built-in themes are both called {name:?}"
            );
            seen.push(name);
        }
    }

    /// `ALL` is the picker order: the dark group, then the light group,
    /// alphabetical by display name within each. Group membership is
    /// MEASURED (background luminance), not hand-tagged, so a new theme
    /// slotted into the wrong half (or dropped at the end of the list,
    /// the natural mistake) fails here with its name.
    #[test]
    fn list_order_is_dark_then_light_alphabetical() {
        let is_light = |t: &TerminalTheme| luminance(t.palette().background) >= 0.5;
        let mut seen_light = false;
        let mut prev: Option<(bool, String)> = None;
        for theme in TerminalTheme::ALL {
            let light = is_light(theme);
            assert!(
                !(seen_light && !light),
                "{}: dark theme listed after the light group started",
                theme.name()
            );
            seen_light |= light;
            let key = theme.name().to_lowercase();
            if let Some((prev_light, prev_key)) = &prev
                && *prev_light == light
            {
                assert!(
                    *prev_key < key,
                    "{}: out of alphabetical order within its group",
                    theme.name()
                );
            }
            prev = Some((light, key));
        }
    }

    /// Night Owl's dark and light halves are read from DIFFERENT keys of
    /// the upstream theme files (the light one declares
    /// `terminal.background` / `terminal.foreground`, the dark one only
    /// `editor.*`), which is the kind of asymmetry someone tidies up by
    /// mistake. Pin both against the published values.
    #[test]
    fn night_owl_matches_the_published_palette() {
        let dark = TerminalPalette::night_owl();
        assert_eq!(dark.background, Color::from_rgb8(1, 22, 39), "editor.background #011627");
        assert_eq!(dark.foreground, Color::from_rgb8(214, 222, 235), "editor.foreground #d6deeb");
        assert_eq!(dark.ansi[5], Color::from_rgb8(199, 146, 234), "ansiMagenta #C792EA");
        assert_eq!(dark.ansi[14], Color::from_rgb8(127, 219, 202), "ansiBrightCyan #7fdbca");

        let light = TerminalPalette::night_owl_light();
        assert_eq!(light.background, Color::from_rgb8(246, 246, 246), "terminal.background #F6F6F6");
        assert_eq!(light.foreground, Color::from_rgb8(64, 63, 83), "terminal.foreground #403f53");
        // Upstream really does repeat these two in the bright slots.
        assert_eq!(light.ansi[0], light.ansi[8], "ansiBrightBlack repeats ansiBlack upstream");
        assert_eq!(light.ansi[4], light.ansi[12], "ansiBrightBlue repeats ansiBlue upstream");
    }
}
