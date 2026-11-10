use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct Config {
    pub font_name: String,
    /// Font size in points.
    pub font_size: FontSize,
    pub line_height: f32,
    /// Initial window size in pixels.
    pub window_width: f32,
    pub window_height: f32,
}

/// Accepts both `11` and `"11"` in the config file.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(untagged)]
pub enum FontSize {
    Number(f32),
    Text(StringF32),
}

#[derive(Debug, Clone, Copy)]
pub struct StringF32(f32);

impl<'de> Deserialize<'de> for StringF32 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.trim().parse().map(StringF32).map_err(serde::de::Error::custom)
    }
}

impl FontSize {
    pub fn points(self) -> f32 {
        match self {
            FontSize::Number(n) => n,
            FontSize::Text(StringF32(n)) => n,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font_name: "JetBrains Mono".into(),
            font_size: FontSize::Number(11.0),
            line_height: 1.8,
            window_width: 1000.0,
            window_height: 600.0,
        }
    }
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Loads `~/.config/rite/config.json`. A missing file yields the defaults;
/// an unreadable or invalid file is reported on stderr and also yields defaults.
pub fn load() -> Config {
    let Some(home) = home_dir() else {
        return Config::default();
    };
    let path = home.join(".config/rite/config.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Config::default(),
        Err(e) => {
            eprintln!("rite: cannot read {}: {e}", path.display());
            return Config::default();
        }
    };
    match serde_json::from_str(&text) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rite: invalid config {}: {e}", path.display());
            Config::default()
        }
    }
}
