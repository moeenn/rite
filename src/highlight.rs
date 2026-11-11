use crate::theme::Palette;
use iced::Color;
use iced::advanced::text::highlighter::{self, Format};
use std::ops::Range;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Lang {
    Rust,
    C,
    Cpp,
    CSharp,
    Java,
    Kotlin,
    Swift,
    Go,
    JavaScript,
    TypeScript,
    Python,
    Ruby,
    Lua,
    Php,
    Shell,
    Fish,
    Sql,
    Toml,
    Yaml,
    Json,
}

/// What the tokenizer needs to know about a language.
struct Spec {
    keywords: &'static [&'static str],
    literals: &'static [&'static str],
    types: &'static [&'static str],
    line_comments: &'static [&'static str],
    block_comments: bool,
    /// Treat capitalised identifiers as type names.
    capitalized_types: bool,
    /// `'` starts a string (false for Rust, where it is mostly a lifetime).
    single_quotes: bool,
}

const C_LIKE_LITERALS: &[&str] = &["true", "false", "null", "nullptr", "NULL", "nil", "undefined"];
const C_TYPES: &[&str] = &[
    "int", "long", "short", "char", "float", "double", "void", "bool", "unsigned", "signed", "size_t",
    "uint8_t", "uint16_t", "uint32_t", "uint64_t", "int8_t", "int16_t", "int32_t", "int64_t", "byte",
    "string", "boolean", "var",
];

