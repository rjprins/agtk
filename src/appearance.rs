use serde::{Deserialize, Serialize};

pub const DEFAULT_UI_FONT_SIZE: u8 = 13;
pub const MIN_UI_FONT_SIZE: u8 = 9;
pub const MAX_UI_FONT_SIZE: u8 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeKey {
    Neutral,
    NeutralLight,
    Dracula,
    TokyoNight,
    SolarizedDark,
    SolarizedLight,
    Light,
}

impl ThemeKey {
    pub const ALL: [Self; 7] = [
        Self::Neutral,
        Self::NeutralLight,
        Self::Dracula,
        Self::TokyoNight,
        Self::SolarizedDark,
        Self::SolarizedLight,
        Self::Light,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::NeutralLight => "neutral-light",
            Self::Dracula => "dracula",
            Self::TokyoNight => "tokyo-night",
            Self::SolarizedDark => "solarized-dark",
            Self::SolarizedLight => "solarized-light",
            Self::Light => "light",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppearancePreferences {
    pub theme: ThemeKey,
    pub follow_system: bool,
    pub font: String,
    #[serde(default = "default_ui_font_size")]
    pub ui_font_size: u8,
}

impl Default for AppearancePreferences {
    fn default() -> Self {
        Self {
            theme: ThemeKey::Neutral,
            follow_system: true,
            font: "Monospace 11".to_owned(),
            ui_font_size: DEFAULT_UI_FONT_SIZE,
        }
    }
}

const fn default_ui_font_size() -> u8 {
    DEFAULT_UI_FONT_SIZE
}

pub const fn clamp_ui_font_size(size: u8) -> u8 {
    if size < MIN_UI_FONT_SIZE {
        MIN_UI_FONT_SIZE
    } else if size > MAX_UI_FONT_SIZE {
        MAX_UI_FONT_SIZE
    } else {
        size
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub key: ThemeKey,
    pub name: &'static str,
    pub is_dark: bool,
    pub chrome: ChromePalette,
    pub terminal: TerminalPalette,
}

#[derive(Debug, Clone, Copy)]
pub struct ChromePalette {
    pub background: &'static str,
    pub panel: &'static str,
    pub text: &'static str,
    pub muted: &'static str,
    pub accent: &'static str,
    pub danger: &'static str,
    pub line: &'static str,
    pub hover: &'static str,
    pub ready: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct TerminalPalette {
    pub background: &'static str,
    pub foreground: &'static str,
    pub cursor: &'static str,
    pub selection: &'static str,
    pub ansi: [&'static str; 16],
}

pub const fn theme(key: ThemeKey) -> &'static Theme {
    match key {
        ThemeKey::Neutral => &NEUTRAL,
        ThemeKey::NeutralLight => &NEUTRAL_LIGHT,
        ThemeKey::Dracula => &DRACULA,
        ThemeKey::TokyoNight => &TOKYO_NIGHT,
        ThemeKey::SolarizedDark => &SOLARIZED_DARK,
        ThemeKey::SolarizedLight => &SOLARIZED_LIGHT,
        ThemeKey::Light => &LIGHT,
    }
}

pub const fn resolve_system_theme(key: ThemeKey, prefers_dark: bool) -> ThemeKey {
    let dark = match key {
        ThemeKey::NeutralLight => ThemeKey::Neutral,
        ThemeKey::SolarizedLight => ThemeKey::SolarizedDark,
        other => other,
    };
    if prefers_dark {
        dark
    } else {
        match dark {
            ThemeKey::Neutral => ThemeKey::NeutralLight,
            ThemeKey::Dracula | ThemeKey::TokyoNight => ThemeKey::Light,
            ThemeKey::SolarizedDark => ThemeKey::SolarizedLight,
            other => other,
        }
    }
}

const NEUTRAL: Theme = Theme {
    key: ThemeKey::Neutral,
    name: "Neutral",
    is_dark: true,
    chrome: ChromePalette {
        background: "#0a0a0a",
        panel: "#111111",
        text: "#e5e5e5",
        muted: "#737373",
        accent: "#3b82f6",
        danger: "#ef4444",
        line: "rgba(255,255,255,0.10)",
        hover: "rgba(255,255,255,0.08)",
        ready: "#22c55e",
    },
    terminal: TerminalPalette {
        background: "#2d2d2d",
        foreground: "#d4d4d4",
        cursor: "#3b82f6",
        selection: "rgba(59,130,246,0.35)",
        ansi: [
            "#1f1f1f", "#f87171", "#4ade80", "#fbbf24", "#60a5fa", "#c084fc", "#22d3ee", "#d4d4d4",
            "#6b7280", "#fca5a5", "#86efac", "#fde047", "#93c5fd", "#d8b4fe", "#67e8f9", "#ffffff",
        ],
    },
};

const NEUTRAL_LIGHT: Theme = Theme {
    key: ThemeKey::NeutralLight,
    name: "Neutral Light",
    is_dark: false,
    chrome: ChromePalette {
        background: "#fafafa",
        panel: "#ffffff",
        text: "#171717",
        muted: "#737373",
        accent: "#2563eb",
        danger: "#dc2626",
        line: "rgba(0,0,0,0.12)",
        hover: "rgba(0,0,0,0.07)",
        ready: "#16a34a",
    },
    terminal: TerminalPalette {
        background: "#fafafa",
        foreground: "#171717",
        cursor: "#2563eb",
        selection: "rgba(37,99,235,0.20)",
        ansi: [
            "#171717", "#dc2626", "#16a34a", "#ca8a04", "#2563eb", "#9333ea", "#0891b2", "#f5f5f5",
            "#737373", "#ef4444", "#22c55e", "#eab308", "#3b82f6", "#a855f7", "#06b6d4", "#ffffff",
        ],
    },
};

const DRACULA: Theme = Theme {
    key: ThemeKey::Dracula,
    name: "Dracula",
    is_dark: true,
    chrome: ChromePalette {
        background: "#0b0e14",
        panel: "#101626",
        text: "#e7ecff",
        muted: "#99a2c2",
        accent: "#ffcc66",
        danger: "#ff5f87",
        line: "rgba(255,255,255,0.10)",
        hover: "#1a2440",
        ready: "#5fd18c",
    },
    terminal: TerminalPalette {
        background: "#0b0e14",
        foreground: "#e7ecff",
        cursor: "#ffcc66",
        selection: "rgba(255,204,102,0.25)",
        ansi: [
            "#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2",
            "#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff",
        ],
    },
};

const TOKYO_NIGHT: Theme = Theme {
    key: ThemeKey::TokyoNight,
    name: "Tokyo Night",
    is_dark: true,
    chrome: ChromePalette {
        background: "#1a1b26",
        panel: "#16161e",
        text: "#c0caf5",
        muted: "#565f89",
        accent: "#7aa2f7",
        danger: "#f7768e",
        line: "rgba(255,255,255,0.09)",
        hover: "rgba(255,255,255,0.07)",
        ready: "#9ece6a",
    },
    terminal: TerminalPalette {
        background: "#1a1b26",
        foreground: "#c0caf5",
        cursor: "#7aa2f7",
        selection: "rgba(122,162,247,0.25)",
        ansi: [
            "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6",
            "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5",
        ],
    },
};

const SOLARIZED_DARK: Theme = Theme {
    key: ThemeKey::SolarizedDark,
    name: "Solarized Dark",
    is_dark: true,
    chrome: ChromePalette {
        background: "#002b36",
        panel: "#073642",
        text: "#839496",
        muted: "#586e75",
        accent: "#b58900",
        danger: "#dc322f",
        line: "rgba(255,255,255,0.10)",
        hover: "rgba(255,255,255,0.07)",
        ready: "#859900",
    },
    terminal: TerminalPalette {
        background: "#002b36",
        foreground: "#839496",
        cursor: "#b58900",
        selection: "rgba(181,137,0,0.25)",
        ansi: [
            "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5",
            "#586e75", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3",
        ],
    },
};

const SOLARIZED_LIGHT: Theme = Theme {
    key: ThemeKey::SolarizedLight,
    name: "Solarized Light",
    is_dark: false,
    chrome: ChromePalette {
        background: "#fdf6e3",
        panel: "#eee8d5",
        text: "#657b83",
        muted: "#93a1a1",
        accent: "#b58900",
        danger: "#dc322f",
        line: "rgba(0,0,0,0.12)",
        hover: "rgba(0,0,0,0.07)",
        ready: "#859900",
    },
    terminal: TerminalPalette {
        background: "#fdf6e3",
        foreground: "#657b83",
        cursor: "#b58900",
        selection: "rgba(181,137,0,0.20)",
        ansi: [
            "#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5",
            "#586e75", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3",
        ],
    },
};

const LIGHT: Theme = Theme {
    key: ThemeKey::Light,
    name: "Light",
    is_dark: false,
    chrome: ChromePalette {
        background: "#ffffff",
        panel: "#f5f5f5",
        text: "#24292e",
        muted: "#6a737d",
        accent: "#0366d6",
        danger: "#d73a49",
        line: "rgba(0,0,0,0.12)",
        hover: "rgba(0,0,0,0.07)",
        ready: "#28a745",
    },
    terminal: TerminalPalette {
        background: "#ffffff",
        foreground: "#24292e",
        cursor: "#0366d6",
        selection: "rgba(3,102,214,0.20)",
        ansi: [
            "#24292e", "#d73a49", "#28a745", "#dbab09", "#0366d6", "#5a32a3", "#0598bc", "#e1e4e8",
            "#6a737d", "#cb2431", "#22863a", "#b08800", "#005cc5", "#5a32a3", "#3192aa", "#fafbfc",
        ],
    },
};
