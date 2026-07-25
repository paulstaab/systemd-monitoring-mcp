use std::{env, net::SocketAddr};

use thiserror::Error;

use crate::rate_limit::{
    RateLimitPolicy, DEFAULT_BURST, DEFAULT_REQUESTS_PER_SECOND, MAX_BURST, MAX_REQUESTS_PER_SECOND,
};

const MIN_API_TOKEN_LENGTH: usize = 16;

#[derive(Debug, Clone)]
struct RawConfig {
    api_token: Option<String>,
    bind_addr: Option<String>,
    bind_port: Option<String>,
    rate_limit_requests_per_second: Option<String>,
    rate_limit_burst: Option<String>,
}

impl RawConfig {
    /// Loads raw environment configuration without validation.
    ///
    /// Validation and defaults are applied later in `Config::parse`.
    fn from_env() -> Self {
        Self {
            api_token: env::var("MCP_API_TOKEN").ok(),
            bind_addr: env::var("BIND_ADDR").ok(),
            bind_port: env::var("BIND_PORT").ok(),
            rate_limit_requests_per_second: env::var("RATE_LIMIT_REQUESTS_PER_SECOND").ok(),
            rate_limit_burst: env::var("RATE_LIMIT_BURST").ok(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub api_token: String,
    pub bind_addr: String,
    pub bind_port: u16,
    pub rate_limit_requests_per_second: u32,
    pub rate_limit_burst: u32,
}

#[derive(Clone, Copy, Debug, Error)]
pub enum ConfigError {
    #[error("MCP_API_TOKEN is required and must not be empty")]
    MissingApiToken,
    #[error("MCP_API_TOKEN must be at least {MIN_API_TOKEN_LENGTH} characters")]
    TokenTooShort,
    #[error("BIND_PORT must be a valid u16")]
    InvalidPort,
    #[error("invalid bind address or port")]
    InvalidSocket,
    #[error(
        "RATE_LIMIT_REQUESTS_PER_SECOND must be an integer between 1 and {MAX_REQUESTS_PER_SECOND}"
    )]
    InvalidRateLimitRequestsPerSecond,
    #[error("RATE_LIMIT_BURST must be an integer between 1 and {MAX_BURST}")]
    InvalidRateLimitBurst,
}

impl Config {
    /// Builds validated runtime config from environment variables.
    ///
    /// Applies defaults for optional bind and rate-limit settings, and validates
    /// token length plus all numeric bounds.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::parse(RawConfig::from_env())
    }

    /// Validates and normalizes a raw config snapshot.
    ///
    /// Ensures required token constraints, validates bounded rate-limit values,
    /// and confirms that bind address/port can form a valid socket.
    fn parse(raw: RawConfig) -> Result<Self, ConfigError> {
        let api_token = raw
            .api_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(ToString::to_string)
            .ok_or(ConfigError::MissingApiToken)?;

        if api_token.len() < MIN_API_TOKEN_LENGTH {
            return Err(ConfigError::TokenTooShort);
        }

        let bind_addr = raw
            .bind_addr
            .as_deref()
            .map(str::trim)
            .filter(|addr| !addr.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| "127.0.0.1".to_string());

        let bind_port = raw
            .bind_port
            .as_deref()
            .map(|value| value.parse::<u16>().map_err(|_| ConfigError::InvalidPort))
            .transpose()?
            .unwrap_or(8080);

        let rate_limit_requests_per_second = parse_bounded_u32(
            raw.rate_limit_requests_per_second.as_deref(),
            DEFAULT_REQUESTS_PER_SECOND,
            MAX_REQUESTS_PER_SECOND,
            ConfigError::InvalidRateLimitRequestsPerSecond,
        )?;
        let rate_limit_burst = parse_bounded_u32(
            raw.rate_limit_burst.as_deref(),
            DEFAULT_BURST,
            MAX_BURST,
            ConfigError::InvalidRateLimitBurst,
        )?;

        let config = Self {
            api_token,
            bind_addr,
            bind_port,
            rate_limit_requests_per_second,
            rate_limit_burst,
        };

        let _ = config.bind_socket()?;
        Ok(config)
    }

    /// Converts bind host/port settings into a concrete socket address.
    ///
    /// Returns `InvalidSocket` when address formatting or parsing fails.
    pub fn bind_socket(&self) -> Result<SocketAddr, ConfigError> {
        format!("{}:{}", self.bind_addr, self.bind_port)
            .parse::<SocketAddr>()
            .map_err(|_| ConfigError::InvalidSocket)
    }