impl Lang {
    fn spec(self) -> Spec {
        let c_like = |keywords, types, capitalized_types| Spec {
            keywords,
            literals: C_LIKE_LITERALS,
            types,
            line_comments: &["//"],
            block_comments: true,
            capitalized_types,
            single_quotes: true,
        };
        match self {
            Lang::Rust => Spec {
                keywords: &[
                    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
                    "extern", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
                    "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "type",
                    "unsafe", "use", "where", "while",
                ],
                literals: &["true", "false"],
                types: &[
                    "bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32",
                    "i64", "i128", "isize", "f32", "f64",
                ],
                line_comments: &["//"],
                block_comments: true,
                capitalized_types: true,
                single_quotes: false,
            },
            Lang::C => c_like(
                &[
                    "auto", "break", "case", "const", "continue", "default", "do", "else", "enum", "extern",
                    "for", "goto", "if", "inline", "register", "return", "sizeof", "static", "struct",
                    "switch", "typedef", "union", "volatile", "while",
                ],
                C_TYPES,
                false,
            ),
            Lang::Cpp => c_like(
                &[
                    "alignas", "auto", "break", "case", "catch", "class", "const", "constexpr", "continue",
                    "default", "delete", "do", "else", "enum", "explicit", "extern", "final", "for",
                    "friend", "if", "inline", "namespace", "new", "noexcept", "operator", "override",
                    "private", "protected", "public", "return", "sizeof", "static", "struct", "switch",
                    "template", "this", "throw", "try", "typedef", "typename", "union", "using", "virtual",
                    "volatile", "while",
                ],
                C_TYPES,
                true,
            ),
            Lang::CSharp => c_like(
                &[
                    "abstract", "as", "async", "await", "base", "break", "case", "catch", "class", "const",
                    "continue", "default", "do", "else", "enum", "event", "finally", "for", "foreach", "if",
                    "in", "interface", "internal", "is", "lock", "namespace", "new", "operator", "out",
                    "override", "private", "protected", "public", "readonly", "ref", "return", "sealed",
                    "static", "struct", "switch", "this", "throw", "try", "using", "virtual", "while",
                ],
                C_TYPES,
                true,
            ),
            Lang::Java => c_like(
                &[
                    "abstract", "assert", "break", "case", "catch", "class", "continue", "default", "do",
                    "else", "enum", "extends", "final", "finally", "for", "if", "implements", "import",
                    "instanceof", "interface", "new", "package", "private", "protected", "public", "return",
                    "static", "super", "switch", "synchronized", "this", "throw", "throws", "try", "while",
                ],
                C_TYPES,
                true,
            ),
            Lang::Kotlin => c_like(
                &[
                    "as", "break", "class", "continue", "do", "else", "for", "fun", "if", "in", "interface",
                    "is", "object", "override", "package", "private", "public", "return", "super", "this",
                    "throw", "try", "typealias", "val", "var", "when", "while", "import", "data", "sealed",
                ],
                C_TYPES,
                true,
            ),
            Lang::Swift => c_like(
                &[
                    "as", "break", "case", "catch", "class", "continue", "default", "defer", "do", "else",
                    "enum", "extension", "fallthrough", "for", "func", "guard", "if", "import", "in", "init",
                    "let", "private", "protocol", "public", "return", "self", "static", "struct", "super",
                    "switch", "throw", "try", "var", "where", "while",
                ],
                C_TYPES,
                true,
            ),
            Lang::Go => c_like(
                &[
                    "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough",
                    "for", "func", "go", "goto", "if", "import", "interface", "map", "package", "range",
                    "return", "select", "struct", "switch", "type", "var",
                ],
                &[
                    "bool", "byte", "error", "float32", "float64", "int", "int8", "int16", "int32", "int64",
                    "rune", "string", "uint", "uint8", "uint16", "uint32", "uint64", "uintptr",
                ],
                true,
            ),
            Lang::JavaScript | Lang::TypeScript => {
                let mut spec = c_like(
                    &[
                        "async", "await", "break", "case", "catch", "class", "const", "continue", "debugger",
                        "default", "delete", "do", "else", "export", "extends", "finally", "for", "from",
                        "function", "if", "import", "in", "instanceof", "let", "new", "of", "return",
                        "static", "super", "switch", "this", "throw", "try", "typeof", "var", "void",
                        "while", "with", "yield", "interface", "type", "enum", "implements", "namespace",
                        "readonly", "declare", "abstract", "as",
                    ],
                    &["string", "number", "boolean", "any", "unknown", "never", "object", "symbol"],
                    true,
                );
                if self == Lang::JavaScript {
                    spec.types = &[];
                }
                spec
            }
            Lang::Python => Spec {
                keywords: &[
                    "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
                    "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is",
                    "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with",
                    "yield", "match", "case",
                ],
                literals: &["True", "False", "None"],
                types: &["int", "float", "str", "bool", "list", "dict", "set", "tuple", "bytes", "self", "cls"],
                line_comments: &["#"],
                block_comments: false,
                capitalized_types: true,
                single_quotes: true,
            },
            Lang::Ruby => Spec {
                keywords: &[
                    "alias", "and", "begin", "break", "case", "class", "def", "defined?", "do", "else",
                    "elsif", "end", "ensure", "for", "if", "in", "module", "next", "not", "or", "redo",
                    "rescue", "retry", "return", "self", "super", "then", "undef", "unless", "until", "when",
                    "while", "yield", "require", "include",
                ],
                literals: &["true", "false", "nil"],
                types: &[],
                line_comments: &["#"],
                block_comments: false,
                capitalized_types: true,
                single_quotes: true,
            },
            Lang::Lua => Spec {
                keywords: &[
                    "and", "break", "do", "else", "elseif", "end", "for", "function", "goto", "if", "in",
                    "local", "not", "or", "repeat", "return", "then", "until", "while",
                ],
                literals: &["true", "false", "nil"],
                types: &[],
                line_comments: &["--"],
                block_comments: false,
                capitalized_types: false,
                single_quotes: true,
            },
            Lang::Php => Spec {
                keywords: &[
                    "abstract", "and", "as", "break", "case", "catch", "class", "const", "continue",
                    "declare", "default", "do", "echo", "else", "elseif", "extends", "final", "finally",
                    "for", "foreach", "function", "global", "if", "implements", "include", "instanceof",
                    "interface", "namespace", "new", "or", "private", "protected", "public", "require",
                    "return", "static", "switch", "throw", "trait", "try", "use", "var", "while", "yield",
                ],
                literals: &["true", "false", "null", "TRUE", "FALSE", "NULL"],
                types: &["int", "float", "string", "bool", "array", "void", "mixed"],
                line_comments: &["//", "#"],
                block_comments: true,
                capitalized_types: true,
                single_quotes: true,
            },
            Lang::Shell => Spec {
                keywords: &[
                    "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function", "if",
                    "in", "local", "readonly", "return", "select", "set", "shift", "then", "time", "unset",
                    "until", "while",
                ],
                literals: &["true", "false"],
                types: &[],
                line_comments: &["#"],
                block_comments: false,
                capitalized_types: false,
                single_quotes: true,
            },
            Lang::Fish => Spec {
                keywords: &[
                    "and", "begin", "break", "case", "continue", "else", "end", "for", "function", "if",
                    "in", "not", "or", "return", "set", "switch", "while",
                ],
                literals: &["true", "false"],
                types: &[],
                line_comments: &["#"],
                block_comments: false,
                capitalized_types: false,
                single_quotes: true,
            },
            Lang::Sql => Spec {
                keywords: &[
                    "select", "from", "where", "insert", "into", "values", "update", "set", "delete", "create",
                    "table", "drop", "alter", "join", "left", "right", "inner", "outer", "on", "group", "by",
                    "order", "having", "limit", "as", "and", "or", "not", "in", "is", "like", "union",
                    "distinct", "primary", "key", "index", "SELECT", "FROM", "WHERE", "INSERT", "INTO",
                    "VALUES", "UPDATE", "SET", "DELETE", "CREATE", "TABLE", "DROP", "ALTER", "JOIN", "LEFT",
                    "RIGHT", "INNER", "OUTER", "ON", "GROUP", "BY", "ORDER", "HAVING", "LIMIT", "AS", "AND",
                    "OR", "NOT", "IN", "IS", "LIKE", "UNION", "DISTINCT", "PRIMARY", "KEY", "INDEX",
                ],
                literals: &["null", "NULL", "true", "false", "TRUE", "FALSE"],
                types: &[],
                line_comments: &["--"],
                block_comments: true,
                capitalized_types: false,
                single_quotes: true,
            },
            Lang::Toml | Lang::Yaml => Spec {
                keywords: &[],
                literals: &["true", "false", "null", "yes", "no", "on", "off"],
                types: &[],
                line_comments: &["#"],
                block_comments: false,
                capitalized_types: false,
                single_quotes: true,
            },
            Lang::Json => Spec {
                keywords: &[],
                literals: &["true", "false", "null"],
                types: &[],
                line_comments: &[],
                block_comments: false,
                capitalized_types: false,
                single_quotes: false,
            },
        }
    }
}

