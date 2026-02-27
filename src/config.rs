use std::{env, net::SocketAddr};

use ipnet::IpNet;

use thiserror::Error;

#[derive(Debug, Clone)]
pub struct Config {
    pub api_token: String,
    pub bind_addr: String,
    pub bind_port: u16,
    pub allowed_cidr: Option<IpNet>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("MCP_API_TOKEN is required and must not be empty")]
    MissingApiToken,
    #[error("BIND_PORT must be a valid u16")]
    InvalidPort,
    #[error("MCP_ALLOWED_CIDR must be a valid CIDR range")]
    InvalidAllowedCidr,
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

        let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1".to_string());
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

        let config = Self {
            api_token,
            bind_addr,
            bind_port,
            allowed_cidr,
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
        env::set_var("MCP_API_TOKEN", "abc");
        env::remove_var("BIND_ADDR");
        env::remove_var("BIND_PORT");
        env::remove_var("MCP_ALLOWED_CIDR");

        let config = Config::from_env().expect("config should parse");
        assert_eq!(config.bind_addr, "127.0.0.1");
        assert_eq!(config.bind_port, 8080);
        assert_eq!(config.allowed_cidr, None);
    }

    #[test]
    fn missing_token_fails() {
        env::remove_var("MCP_API_TOKEN");

        let err = Config::from_env().expect_err("expected missing token error");
        assert!(matches!(err, ConfigError::MissingApiToken));
    }

    #[test]
    fn allowed_cidr_parses_when_valid() {
        env::set_var("MCP_API_TOKEN", "abc");
        env::set_var("MCP_ALLOWED_CIDR", "10.0.0.0/8");

        let config = Config::from_env().expect("config should parse");
        assert_eq!(
            config.allowed_cidr,
            Some("10.0.0.0/8".parse().expect("valid cidr"))
        );
    }

    #[test]
    fn invalid_allowed_cidr_fails() {
        env::set_var("MCP_API_TOKEN", "abc");
        env::set_var("MCP_ALLOWED_CIDR", "not-a-cidr");

        let err = Config::from_env().expect_err("expected invalid cidr error");
        assert!(matches!(err, ConfigError::InvalidAllowedCidr));
    }
}
