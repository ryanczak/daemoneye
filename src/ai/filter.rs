use regex::Regex;
use std::borrow::Cow;
use std::sync::OnceLock;

static PATTERNS: OnceLock<Vec<(Regex, String)>> = OnceLock::new();

/// Built-in (always-on) pattern definitions: `(regex, replacement)`.
fn builtin_defs() -> &'static [(&'static str, &'static str)] {
    &[
        // AWS access key IDs  (AKIA…)
        (r"AKIA[0-9A-Z]{16}", "<AWS_KEY>"),
        // PEM private key blocks (RSA, EC, OPENSSH, etc.)
        (
            r"-----BEGIN [A-Z ]+PRIVATE KEY-----[\s\S]*?-----END [A-Z ]+PRIVATE KEY-----",
            "<PRIVATE_KEY>",
        ),
        // GCP service-account JSON: "private_key" field (value is always a PEM key)
        (r#""private_key"\s*:\s*"[^"]+""#, "<GCP_KEY>"),
        // JWT bearer tokens: three base64url segments; header always starts with eyJ
        (
            r"eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+",
            "<JWT>",
        ),
        // GitHub PATs — classic (ghp_/gho_/ghu_/ghs_/ghr_) and fine-grained (github_pat_)
        (r"gh[psohr]_[A-Za-z0-9]{36}", "<GITHUB_TOKEN>"),
        (r"github_pat_[A-Za-z0-9_]{22,}", "<GITHUB_TOKEN>"),
        // Database / broker connection strings with embedded credentials
        (
            r"(?i)(postgresql|postgres|mysql|mongodb(\+srv)?|redis|amqps?|rabbitmq)://[^:@\s]+:[^@\s]+@\S+",
            "<DB_URL>",
        ),
        // Generic key=value / key: value secret assignments
        (
            r"(?i)(password|passwd|secret|token|api[_\-]?key|apikey)\s*[=:]\s*\S+",
            "<REDACTED>",
        ),
        // URL query-param secrets: ?password=… or &api_key=…
        (
            r"(?i)[?&](password|passwd|secret|token|api[_\-]?key)=[^\s&]+",
            "<REDACTED>",
        ),
        // Basic credit card (4-group 16-digit number)
        (r"\b(?:\d{4}[- ]){3}\d{4}\b", "<CARD>"),
        // US Social Security Number
        (r"\b\d{3}-\d{2}-\d{4}\b", "<SSN>"),
    ]
}

/// Compile built-in patterns plus any caller-supplied extras into a pattern list.
fn compile_patterns(extra: &[String]) -> Vec<(Regex, String)> {
    let mut result = Vec::new();
    for (pat, rep) in builtin_defs() {
        match Regex::new(pat) {
            Ok(re) => result.push((re, rep.to_string())),
            Err(e) => eprintln!("Warning: built-in masking pattern failed to compile: {e}"),
        }
    }
    for pat in extra {
        match Regex::new(pat) {
            Ok(re) => result.push((re, "<REDACTED>".to_string())),
            Err(_) => eprintln!("Warning: invalid masking pattern ignored: {pat}"),
        }
    }
    result
}

/// Initialise the global pattern set with built-in patterns plus `extra_patterns`
/// from the user's config. Call once at daemon startup before the first
/// `mask_sensitive` invocation. If not called, only built-in patterns are used.
pub fn init_masking(extra_patterns: &[String]) {
    let _ = PATTERNS.set(compile_patterns(extra_patterns));
}

/// Mask all known-sensitive patterns in `text` before it is sent to an AI API.
pub fn mask_sensitive(text: &str) -> String {
    let pats = PATTERNS.get_or_init(|| compile_patterns(&[]));
    let mut result: Cow<str> = Cow::Borrowed(text);
    for (re, rep) in pats {
        if re.is_match(&result) {
            result = Cow::Owned(re.replace_all(&result, rep.as_str()).into_owned());
        }
    }
    result.into_owned()
}

#[cfg(test)]
mod tests {
    use super::compile_patterns;

    /// Run patterns compiled fresh (bypasses the global OnceLock) so tests are
    /// isolated and can be run in any order without interfering with each other.
    fn mask(text: &str) -> String {
        let pats = compile_patterns(&[]);
        let mut result: std::borrow::Cow<str> = std::borrow::Cow::Borrowed(text);
        for (re, rep) in &pats {
            if re.is_match(&result) {
                result =
                    std::borrow::Cow::Owned(re.replace_all(&result, rep.as_str()).into_owned());
            }
        }
        result.into_owned()
    }

    #[test]
    fn aws_key() {
        assert!(mask("AKIAIOSFODNN7EXAMPLE").contains("<AWS_KEY>"));
    }

    #[test]
    fn private_key_pem() {
        let text =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA\n-----END RSA PRIVATE KEY-----";
        assert!(mask(text).contains("<PRIVATE_KEY>"));
    }

    #[test]
    fn gcp_json_key() {
        let text = r#"{"private_key": "-----BEGIN RSA PRIVATE KEY-----\nMIIE\n-----END RSA PRIVATE KEY-----\n"}"#;
        assert!(mask(text).contains("<GCP_KEY>"));
    }

