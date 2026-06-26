#[derive(Copy, Clone)]
enum CommentStyle {
    Hash,        // #  (bash, python, yaml, ruby, dockerfile)
    DoubleSlash, // // (rust, js, go, java, c, c++)
    DoubleDash,  // -- (sql, lua, haskell)
    Semicolon,   // ;  (lisp, asm)
    None,
}

fn lang_keywords(lang: &str) -> &'static [&'static str] {
    match lang {
        "bash" | "sh" | "shell" | "zsh" | "fish" => &[
            "if", "then", "else", "elif", "fi", "for", "in", "do", "done", "while", "until",
            "case", "esac", "function", "return", "local", "export", "readonly", "declare",
            "unset", "source", "echo", "printf", "cd", "exit", "break", "continue", "shift", "set",
            "unsetopt",
        ],
        "python" | "py" => &[
            "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class",
            "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global",
            "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise",
            "return", "try", "while", "with", "yield",
        ],
        "rust" | "rs" => &[
            "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
            "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
            "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super",
            "trait", "true", "type", "union", "unsafe", "use", "where", "while",
        ],
        "javascript" | "js" | "typescript" | "ts" | "jsx" | "tsx" => &[
            "break",
            "case",
            "catch",
            "class",
            "const",
            "continue",
            "debugger",
            "default",
            "delete",
            "do",
            "else",
            "export",
            "extends",
            "false",
            "finally",
            "for",
            "function",
            "if",
            "import",
            "in",
            "instanceof",
            "let",
            "new",
            "null",
            "return",
            "static",
            "super",
            "switch",
            "this",
            "throw",
            "true",
            "try",
            "typeof",
            "undefined",
            "var",
            "void",
            "while",
            "with",
            "yield",
            "async",
            "await",
            "of",
            "from",
            "type",
            "interface",
            "enum",
            "implements",
            "readonly",
        ],
        "go" | "golang" => &[
            "break",
            "case",
            "chan",
            "const",
            "continue",
            "default",
            "defer",
            "else",
            "fallthrough",
            "for",
            "func",
            "go",
            "goto",
            "if",
            "import",
            "interface",
            "map",
            "package",
            "range",
            "return",
            "select",
            "struct",
            "switch",
            "type",
            "var",
            "true",
            "false",
            "nil",
        ],
        "java" => &[
            "abstract",
            "assert",
            "boolean",
            "break",
            "byte",
            "case",
            "catch",
            "char",
            "class",
            "const",
            "continue",
            "default",
            "do",
            "double",
            "else",
            "enum",
            "extends",
            "false",
            "final",
            "finally",
            "float",
            "for",
            "goto",
            "if",
            "implements",
            "import",
            "instanceof",
            "int",
            "interface",
            "long",
            "native",
            "new",
            "null",
            "package",
            "private",
            "protected",
            "public",
            "return",
            "short",
            "static",
            "strictfp",
            "super",
            "switch",
            "synchronized",
            "this",
            "throw",
            "throws",
            "transient",
            "true",
            "try",
            "void",
            "volatile",
            "while",
        ],
        "sql" => &[
            "SELECT",
            "FROM",
            "WHERE",
            "AND",
            "OR",
            "NOT",
            "INSERT",
            "INTO",
            "VALUES",
            "UPDATE",
            "SET",
            "DELETE",
            "CREATE",
            "TABLE",
            "DROP",
            "ALTER",
            "ADD",
            "COLUMN",
            "INDEX",
            "PRIMARY",
            "KEY",
            "FOREIGN",
            "REFERENCES",
            "JOIN",
            "LEFT",
            "RIGHT",
            "INNER",
            "OUTER",
            "ON",
            "GROUP",
            "BY",
            "ORDER",
            "HAVING",
            "LIMIT",
            "OFFSET",
            "DISTINCT",
            "AS",
            "IN",
            "IS",
            "NULL",
            "NOT",
            "EXISTS",
            "UNION",
            "ALL",
            "CASE",
            "WHEN",
            "THEN",
            "ELSE",
            "END",
            "WITH",
            "RETURNING",
            "CONSTRAINT",
            "UNIQUE",
            "DEFAULT",
            "AUTO_INCREMENT",
            "SERIAL",
        ],
        _ => &[],
    }
}

