use regex::Regex;
use std::borrow::Cow;
use std::sync::OnceLock;

/// Mask sensitive patterns in terminal context before sending to an AI API.
pub fn mask_sensitive(text: &str) -> String {
    static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();

    let pats = PATTERNS.get_or_init(|| {
        let defs: &[(&str, &str)] = &[
            // AWS access key IDs  (AKIA...)
            (r"AKIA[0-9A-Z]{16}", "<AWS_KEY>"),
            // Private key blocks
            (
                r"-----BEGIN [A-Z ]+PRIVATE KEY-----[\s\S]*?-----END [A-Z ]+PRIVATE KEY-----",
                "<PRIVATE_KEY>",
            ),
            // Generic secret/password/token/api_key assignments
            (
                r"(?i)(password|passwd|secret|token|api[_\-]?key|apikey)\s*[=:]\s*\S+",
                "<REDACTED>",
            ),
            // Basic credit card  (4-group 16-digit number)
            (r"\b(?:\d{4}[- ]){3}\d{4}\b", "<CARD>"),
            // US SSN
            (r"\b\d{3}-\d{2}-\d{4}\b", "<SSN>"),
        ];
        defs.iter()
            .filter_map(|(pat, rep)| Regex::new(pat).ok().map(|r| (r, *rep)))
            .collect()
    });

    let mut result: Cow<str> = Cow::Borrowed(text);
    for (re, rep) in pats {
        if re.is_match(&result) {
            result = Cow::Owned(re.replace_all(&result, *rep).into_owned());
        }
    }
    result.into_owned()
}