    #[test]
    fn jwt() {
        let token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9\
                     .eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIn0\
                     .SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        assert!(mask(token).contains("<JWT>"));
    }

    #[test]
    fn github_pat_classic() {
        assert!(mask("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefgh01").contains("<GITHUB_TOKEN>"));
        assert!(mask("ghs_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefgh01").contains("<GITHUB_TOKEN>"));
    }

    #[test]
    fn github_pat_fine_grained() {
        assert!(mask("github_pat_11ABCDEFG0abcdefghijklmnopqrst").contains("<GITHUB_TOKEN>"));
    }

    #[test]
    fn db_url_postgres() {
        assert!(mask("postgresql://admin:hunter2@db.example.com:5432/mydb").contains("<DB_URL>"));
    }

    #[test]
    fn db_url_mysql() {
        assert!(mask("mysql://root:password@localhost/app").contains("<DB_URL>"));
    }

    #[test]
    fn db_url_mongodb_srv() {
        assert!(
            mask("mongodb+srv://user:pass@cluster0.example.mongodb.net/db").contains("<DB_URL>")
        );
    }

    #[test]
    fn generic_assignment() {
        assert!(mask("password=supersecret").contains("<REDACTED>"));
        assert!(mask("api_key: abc123xyz").contains("<REDACTED>"));
        assert!(mask("SECRET=hunter2").contains("<REDACTED>"));
    }

    #[test]
    fn query_param_secret() {
        assert!(mask("https://api.example.com/v1?token=abc123&other=fine").contains("<REDACTED>"));
    }

    #[test]
    fn credit_card() {
        assert!(mask("card: 4111 1111 1111 1111").contains("<CARD>"));
        assert!(mask("4111-1111-1111-1111").contains("<CARD>"));
    }

    #[test]
    fn ssn() {
        assert!(mask("SSN: 123-45-6789").contains("<SSN>"));
    }

    #[test]
    fn extra_user_pattern() {
        let pats = compile_patterns(&["MYCO-[A-Z0-9]{8}".to_string()]);
        let mut result: std::borrow::Cow<str> =
            std::borrow::Cow::Borrowed("deploy key: MYCO-ABCD1234");
        for (re, rep) in &pats {
            if re.is_match(&result) {
                result =
                    std::borrow::Cow::Owned(re.replace_all(&result, rep.as_str()).into_owned());
            }
        }
        assert!(result.contains("<REDACTED>"));
    }

    #[test]
    fn invalid_extra_pattern_is_ignored() {
        // Should not panic; just skips the bad pattern.
        let pats = compile_patterns(&["[invalid".to_string()]);
        // Built-in patterns should still be present.
        assert!(!pats.is_empty());
    }

    #[test]
    fn clean_text_unchanged() {
        let text = "ls -la /home/user";
        assert_eq!(mask(text), text);
    }

    // ── Additional edge cases ─────────────────────────────────────────────────

    #[test]
    fn multiple_secrets_in_one_string() {
        let text = "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE and password=hunter2";
        let out = mask(text);
        assert!(
            !out.contains("AKIAIOSFODNN7EXAMPLE"),
            "AWS key should be masked"
        );
        assert!(!out.contains("hunter2"), "password should be masked");
    }

    #[test]
    fn jwt_inside_json_value() {
        let text = r#"{"token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"}"#;
        let out = mask(text);
        assert!(out.contains("<JWT>"), "JWT inside JSON should be masked");
    }

    #[test]
    fn github_pat_fine_grained_masked() {
        let text = "token: github_pat_11ABCDEFG_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let out = mask(text);
        assert!(out.contains("<GITHUB_PAT>") || out.contains("<REDACTED>"));
        assert!(!out.contains("github_pat_11ABCDEFG"));
    }

    #[test]
    fn ssh_key_pem_masked() {
        let text =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA\n-----END RSA PRIVATE KEY-----";
        let out = mask(text);
        assert!(
            !out.contains("MIIEpAIBAAKCAQEA"),
            "PEM body should be masked"
        );
    }

    #[test]
    fn credit_card_with_spaces_masked() {
        let text = "Payment card: 5555 5555 5555 4444";
        let out = mask(text);
        assert!(out.contains("<CARD>") || !out.contains("5555 5555 5555 4444"));
    }

    #[test]
    fn api_key_assignment_case_insensitive() {
        // Both "API_KEY" and "api_key" patterns should be caught.
        assert!(mask("API_KEY=abc123xyz").contains("<REDACTED>"));
        assert!(mask("api_key=abc123xyz").contains("<REDACTED>"));
    }

    #[test]
    fn url_without_credentials_not_masked() {
        let text = "https://example.com/api/v1/resource";
        let out = mask(text);
        assert_eq!(out, text, "plain URL should not be modified");
    }

    #[test]
    fn empty_string_unchanged() {
        assert_eq!(mask(""), "");
    }

    #[test]
    fn multiline_text_preserves_clean_lines() {
        let text = "ls -la\necho hello\ncat /etc/hostname";
        assert_eq!(mask(text), text);
    }
}
