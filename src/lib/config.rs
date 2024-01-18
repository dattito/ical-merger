use serde::Deserialize;

use super::error::Result;

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub urls: Vec<String>,

    #[serde(default = "default_tz_offsets")]
    pub tz_offsets: Vec<i64>,

    #[serde(default = "default_host")]
    pub host: String,

    #[serde(default = "default_port")]
    pub port: u32,

    #[serde(default = "default_hide_details")]
    pub hide_details: bool,

    #[serde(default = "default_merge_overlapping_events")]
    pub merge_overlapping_events: bool,
}

fn default_port() -> u32 {
    3000
}

fn default_host() -> String {
    "0.0.0.0".into()
}

fn default_hide_details() -> bool {
    true
}

fn default_tz_offsets() -> Vec<i64> {
    Vec::new()
}

fn default_merge_overlapping_events() -> bool {
    true
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        match (self.hide_details, self.merge_overlapping_events) {
            (false, true) => Err(super::error::Error::Config(
                "MERGE_OVERLAPPING_EVENTS cannot be used without HIDE DETAILS".into(),
            )),
            _ => Ok(()),
        }
    }
}
