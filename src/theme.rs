use iced::Color;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub background: Color,
    pub foreground: Color,
    pub gutter: Color,
    pub selection: Color,
    pub keyword: Color,
    pub string: Color,
    pub number: Color,
    pub comment: Color,
    pub type_name: Color,
}

fn hex(s: u32) -> Color {
    Color::from_rgb8((s >> 16) as u8, (s >> 8) as u8, s as u8)
}

impl Palette {
    pub fn monokai() -> Self {
        Self {
            background: hex(0x272822),
            foreground: hex(0xf8f8f2),
            gutter: hex(0x90908a),
            selection: hex(0x49483e),
            keyword: hex(0xf92672),
            string: hex(0xe6db74),
            number: hex(0xae81ff),
            comment: hex(0x75715e),
            type_name: hex(0x66d9ef),
        }
    }
}

/// Palette for the editor. Reading `~/.config/foot/foot.ini` is disabled for now,
/// so this is always monokai; the original implementation is kept below.
pub fn load() -> Palette {
    Palette::monokai()
}

/*
/// Palette from `~/.config/foot/foot.ini`, or monokai when the file is
/// absent or cannot be parsed (the error goes to stderr).
pub fn load() -> Palette {
    let Some(home) = home_dir() else {
        return Palette::monokai();
    };
    let path = home.join(".config/foot/foot.ini");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Palette::monokai(),
        Err(e) => {
            eprintln!("rite: cannot read {}: {e}", path.display());
            return Palette::monokai();
        }
    };
    match parse_foot(&text) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("rite: cannot parse {}: {e}; using monokai", path.display());
            Palette::monokai()
        }
    }
}

*/

#[allow(dead_code)]
fn parse_foot(text: &str) -> Result<Palette, String> {
    let mut in_colors = false;
    let mut found: Vec<(String, Color)> = Vec::new();

    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_colors = matches!(section.trim(), "colors" | "colors-dark");
            continue;
        }
        if !in_colors {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("line {}: expected key=value", n + 1));
        };
        let key = key.trim();
        // Foot allows trailing comments after the value.
        let value = value.split(['#', ';']).next().unwrap_or("").trim();
        let value = value.trim_start_matches('#');
        // Keys may carry an alpha suffix in some entries (`aarrggbb`); only plain RRGGBB is used here.
        if value.len() != 6 {
            if matches!(key, "background" | "foreground") || key.starts_with("regular") || key.starts_with("bright") {
                return Err(format!("line {}: invalid color `{value}` for `{key}`", n + 1));
            }
            continue;
        }
        let rgb = u32::from_str_radix(value, 16)
            .map_err(|_| format!("line {}: invalid color `{value}` for `{key}`", n + 1))?;
        found.push((key.to_string(), hex(rgb)));
    }

    let get = |k: &str| {
        found
            .iter()
            .rev()
            .find(|(key, _)| key == k)
            .map(|(_, c)| *c)
            .ok_or_else(|| format!("missing `{k}` in [colors]"))
    };

    // Validate that the whole expected set is present.
    for i in 0..8 {
        get(&format!("regular{i}"))?;
        get(&format!("bright{i}"))?;
    }

    Ok(Palette {
        background: get("background")?,
        foreground: get("foreground")?,
        gutter: get("bright3")?,
        selection: get("bright0")?,
        keyword: get("regular1")?,
        string: get("regular2")?,
        number: get("regular5")?,
        comment: get("bright2")?,
        type_name: get("regular4")?,
    })
}
