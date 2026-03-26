use regex::Regex;
use std::borrow::Cow;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

static PATTERNS: OnceLock<Vec<(Regex, String, bool)>> = OnceLock::new();

// Per-type redaction counters (lifetime of the daemon process).
static REDACT_AWS_KEY: AtomicUsize = AtomicUsize::new(0);
static REDACT_PRIVATE_KEY: AtomicUsize = AtomicUsize::new(0);
static REDACT_GCP_KEY: AtomicUsize = AtomicUsize::new(0);
static REDACT_JWT: AtomicUsize = AtomicUsize::new(0);
static REDACT_GITHUB_TOKEN: AtomicUsize = AtomicUsize::new(0);
static REDACT_DB_URL: AtomicUsize = AtomicUsize::new(0);
static REDACT_GENERIC: AtomicUsize = AtomicUsize::new(0);
static REDACT_CARD: AtomicUsize = AtomicUsize::new(0);
static REDACT_SSN: AtomicUsize = AtomicUsize::new(0);
static REDACT_USER: AtomicUsize = AtomicUsize::new(0);
static REDACT_SSH_PUBKEY: AtomicUsize = AtomicUsize::new(0);
static REDACT_SSH_HOST: AtomicUsize = AtomicUsize::new(0);

fn counter_for(rep: &str) -> &'static AtomicUsize {
    match rep {
        "<AWS_KEY>" => &REDACT_AWS_KEY,
        "<PRIVATE_KEY>" => &REDACT_PRIVATE_KEY,
        "<GCP_KEY>" => &REDACT_GCP_KEY,
        "<JWT>" => &REDACT_JWT,
        "<GITHUB_TOKEN>" => &REDACT_GITHUB_TOKEN,
        "<DB_URL>" => &REDACT_DB_URL,
        "<CARD>" => &REDACT_CARD,
        "<SSN>" => &REDACT_SSN,
        "<SSH_PUBKEY>" => &REDACT_SSH_PUBKEY,
        "<SSH_HOST>" => &REDACT_SSH_HOST,
        _ => &REDACT_GENERIC,
    }
}

/// Returns a snapshot of redaction counts by type since daemon start.
/// All built-in types are always included, even when the count is zero.
pub fn get_redaction_counts() -> std::collections::HashMap<String, usize> {
    let raw = [
        ("AWS Key", REDACT_AWS_KEY.load(Ordering::Relaxed)),
        ("Private Key", REDACT_PRIVATE_KEY.load(Ordering::Relaxed)),
        ("GCP Key", REDACT_GCP_KEY.load(Ordering::Relaxed)),
        ("JWT", REDACT_JWT.load(Ordering::Relaxed)),
        ("GitHub Token", REDACT_GITHUB_TOKEN.load(Ordering::Relaxed)),
        ("DB URL", REDACT_DB_URL.load(Ordering::Relaxed)),
        ("Secret", REDACT_GENERIC.load(Ordering::Relaxed)),
        ("Card", REDACT_CARD.load(Ordering::Relaxed)),
        ("SSN", REDACT_SSN.load(Ordering::Relaxed)),
        ("User Defined", REDACT_USER.load(Ordering::Relaxed)),
        (
            "SSH Secrets",
            REDACT_SSH_PUBKEY.load(Ordering::Relaxed) + REDACT_SSH_HOST.load(Ordering::Relaxed),
        ),
    ];
    raw.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

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
        // SSH public keys: id_rsa.pub, authorized_keys, known_hosts key data.
        // All OpenSSH public key types start with "AAAA" in base64.
        (
            r"(?:ssh-(?:rsa|dss|ed25519)|ecdsa-sha2-nistp(?:256|384|521)|sk-(?:ssh-ed25519|ecdsa-sha2-nistp256)@openssh\.com)\s+AAAA[A-Za-z0-9+/=]+",
            "<SSH_PUBKEY>",
        ),
        // SSH config file directives that reveal server topology.
        // Matches: HostName <value> and IdentityFile <value> (case-insensitive).
        (r"(?i)\b(?:HostName|IdentityFile)\s+\S+", "<SSH_HOST>"),
    ]
}

