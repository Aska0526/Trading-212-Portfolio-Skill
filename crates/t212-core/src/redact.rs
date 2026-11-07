//! Redaction shared by all modules. Allowlist-style: callers log only fields
//! they pass through here, and known secrets are scrubbed from free text.

pub const REDACTED: &str = "[REDACTED]";

/// Scrub known secret values and `Authorization` header values from text.
pub fn redact(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_owned();
    for s in secrets.iter().filter(|s| !s.is_empty()) {
        out = out.replace(s, REDACTED);
    }
    scrub_authorization(&out)
}

fn scrub_authorization(text: &str) -> String {
    text.lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            match lower.find("authorization:") {
                Some(i) => format!("{}authorization: {REDACTED}", &line[..i]),
                None => line.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_canary_and_auth_header() {
        let out = redact(
            "key=CANARY123 failed\nAuthorization: Basic abc",
            &["CANARY123"],
        );
        assert!(!out.contains("CANARY123"));
        assert!(!out.contains("Basic abc"));
    }
}
