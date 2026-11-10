use crate::theme::Palette;
use iced::Color;
use iced::advanced::text::highlighter::{self, Format};
use std::ops::Range;

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "case", "catch", "class", "const", "continue", "def", "default",
    "defer", "do", "elif", "else", "enum", "except", "export", "extends", "fn", "finally", "for",
    "func", "function", "go", "if", "impl", "import", "in", "interface", "is", "lambda", "let", "loop",
    "match", "mod", "mut", "namespace", "new", "not", "or", "and", "package", "pass", "pub", "raise",
    "return", "self", "Self", "static", "struct", "super", "switch", "this", "throw", "trait", "try",
    "type", "typeof", "use", "var", "void", "while", "with", "yield", "where", "unsafe", "extern",
    "crate", "move", "ref", "dyn", "fi", "then", "esac", "done", "local",
];

const LITERALS: &[&str] = &["true", "false", "null", "nil", "None", "True", "False", "undefined"];

const TYPES: &[&str] = &[
    "bool", "char", "str", "string", "int", "float", "double", "long", "short", "byte", "usize",
    "isize", "u8", "u16", "u32", "u64", "u128", "i8", "i16", "i32", "i64", "i128", "f32", "f64",
    "String", "Vec", "Option", "Result", "Box",
];

/// A small per-line tokenizer. It carries no state across lines, so
/// multi-line block comments and strings are not recognised.
pub struct Syntax {
    palette: Palette,
    line: usize,
}

impl highlighter::Highlighter for Syntax {
    type Settings = Palette;
    type Highlight = Color;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, Color)>;

    fn new(settings: &Palette) -> Self {
        Self { palette: *settings, line: 0 }
    }

    fn update(&mut self, new_settings: &Palette) {
        self.palette = *new_settings;
    }

    fn change_line(&mut self, line: usize) {
        self.line = self.line.min(line);
    }

    fn highlight_line(&mut self, line: &str) -> Self::Iterator<'_> {
        self.line += 1;
        tokenize(line, &self.palette).into_iter()
    }

    fn current_line(&self) -> usize {
        self.line
    }
}

pub fn format(color: &Color, _theme: &iced::Theme) -> Format<iced::Font> {
    Format { color: Some(*color), font: None }
}

fn tokenize(line: &str, p: &Palette) -> Vec<(Range<usize>, Color)> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        let c = bytes[i];
        let rest = &line[i..];

        if rest.starts_with("//") || c == b'#' || rest.starts_with("--") {
            out.push((i..line.len(), p.comment));
            break;
        }

        if c == b'"' || c == b'\'' || c == b'`' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != c {
                if bytes[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            let end = (j + 1).min(line.len());
            // A lone `'` is usually a Rust lifetime; leave it uncolored.
            if c == b'\'' && j >= bytes.len() {
                i += 1;
                continue;
            }
            out.push((i..end, p.string));
            i = end;
            continue;
        }

        if c.is_ascii_digit() {
            let mut j = i;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'.' || bytes[j] == b'_') {
                j += 1;
            }
            out.push((i..j, p.number));
            i = j;
            continue;
        }

        if c.is_ascii_alphabetic() || c == b'_' {
            let mut j = i;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            let word = &line[i..j];
            if KEYWORDS.contains(&word) {
                out.push((i..j, p.keyword));
            } else if LITERALS.contains(&word) {
                out.push((i..j, p.number));
            } else if TYPES.contains(&word) || word.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                out.push((i..j, p.type_name));
            }
            i = j;
            continue;
        }

        // Skip a full UTF-8 character.
        i += rest.chars().next().map_or(1, char::len_utf8);
    }

    out
}
