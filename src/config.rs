use std::{env, net::SocketAddr};

use ipnet::IpNet;
use thiserror::Error;

const MIN_API_TOKEN_LENGTH: usize = 16;

#[derive(Debug, Clone)]
struct RawConfig {
    api_token: Option<String>,
    bind_addr: Option<String>,
    bind_port: Option<String>,
    allowed_cidr: Option<String>,
    trusted_proxies: Option<String>,
}

impl RawConfig {
    fn from_env() -> Self {
        Self {
            api_token: env::var("MCP_API_TOKEN").ok(),
            bind_addr: env::var("BIND_ADDR").ok(),
            bind_port: env::var("BIND_PORT").ok(),
            allowed_cidr: env::var("MCP_ALLOWED_CIDR").ok(),
            trusted_proxies: env::var("MCP_TRUSTED_PROXIES").ok(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub api_token: String,
    pub bind_addr: String,
    pub bind_port: u16,
    pub allowed_cidr: Option<IpNet>,
    pub trusted_proxies: Vec<IpNet>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("MCP_API_TOKEN is required and must not be empty")]
    MissingApiToken,
    #[error("MCP_API_TOKEN must be at least {MIN_API_TOKEN_LENGTH} characters")]
    TokenTooShort,
    #[error("BIND_PORT must be a valid u16")]
    InvalidPort,
    #[error("MCP_ALLOWED_CIDR must be a valid CIDR range")]
    InvalidAllowedCidr,
    #[error("MCP_TRUSTED_PROXIES contains an invalid CIDR range")]
    InvalidTrustedProxy,
    #[error("invalid bind address or port")]
    InvalidSocket,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::parse(RawConfig::from_env())
    }

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

        let allowed_cidr = raw
            .allowed_cidr
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                value
                    .parse::<IpNet>()
                    .map_err(|_| ConfigError::InvalidAllowedCidr)
            })
            .transpose()?;

        let trusted_proxies = raw
            .trusted_proxies
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                value
                    .split(',')
                    .map(|entry| {
                        entry
                            .trim()
                            .parse::<IpNet>()
                            .map_err(|_| ConfigError::InvalidTrustedProxy)
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();

        let config = Self {
            api_token,
            bind_addr,
            bind_port,
            allowed_cidr,
            trusted_proxies,
        };

        let _ = config.bind_socket()?;
        Ok(config)
    }

    pub fn bind_socket(&self) -> Result<SocketAddr, ConfigError> {
        format!("{}:{}", self.bind_addr, self.bind_port)
            .parse::<SocketAddr>()
            .map_err(|_| ConfigError::InvalidSocket)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_config(
        api_token: Option<&str>,
        bind_addr: Option<&str>,
        bind_port: Option<&str>,
        allowed_cidr: Option<&str>,
        trusted_proxies: Option<&str>,
    ) -> RawConfig {
        RawConfig {
            api_token: api_token.map(ToString::to_string),
            bind_addr: bind_addr.map(ToString::to_string),
            bind_port: bind_port.map(ToString::to_string),
            allowed_cidr: allowed_cidr.map(ToString::to_string),
            trusted_proxies: trusted_proxies.map(ToString::to_string),
        }
    }

    #[test]
    fn parse_defaults() {
        let raw = raw_config(Some("abcdefghijklmnop"), None, None, None, None);

        let config = Config::parse(raw).expect("config should parse");
        assert_eq!(config.bind_addr, "127.0.0.1");
        assert_eq!(config.bind_port, 8080);
        assert_eq!(config.allowed_cidr, None);
        assert!(config.trusted_proxies.is_empty());
    }

    #[test]
    fn missing_token_fails() {
        let raw = raw_config(None, None, None, None, None);

        let err = Config::parse(raw).expect_err("expected missing token error");
        assert!(matches!(err, ConfigError::MissingApiToken));
    }

    #[test]
    fn short_token_fails() {
        let raw = raw_config(Some("short"), None, None, None, None);

        let err = Config::parse(raw).expect_err("expected short token error");
        assert!(matches!(err, ConfigError::TokenTooShort));
    }

    #[test]
    fn allowed_cidr_parses_when_valid() {
        let raw = raw_config(Some("abcdefghijklmnop"), None, None, Some("10.0.0.0/8"), None);

        let config = Config::parse(raw).expect("config should parse");
        assert_eq!(
            config.allowed_cidr,
            Some("10.0.0.0/8".parse().expect("valid cidr"))
        );
    }

    #[test]
    fn invalid_allowed_cidr_fails() {
        let raw = raw_config(
            Some("abcdefghijklmnop"),
            None,
            None,
            Some("not-a-cidr"),
            None,
        );

        let err = Config::parse(raw).expect_err("expected invalid cidr error");
        assert!(matches!(err, ConfigError::InvalidAllowedCidr));
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

        let err = Config::parse(raw).expect_err("expected invalid port error");
        assert!(matches!(err, ConfigError::InvalidPort));
    }

    #[test]
    fn trusted_proxies_parses() {
        let raw = raw_config(
            Some("abcdefghijklmnop"),
            None,
            None,
            None,
            Some("10.0.0.1/32, 172.16.0.0/12"),
        );

        let config = Config::parse(raw).expect("config should parse");
        assert_eq!(config.trusted_proxies.len(), 2);
    }

    #[test]
    fn invalid_trusted_proxy_fails() {
        let raw = raw_config(
            Some("abcdefghijklmnop"),
            None,
            None,
            None,
            Some("10.0.0.1/32, not-a-cidr"),
        );

        let err = Config::parse(raw).expect_err("expected invalid trusted proxy error");
        assert!(matches!(err, ConfigError::InvalidTrustedProxy));
    }
}