fn lang_comment_style(lang: &str) -> CommentStyle {
    match lang {
        "bash" | "sh" | "shell" | "zsh" | "fish" | "python" | "py" | "ruby" | "rb" | "yaml"
        | "yml" | "toml" | "dockerfile" | "docker" => CommentStyle::Hash,

        "rust" | "rs" | "javascript" | "js" | "typescript" | "ts" | "jsx" | "tsx" | "go"
        | "golang" | "java" | "c" | "cpp" | "c++" | "cc" | "h" | "hpp" | "css" | "scss"
        | "sass" | "swift" | "kotlin" | "scala" => CommentStyle::DoubleSlash,

        "sql" | "lua" | "haskell" | "hs" => CommentStyle::DoubleDash,

        "lisp" | "scheme" | "clojure" | "asm" | "nasm" => CommentStyle::Semicolon,

        _ => CommentStyle::None,
    }
}

/// Colorize a single word if it matches the keyword list.
fn emit_word_token(out: &mut String, word: &str, keywords: &[&str], is_sql: bool) {
    if word.is_empty() {
        return;
    }
    let matched = if is_sql {
        keywords.iter().any(|k| k.eq_ignore_ascii_case(word))
    } else {
        keywords.contains(&word)
    };
    if matched {
        out.push_str("\x1b[1m\x1b[94m"); // bold bright-blue
        out.push_str(word);
        out.push_str("\x1b[0m");
    } else {
        out.push_str(word);
    }
}

/// Apply syntax highlighting to a single code line.
///
/// For known languages, scans character-by-character tracking string and
/// comment state.  For unknown or missing languages, falls back to plain cyan.
pub(super) fn highlight_code(line: &str, lang: Option<&str>) -> String {
    let lang_lower = lang.map(|l| l.to_lowercase());
    let lang_str = lang_lower.as_deref().unwrap_or("");
    let keywords = lang_keywords(lang_str);
    let comment_style = lang_comment_style(lang_str);
    let is_sql = matches!(lang_str, "sql");

    // Unknown / plain language: just emit in cyan.
    if keywords.is_empty() && matches!(comment_style, CommentStyle::None) {
        return format!("\x1b[36m{}\x1b[0m", line);
    }

    let mut out = String::with_capacity(line.len() * 2);
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    // Detect single-line comments that start at column 0 or after whitespace.
    // We check for comment prefix at the start of each "token" boundary.
    let comment_prefix: Option<&str> = match comment_style {
        CommentStyle::Hash => Some("#"),
        CommentStyle::DoubleSlash => Some("//"),
        CommentStyle::DoubleDash => Some("--"),
        CommentStyle::Semicolon => Some(";"),
        CommentStyle::None => None,
    };

    // String quote char currently open (None = not in a string).
    let mut in_string: Option<char> = None;
    // Current non-string word accumulator.
    let mut word = String::new();

    macro_rules! flush_word {
        () => {
            if !word.is_empty() {
                let w = std::mem::take(&mut word);
                emit_word_token(&mut out, &w, keywords, is_sql);
            }
        };
    }

    while i < len {
        // ── Inside a string literal ──────────────────────────────────────
        if let Some(q) = in_string {
            out.push(chars[i]);
            if chars[i] == '\\' && i + 1 < len {
                i += 1;
                out.push(chars[i]);
            } else if chars[i] == q {
                out.push_str("\x1b[0m");
                in_string = None;
            }
            i += 1;
            continue;
        }

        // ── Check for comment start ──────────────────────────────────────
        if let Some(prefix) = comment_prefix {
            let remaining: String = chars[i..].iter().collect();
            if remaining.starts_with(prefix) {
                flush_word!();
                out.push_str("\x1b[2m\x1b[3m"); // dim italic
                // Emit the rest of the line as comment
                for &c in &chars[i..] {
                    out.push(c);
                }
                out.push_str("\x1b[0m");
                return out;
            }
        }

        // ── String open ─────────────────────────────────────────────────
        if chars[i] == '"' || chars[i] == '\'' {
            flush_word!();
            let q = chars[i];
            out.push_str("\x1b[32m"); // green
            out.push(q);
            in_string = Some(q);
            i += 1;
            continue;
        }

        // ── Word boundary (identifier / keyword chars) ───────────────────
        if chars[i].is_alphanumeric() || chars[i] == '_' {
            word.push(chars[i]);
            i += 1;
            continue;
        }

        // ── Number literal ───────────────────────────────────────────────
        if word.is_empty() && chars[i].is_ascii_digit() {
            // Collect the whole number token
            let mut num = String::new();
            while i < len
                && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_')
            {
                num.push(chars[i]);
                i += 1;
            }
            out.push_str("\x1b[33m"); // yellow
            out.push_str(&num);
            out.push_str("\x1b[0m");
            continue;
        }

        // ── Non-word, non-string, non-comment punctuation / space ────────
        flush_word!();
        out.push(chars[i]);
        i += 1;
    }

    flush_word!();

    // Close any unclosed string (shouldn't happen for well-formed code)
    if in_string.is_some() {
        out.push_str("\x1b[0m");
    }

    out
}
