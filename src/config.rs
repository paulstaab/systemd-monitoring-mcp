use std::{env, net::SocketAddr};

use ipnet::IpNet;

use thiserror::Error;

const MIN_API_TOKEN_LENGTH: usize = 16;

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
        let api_token = env::var("MCP_API_TOKEN")
            .ok()
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty())
            .ok_or(ConfigError::MissingApiToken)?;

        if api_token.len() < MIN_API_TOKEN_LENGTH {
            return Err(ConfigError::TokenTooShort);
        }

        let bind_addr = env::var("BIND_ADDR")
            .ok()
            .map(|addr| addr.trim().to_string())
            .filter(|addr| !addr.is_empty())
            .unwrap_or_else(|| "127.0.0.1".to_string());
        let bind_port = env::var("BIND_PORT")
            .ok()
            .map(|value| value.parse::<u16>().map_err(|_| ConfigError::InvalidPort))
            .transpose()?
            .unwrap_or(8080);
        let allowed_cidr = env::var("MCP_ALLOWED_CIDR")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(|value| {
                value
                    .parse::<IpNet>()
                    .map_err(|_| ConfigError::InvalidAllowedCidr)
            })
            .transpose()?;

        let trusted_proxies = env::var("MCP_TRUSTED_PROXIES")
            .ok()
            .map(|value| value.trim().to_string())
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

    #[test]
    fn parse_defaults() {
        env::set_var("MCP_API_TOKEN", "abcdefghijklmnop");
        env::remove_var("BIND_ADDR");
        env::remove_var("BIND_PORT");
        env::remove_var("MCP_ALLOWED_CIDR");
        env::remove_var("MCP_TRUSTED_PROXIES");

        let config = Config::from_env().expect("config should parse");
        assert_eq!(config.bind_addr, "127.0.0.1");
        assert_eq!(config.bind_port, 8080);
        assert_eq!(config.allowed_cidr, None);
        assert!(config.trusted_proxies.is_empty());
    }

    #[test]
    fn missing_token_fails() {
        env::remove_var("MCP_API_TOKEN");

        let err = Config::from_env().expect_err("expected missing token error");
        assert!(matches!(err, ConfigError::MissingApiToken));
    }

    #[test]
    fn short_token_fails() {
        env::set_var("MCP_API_TOKEN", "short");
        env::remove_var("MCP_ALLOWED_CIDR");
        env::remove_var("MCP_TRUSTED_PROXIES");

        let err = Config::from_env().expect_err("expected short token error");
        assert!(matches!(err, ConfigError::TokenTooShort));
    }

    #[test]
    fn allowed_cidr_parses_when_valid() {
        env::set_var("MCP_API_TOKEN", "abcdefghijklmnop");
        env::set_var("MCP_ALLOWED_CIDR", "10.0.0.0/8");
        env::remove_var("MCP_TRUSTED_PROXIES");

        let config = Config::from_env().expect("config should parse");
        assert_eq!(
            config.allowed_cidr,
            Some("10.0.0.0/8".parse().expect("valid cidr"))
        );
    }

    #[test]
    fn invalid_allowed_cidr_fails() {
        env::set_var("MCP_API_TOKEN", "abcdefghijklmnop");
        env::set_var("MCP_ALLOWED_CIDR", "not-a-cidr");

        let err = Config::from_env().expect_err("expected invalid cidr error");
        assert!(matches!(err, ConfigError::InvalidAllowedCidr));
    }

    #[test]
    fn trusted_proxies_parses() {
        env::set_var("MCP_API_TOKEN", "abcdefghijklmnop");
        env::remove_var("MCP_ALLOWED_CIDR");
        env::set_var("MCP_TRUSTED_PROXIES", "10.0.0.1/32, 172.16.0.0/12");

        let config = Config::from_env().expect("config should parse");
        assert_eq!(config.trusted_proxies.len(), 2);
    }
}