/// Picks the language from the file extension; a file without an extension
/// falls back to the shebang on its first line. `None` disables highlighting.
pub fn detect(path: Option<&Path>, first_line: &str) -> Option<Lang> {
    match path.and_then(|p| p.extension()) {
        Some(ext) => from_extension(&ext.to_string_lossy().to_ascii_lowercase()),
        None => from_shebang(first_line),
    }
}

fn from_extension(ext: &str) -> Option<Lang> {
    Some(match ext {
        "rs" => Lang::Rust,
        "c" | "h" => Lang::C,
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Lang::Cpp,
        "cs" => Lang::CSharp,
        "java" => Lang::Java,
        "kt" | "kts" => Lang::Kotlin,
        "swift" => Lang::Swift,
        "go" => Lang::Go,
        "js" | "mjs" | "cjs" | "jsx" => Lang::JavaScript,
        "ts" | "mts" | "cts" | "tsx" => Lang::TypeScript,
        "py" | "pyw" => Lang::Python,
        "rb" => Lang::Ruby,
        "lua" => Lang::Lua,
        "php" => Lang::Php,
        "sh" | "bash" | "zsh" | "ksh" => Lang::Shell,
        "fish" => Lang::Fish,
        "sql" => Lang::Sql,
        "toml" => Lang::Toml,
        "yaml" | "yml" => Lang::Yaml,
        "json" => Lang::Json,
        _ => return None,
    })
}

fn from_shebang(first_line: &str) -> Option<Lang> {
    let rest = first_line.strip_prefix("#!")?;
    let mut parts = rest.split_whitespace();
    let mut program = parts.next()?.rsplit('/').next()?;
    if program == "env" {
        // `env [-S] [VAR=value] program`
        program = parts.find(|a| !a.starts_with('-') && !a.contains('='))?;
    }
    Some(match program {
        p if p.starts_with("python") => Lang::Python,
        "bash" | "sh" | "zsh" | "dash" | "ash" | "ksh" => Lang::Shell,
        "fish" => Lang::Fish,
        "node" | "nodejs" | "deno" | "bun" => Lang::JavaScript,
        p if p.starts_with("ruby") => Lang::Ruby,
        p if p.starts_with("lua") => Lang::Lua,
        p if p.starts_with("php") => Lang::Php,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub palette: Palette,
    /// `None` leaves the text uncoloured.
    pub lang: Option<Lang>,
    /// Changes when another document is shown, which makes the editor
    /// highlight the new text from its first line.
    pub doc_id: u64,
}

/// A small per-line tokenizer. It carries no state across lines, so
/// multi-line block comments and strings are not recognised.
pub struct Syntax {
    settings: Settings,
    line: usize,
}

impl highlighter::Highlighter for Syntax {
    type Settings = Settings;
    type Highlight = Color;
    type Iterator<'a> = std::vec::IntoIter<(Range<usize>, Color)>;

    fn new(settings: &Settings) -> Self {
        Self { settings: *settings, line: 0 }
    }

    fn update(&mut self, new_settings: &Settings) {
        self.settings = *new_settings;
        self.line = 0;
    }

    fn change_line(&mut self, line: usize) {
        self.line = self.line.min(line);
    }

    fn highlight_line(&mut self, line: &str) -> Self::Iterator<'_> {
        self.line += 1;
        match self.settings.lang {
            Some(lang) => tokenize(line, &self.settings.palette, &lang.spec()).into_iter(),
            None => Vec::new().into_iter(),
        }
    }

    fn current_line(&self) -> usize {
        self.line
    }
}

pub fn format(color: &Color, _theme: &iced::Theme) -> Format<iced::Font> {
    Format { color: Some(*color), font: None }
}

fn tokenize(line: &str, p: &Palette, spec: &Spec) -> Vec<(Range<usize>, Color)> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        let c = bytes[i];
        let rest = &line[i..];

        if spec.line_comments.iter().any(|m| rest.starts_with(m)) {
            out.push((i..line.len(), p.comment));
            break;
        }

        if spec.block_comments && rest.starts_with("/*") {
            let end = rest[2..].find("*/").map_or(line.len(), |e| i + 2 + e + 2);
            out.push((i..end, p.comment));
            i = end;
            continue;
        }

        let is_quote = c == b'"' || c == b'`' || (c == b'\'' && spec.single_quotes);
        if is_quote {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != c {
                if bytes[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            let end = (j + 1).min(line.len());
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
            if spec.keywords.contains(&word) {
                out.push((i..j, p.keyword));
            } else if spec.literals.contains(&word) {
                out.push((i..j, p.number));
            } else if spec.types.contains(&word)
                || (spec.capitalized_types && word.chars().next().is_some_and(|c| c.is_ascii_uppercase()))
            {
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
