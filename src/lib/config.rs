use std::{env, fs, net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};

use chrono_tz::Tz;
use serde::Deserialize;
use url::Url;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    #[default]
    Busy,
    Title,
}

#[derive(Clone, Debug)]
pub struct SourceConfig {
    pub url: Url,
    pub timezone: Option<Tz>,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub listen: SocketAddr,
    pub default_timezone: Tz,
    pub horizon_days: u32,
    pub refresh_interval: Duration,
    pub output: OutputMode,
    pub sources: Vec<SourceConfig>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("set exactly one of ICAL_MERGER_CONFIG or ICAL_MERGER_CONFIG_FILE")]
    ConfigSource,
    #[error("could not read configuration file {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid TOML configuration at line {line}, column {column}: {message}")]
    Toml {
        message: String,
        line: usize,
        column: usize,
    },
    #[error("invalid configuration field `{field}`: {reason}")]
    Invalid { field: String, reason: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default = "default_listen")]
    listen: String,
    default_timezone: String,
    #[serde(default = "default_horizon_days")]
    horizon_days: u32,
    #[serde(default = "default_refresh_seconds")]
    refresh_seconds: u64,
    #[serde(default)]
    output: OutputMode,
    sources: Vec<RawSource>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSource {
    url: String,
    timezone: Option<String>,
}

fn default_listen() -> String {
    "0.0.0.0:3000".into()
}

fn default_horizon_days() -> u32 {
    90
}

fn default_refresh_seconds() -> u64 {
    15 * 60
}

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        let inline = env::var_os("ICAL_MERGER_CONFIG");
        let path = env::var_os("ICAL_MERGER_CONFIG_FILE").map(PathBuf::from);
        match (inline, path) {
            (Some(_), Some(_)) | (None, None) => Err(ConfigError::ConfigSource),
            (Some(contents), None) => {
                let contents = contents.into_string().map_err(|_| {
                    ConfigError::invalid("ICAL_MERGER_CONFIG", "must contain valid UTF-8 TOML text")
                })?;
                Self::parse(&contents)
            }
            (None, Some(path)) => {
                let contents =
                    fs::read_to_string(&path).map_err(|source| ConfigError::ReadFile {
                        path: path.clone(),
                        source,
                    })?;
                Self::parse(&contents)
            }
        }
    }

    pub fn parse(contents: &str) -> Result<Self, ConfigError> {
        let raw: RawConfig = toml::from_str(contents).map_err(|error| {
            let (line, column) = error
                .span()
                .map(|span| {
                    let prefix = contents.get(..span.start.min(contents.len())).unwrap_or("");
                    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
                    let column = prefix
                        .rsplit_once('\n')
                        .map_or(prefix.len() + 1, |(_, tail)| tail.len() + 1);
                    (line, column)
                })
                .unwrap_or((1, 1));
            ConfigError::Toml {
                message: error.message().to_owned(),
                line,
                column,
            }
        })?;

        if raw.listen.trim().is_empty() {
            return Err(ConfigError::invalid("listen", "must not be empty"));
        }
        let listen = SocketAddr::from_str(&raw.listen).map_err(|_| {
            ConfigError::invalid(
                "listen",
                "must be an IP socket address such as 0.0.0.0:3000 or [::]:3000",
            )
        })?;
        if raw.horizon_days == 0 {
            return Err(ConfigError::invalid(
                "horizon_days",
                "must be greater than zero",
            ));
        }
        if raw.refresh_seconds == 0 {
            return Err(ConfigError::invalid(
                "refresh_seconds",
                "must be greater than zero",
            ));
        }
        if raw.sources.is_empty() {
            return Err(ConfigError::invalid(
                "sources",
                "must contain at least one URL",
            ));
        }

        let default_timezone = Tz::from_str(&raw.default_timezone).map_err(|_| {
            ConfigError::invalid("default_timezone", "must be a valid IANA timezone")
        })?;

        let sources = raw
            .sources
            .into_iter()
            .enumerate()
            .map(|(index, source)| {
                let field = format!("sources[{index}].url");
                let url = Url::parse(&source.url).map_err(|_| {
                    ConfigError::invalid(&field, "must be an absolute HTTP or HTTPS URL")
                })?;
                if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                    return Err(ConfigError::invalid(
                        &field,
                        "must use HTTP or HTTPS and include a host",
                    ));
                }
                let timezone = source
                    .timezone
                    .map(|timezone| {
                        Tz::from_str(&timezone).map_err(|_| {
                            ConfigError::invalid(
                                format!("sources[{index}].timezone"),
                                "must be a valid IANA timezone",
                            )
                        })
                    })
                    .transpose()?;
                Ok(SourceConfig { url, timezone })
            })
            .collect::<Result<Vec<_>, ConfigError>>()?;

        Ok(Self {
            listen,
            default_timezone,
            horizon_days: raw.horizon_days,
            refresh_interval: Duration::from_secs(raw.refresh_seconds),
            output: raw.output,
            sources,
        })
    }
}

impl ConfigError {
    fn invalid(field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Invalid {
            field: field.into(),
            reason: reason.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
default_timezone = "Europe/Berlin"
[[sources]]
url = "https://calendar.example/calendar.ics"
"#;

    #[test]
    fn parses_defaults_and_timezone_override() {
        let config = Config::parse(VALID).unwrap();
        assert_eq!(config.listen, "0.0.0.0:3000".parse::<SocketAddr>().unwrap());
        assert_eq!(config.horizon_days, 90);
        assert_eq!(config.refresh_interval, Duration::from_secs(900));
        assert_eq!(config.output, OutputMode::Busy);
        assert_eq!(config.default_timezone, chrono_tz::Europe::Berlin);

        let config = Config::parse(
            r#"
default_timezone = "Europe/Berlin"
[[sources]]
url = "https://calendar.example/calendar.ics"
timezone = "America/New_York"
"#,
        )
        .unwrap();
        assert_eq!(
            config.sources[0].timezone,
            Some(chrono_tz::America::New_York)
        );
    }

    #[test]
    fn rejects_empty_sources_and_invalid_zones() {
        assert!(Config::parse("default_timezone = \"UTC\"\nsources = []").is_err());
        assert!(Config::parse(
            "default_timezone = \"Not/AZone\"\n[[sources]]\nurl = \"https://example.test/a.ics\""
        )
        .is_err());
    }

    #[test]
    fn rejects_non_http_urls_and_unknown_fields() {
        assert!(Config::parse(
            "default_timezone = \"UTC\"\n[[sources]]\nurl = \"file:///calendar.ics\""
        )
        .is_err());
        assert!(Config::parse(
            "default_timezone = \"UTC\"\nurls = [\"https://example.test/a.ics\"]"
        )
        .is_err());
    }

    #[test]
    fn validation_messages_name_fields_without_echoing_url_credentials() {
        let listen_error = Config::parse(
            "listen = \"not-an-address\"\ndefault_timezone = \"UTC\"\n[[sources]]\nurl = \"https://example.test/a.ics\"",
        )
        .unwrap_err()
        .to_string();
        assert!(listen_error.contains("listen"));
        assert!(listen_error.contains("IP socket address"));

        let url_error = Config::parse(
            r#"
default_timezone = "UTC"
[[sources]]
url = "ftp://user:secret@example.test/calendar.ics"
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(url_error.contains("sources[0].url"));
        assert!(url_error.contains("HTTP or HTTPS"));
        assert!(!url_error.contains("secret"));
    }
}
