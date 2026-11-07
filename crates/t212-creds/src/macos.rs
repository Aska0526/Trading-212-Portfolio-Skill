//! macOS login keychain via `/usr/bin/security`.
//!
//! Secrets never appear in any process's arguments:
//! - write: `security -i` reads the add command (with the secret hex-encoded)
//!   from a stdin pipe; its argv is only `-i`.
//! - read: `find-generic-password -w` prints the secret to a stdout pipe.
//!
//! The item's access list trusts `/usr/bin/security` (Apple-signed, stable
//! identity), so reads do not prompt after updates of `t212`.

use crate::storage_error;
use std::io::Write;
use std::process::{Command, Stdio};
use t212_core::error::T212Error;

const SECURITY: &str = "/usr/bin/security";
/// `security` exit status when the item does not exist (errSecItemNotFound).
const NOT_FOUND: i32 = 44;

fn hex(data: &str) -> String {
    data.bytes().map(|b| format!("{b:02x}")).collect()
}

/// Only simple identifiers are accepted for service/account so they can be
/// placed in the interactive command line without quoting issues.
fn check_label(s: &str) -> Result<(), T212Error> {
    let ok = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(storage_error())
    }
}

pub fn find(service: &str, account: &str) -> Result<Option<String>, T212Error> {
    check_label(service)?;
    check_label(account)?;
    let out = Command::new(SECURITY)
        .args(["find-generic-password", "-s", service, "-a", account, "-w"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| storage_error())?;
    match out.status.code() {
        Some(0) => {
            let mut s = String::from_utf8(out.stdout).map_err(|_| storage_error())?;
            if s.ends_with('\n') {
                s.pop();
            }
            Ok(Some(s))
        }
        Some(NOT_FOUND) => Ok(None),
        _ => Err(storage_error()),
    }
}

pub fn delete(service: &str, account: &str) -> Result<bool, T212Error> {
    check_label(service)?;
    check_label(account)?;
    let status = Command::new(SECURITY)
        .args(["delete-generic-password", "-s", service, "-a", account])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| storage_error())?;
    match status.code() {
        Some(0) => Ok(true),
        Some(NOT_FOUND) => Ok(false),
        _ => Err(storage_error()),
    }
}

/// Replace the item. The old item is deleted first so the new one gets a
/// fresh access list (an update would keep the old list).
pub fn add(service: &str, account: &str, secret: &str) -> Result<(), T212Error> {
    check_label(service)?;
    check_label(account)?;
    delete(service, account)?;
    let mut child = Command::new(SECURITY)
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| storage_error())?;
    {
        let stdin = child.stdin.as_mut().ok_or_else(storage_error)?;
        let cmd = format!(
            "add-generic-password -s {service} -a {account} -T {SECURITY} -X {}\n",
            hex(secret)
        );
        stdin
            .write_all(cmd.as_bytes())
            .map_err(|_| storage_error())?;
    }
    let status = child.wait().map_err(|_| storage_error())?;
    if !status.success() {
        return Err(storage_error());
    }
    // `security -i` can exit 0 even if a command failed, so confirm the write.
    match find(service, account)? {
        Some(v) if v == secret => Ok(()),
        _ => Err(storage_error()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_encodes_bytes() {
        assert_eq!(hex("a\"{"), "61227b");
    }

    #[test]
    fn rejects_labels_that_could_inject_commands() {
        assert!(check_label("t212-portfolio").is_ok());
        assert!(check_label("x; delete-keychain").is_err());
        assert!(check_label("").is_err());
    }

    /// Writes to the real login keychain under a throwaway service name.
    /// Opt-in: `T212_KEYCHAIN_TEST=1 cargo test -p t212-creds`.
    #[test]
    fn real_keychain_roundtrip() {
        if std::env::var("T212_KEYCHAIN_TEST").is_err() {
            return;
        }
        let svc = "t212-portfolio-selftest";
        let secret = r#"{"key":"k","secret":"s p\"ace","environment":"demo"}"#;
        add(svc, "default", secret).unwrap();
        assert_eq!(find(svc, "default").unwrap().as_deref(), Some(secret));
        add(svc, "default", "second").unwrap();
        assert_eq!(find(svc, "default").unwrap().as_deref(), Some("second"));
        assert!(delete(svc, "default").unwrap());
        assert_eq!(find(svc, "default").unwrap(), None);
    }
}