    /// Converts validated rate settings into the runtime limiter policy.
    ///
    /// Configuration parsing has already enforced the policy bounds, so this
    /// conversion cannot fail unless those invariants are changed inconsistently.
    pub fn rate_limit_policy(&self) -> RateLimitPolicy {
        RateLimitPolicy::new(self.rate_limit_requests_per_second, self.rate_limit_burst)
            .expect("validated configuration must produce a valid rate-limit policy")
    }
}

/// Parses an optional positive bounded integer, applying a default when absent.
///
/// Empty, malformed, zero, overflowing, and above-maximum values return the
/// caller-provided field-specific error.
fn parse_bounded_u32(
    raw: Option<&str>,
    default: u32,
    maximum: u32,
    error: ConfigError,
) -> Result<u32, ConfigError> {
    let Some(raw) = raw else {
        return Ok(default);
    };
    let value = raw.trim().parse::<u32>().map_err(|_| error)?;
    if value == 0 || value > maximum {
        return Err(error);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a raw snapshot with optional rate-limit fields for parser tests.
    fn raw_config(
        api_token: Option<&str>,
        bind_addr: Option<&str>,
        bind_port: Option<&str>,
        rate_limit_requests_per_second: Option<&str>,
        rate_limit_burst: Option<&str>,
    ) -> RawConfig {
        RawConfig {
            api_token: api_token.map(ToString::to_string),
            bind_addr: bind_addr.map(ToString::to_string),
            bind_port: bind_port.map(ToString::to_string),
            rate_limit_requests_per_second: rate_limit_requests_per_second.map(ToString::to_string),
            rate_limit_burst: rate_limit_burst.map(ToString::to_string),
        }
    }

    #[test]
    fn parse_defaults() {
        let raw = raw_config(Some("abcdefghijklmnop"), None, None, None, None);

        let config = Config::parse(raw).expect("config should parse");
        assert_eq!(config.bind_addr, "127.0.0.1");
        assert_eq!(config.bind_port, 8080);
        assert_eq!(
            config.rate_limit_requests_per_second,
            DEFAULT_REQUESTS_PER_SECOND
        );
        assert_eq!(config.rate_limit_burst, DEFAULT_BURST);
    }

    #[test]
    fn missing_token_fails() {
        let raw = raw_config(None, None, None, None, None);
        assert!(matches!(
            Config::parse(raw),
            Err(ConfigError::MissingApiToken)
        ));
    }

    #[test]
    fn short_token_fails() {
        let raw = raw_config(Some("short"), None, None, None, None);
        assert!(matches!(
            Config::parse(raw),
            Err(ConfigError::TokenTooShort)
        ));
    }

    #[test]
    fn invalid_port_fails() {
        let raw = raw_config(
            Some("abcdefghijklmnop"),
            None,
            Some("not-a-port"),
            None,
            None,
        );
        assert!(matches!(Config::parse(raw), Err(ConfigError::InvalidPort)));
    }

    /// Verifies explicit in-range rate and burst values are retained.
    #[test]
    fn configured_rate_limit_values_parse() {
        let raw = raw_config(Some("abcdefghijklmnop"), None, None, Some("25"), Some("50"));

        let config = Config::parse(raw).expect("config should parse");
        assert_eq!(config.rate_limit_requests_per_second, 25);
        assert_eq!(config.rate_limit_burst, 50);
    }

    /// Verifies zero, malformed, overflowing, high, and empty rates fail.
    #[test]
    fn invalid_rate_values_fail() {
        for invalid in ["0", "not-a-number", "4294967296", "1000001", ""] {
            let raw = raw_config(Some("abcdefghijklmnop"), None, None, Some(invalid), None);
            assert!(matches!(
                Config::parse(raw),
                Err(ConfigError::InvalidRateLimitRequestsPerSecond)
            ));
        }
    }

    /// Verifies zero, malformed, overflowing, high, and empty bursts fail.
    #[test]
    fn invalid_burst_values_fail() {
        for invalid in ["0", "not-a-number", "4294967296", "1000001", ""] {
            let raw = raw_config(Some("abcdefghijklmnop"), None, None, None, Some(invalid));
            assert!(matches!(
                Config::parse(raw),
                Err(ConfigError::InvalidRateLimitBurst)
            ));
        }
    }
}
