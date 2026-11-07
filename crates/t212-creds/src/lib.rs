//! Credential storage in the OS secret store. Fails closed: if secure storage
//! is unavailable, nothing is written anywhere else (no plaintext fallback).
//!
//! Stored as one generic item (service `t212-portfolio`, account `default`);
//! the secret never appears in the item name. Values are never logged.

use serde::{Deserialize, Serialize};
use t212_api::{Credentials, Environment};
use t212_core::error::{ErrorCode, T212Error};

pub const SERVICE: &str = "t212-portfolio";
pub const ACCOUNT: &str = "default";

#[derive(Clone)]
pub struct Stored {
    pub credentials: Credentials,
    pub environment: Environment,
}

#[derive(Serialize, Deserialize)]
struct Blob {
    key: String,
    secret: String,
    environment: String,
}

pub trait CredentialStore {
    fn load(&self) -> Result<Option<Stored>, T212Error>;
    fn save(&self, s: &Stored) -> Result<(), T212Error>;
    /// Returns true if something was deleted.
    fn delete(&self) -> Result<bool, T212Error>;
}

pub(crate) fn storage_error() -> T212Error {
    T212Error::new(
        ErrorCode::SecureStorageUnavailable,
        "the OS secure credential store is unavailable or locked; unlock it and retry. \
         Credentials are never saved anywhere else.",
    )
}

fn encode(s: &Stored) -> Result<String, T212Error> {
    serde_json::to_string(&Blob {
        key: s.credentials.key.clone(),
        secret: s.credentials.secret.clone(),
        environment: s.environment.as_str().to_owned(),
    })
    .map_err(|_| storage_error())
}

fn decode(raw: &str) -> Result<Stored, T212Error> {
    let b: Blob = serde_json::from_str(raw).map_err(|_| {
        T212Error::new(
            ErrorCode::SecureStorageUnavailable,
            "stored credentials are unreadable; run `t212 setup` again",
        )
    })?;
    let environment = Environment::parse(&b.environment).ok_or_else(storage_error)?;
    Ok(Stored {
        credentials: Credentials {
            key: b.key,
            secret: b.secret,
        },
        environment,
    })
}

#[cfg(target_os = "macos")]
mod macos;

/// The platform's secure credential store.
///
/// macOS: login keychain, accessed through Apple's signed `/usr/bin/security`
/// tool so the item's access list names a stable, Apple-signed binary. This
/// avoids repeated password prompts for the unsigned `t212` binary.
/// Windows/Linux: native APIs via
/// the `keyring` crate.
pub struct OsStore;

#[cfg(target_os = "macos")]
impl CredentialStore for OsStore {
    fn load(&self) -> Result<Option<Stored>, T212Error> {
        macos::find(SERVICE, ACCOUNT)?
            .as_deref()
            .map(decode)
            .transpose()
    }
    fn save(&self, s: &Stored) -> Result<(), T212Error> {
        macos::add(SERVICE, ACCOUNT, &encode(s)?)
    }
    fn delete(&self) -> Result<bool, T212Error> {
        macos::delete(SERVICE, ACCOUNT)
    }
}

#[cfg(not(target_os = "macos"))]
impl OsStore {
    fn entry() -> Result<keyring::Entry, T212Error> {
        keyring::Entry::new(SERVICE, ACCOUNT).map_err(|_| storage_error())
    }
}

#[cfg(not(target_os = "macos"))]
impl CredentialStore for OsStore {
    fn load(&self) -> Result<Option<Stored>, T212Error> {
        match Self::entry()?.get_password() {
            Ok(raw) => decode(&raw).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(storage_error()),
        }
    }
    fn save(&self, s: &Stored) -> Result<(), T212Error> {
        Self::entry()?
            .set_password(&encode(s)?)
            .map_err(|_| storage_error())
    }
    fn delete(&self) -> Result<bool, T212Error> {
        match Self::entry()?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(_) => Err(storage_error()),
        }
    }
}

/// In-memory store for tests.
#[derive(Default)]
pub struct MemoryStore(std::sync::Mutex<Option<String>>);

impl CredentialStore for MemoryStore {
    fn load(&self) -> Result<Option<Stored>, T212Error> {
        self.0.lock().unwrap().as_deref().map(decode).transpose()
    }
    fn save(&self, s: &Stored) -> Result<(), T212Error> {
        *self.0.lock().unwrap() = Some(encode(s)?);
        Ok(())
    }
    fn delete(&self) -> Result<bool, T212Error> {
        Ok(self.0.lock().unwrap().take().is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_delete() {
        let s = MemoryStore::default();
        assert!(s.load().unwrap().is_none());
        s.save(&Stored {
            credentials: Credentials {
                key: "k".into(),
                secret: "s".into(),
            },
            environment: Environment::Demo,
        })
        .unwrap();
        let got = s.load().unwrap().unwrap();
        assert_eq!(got.credentials.key, "k");
        assert_eq!(got.environment, Environment::Demo);
        assert!(s.delete().unwrap());
        assert!(!s.delete().unwrap());
    }
}
