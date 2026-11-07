use crate::Environment;
use std::io::Read;
use std::time::Duration;
use t212_core::error::{ErrorCode, T212Error};

pub const MAX_BODY_BYTES: u64 = 4 * 1024 * 1024;

pub struct RawResponse {
    pub status: u16,
    pub body: Vec<u8>,
    /// Seconds until the rate limit resets, when the provider says so.
    pub retry_after_secs: Option<u64>,
}

/// GET-only transport. Callers pass a path that has already passed the route
/// allowlist; the implementation adds scheme and host from [`Environment`].
pub trait Transport {
    fn get(
        &self,
        env: Environment,
        path: &str,
        authorization: &str,
    ) -> Result<RawResponse, T212Error>;
}

pub struct UreqTransport {
    agent: ureq::Agent,
}

impl Default for UreqTransport {
    fn default() -> Self {
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .https_only(true)
            .timeout(Duration::from_secs(15))
            .user_agent("t212-portfolio")
            .build();
        Self { agent }
    }
}

fn unavailable() -> T212Error {
    T212Error::new(
        ErrorCode::ProviderUnavailable,
        "could not reach Trading 212",
    )
}

impl Transport for UreqTransport {
    fn get(
        &self,
        env: Environment,
        path: &str,
        authorization: &str,
    ) -> Result<RawResponse, T212Error> {
        let url = format!("https://{}{}", env.host(), path);
        let result = self
            .agent
            .get(&url)
            .set("Authorization", authorization)
            .call();
        let resp = match result {
            Ok(r) => r,
            Err(ureq::Error::Status(_, r)) => r,
            Err(_) => return Err(unavailable()),
        };
        let status = resp.status();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let retry_after_secs = resp
            .header("x-ratelimit-reset")
            .and_then(|v| v.parse::<u64>().ok())
            .map(|reset| reset.saturating_sub(now));
        let mut body = Vec::new();
        resp.into_reader()
            .take(MAX_BODY_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|_| unavailable())?;
        if body.len() as u64 > MAX_BODY_BYTES {
            return Err(T212Error::new(
                ErrorCode::CompatibilityError,
                "response too large",
            ));
        }
        Ok(RawResponse {
            status,
            body,
            retry_after_secs,
        })
    }
}