/// Compile built-in patterns plus any caller-supplied extras into a pattern list.
/// Each entry is `(regex, replacement, is_user)` where `is_user` marks user-config patterns.
fn compile_patterns(extra: &[String]) -> Vec<(Regex, String, bool)> {
    let mut result = Vec::new();
    for (pat, rep) in builtin_defs() {
        match Regex::new(pat) {
            Ok(re) => result.push((re, rep.to_string(), false)),
            Err(e) => log::warn!("Built-in masking pattern failed to compile: {e}"),
        }
    }
    for pat in extra {
        match Regex::new(pat) {
            Ok(re) => result.push((re, "<REDACTED>".to_string(), true)),
            Err(e) => log::warn!("Invalid masking pattern ignored ({pat}): {e}"),
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
    for (re, rep, is_user) in pats {
        if re.is_match(&result) {
            let rep_str = rep.as_str();
            let counter = if *is_user {
                &REDACT_USER
            } else {
                counter_for(rep_str)
            };
            let mut n = 0usize;
            result = Cow::Owned(
                re.replace_all(&result, |_: &regex::Captures<'_>| {
                    n += 1;
                    rep_str
                })
                .into_owned(),
            );
            counter.fetch_add(n, Ordering::Relaxed);
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
        for (re, rep, _) in &pats {
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
        for (re, rep, _) in &pats {
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

    // ── SSH-specific patterns ─────────────────────────────────────────────────

    #[test]
    fn ssh_rsa_pubkey_masked() {
        let text = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQC7user@host";
        let out = mask(text);
        assert!(
            out.contains("<SSH_PUBKEY>"),
            "ssh-rsa public key should be masked"
        );
        assert!(
            !out.contains("AAAAB3NzaC1yc2E"),
            "key data should not be present"
        );
    }

    #[test]
    fn ssh_ed25519_pubkey_masked() {
        let text = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAbcdef user@laptop";
        let out = mask(text);
        assert!(
            out.contains("<SSH_PUBKEY>"),
            "ssh-ed25519 public key should be masked"
        );
    }

    #[test]
    fn authorized_keys_line_masked() {
        // Typical authorized_keys line: key-type key-data optional-comment
        let text = "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQDexample== deploy@ci";
        let out = mask(text);
        assert!(out.contains("<SSH_PUBKEY>"));
        assert!(!out.contains("AAAAB3Nz"));
    }

    #[test]
    fn known_hosts_key_data_masked() {
        // known_hosts lines end with a public key
        let text = "github.com ssh-rsa AAAAB3NzaC1yc2EAAAABIwAAAQEAq2A7hRGm";
        let out = mask(text);
        assert!(out.contains("<SSH_PUBKEY>"));
        assert!(!out.contains("AAAAB3Nz"));
        // hostname itself is preserved (not a secret)
        assert!(out.contains("github.com"));
    }

    #[test]
    fn ssh_config_hostname_masked() {
        let text = "  HostName 10.0.0.42";
        let out = mask(text);
        assert!(
            out.contains("<SSH_HOST>"),
            "HostName directive should be masked"
        );
        assert!(!out.contains("10.0.0.42"));
    }

    #[test]
    fn ssh_config_identityfile_masked() {
        let text = "  IdentityFile ~/.ssh/id_rsa_prod";
        let out = mask(text);
        assert!(
            out.contains("<SSH_HOST>"),
            "IdentityFile directive should be masked"
        );
        assert!(!out.contains("id_rsa_prod"));
    }

    #[test]
    fn ssh_config_hostname_case_insensitive() {
        assert!(mask("hostname 192.168.1.1").contains("<SSH_HOST>"));
        assert!(mask("HOSTNAME server.internal").contains("<SSH_HOST>"));
    }

    #[test]
    fn ecdsa_pubkey_masked() {
        let text = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTY= user@host";
        let out = mask(text);
        assert!(out.contains("<SSH_PUBKEY>"));
    }
}
