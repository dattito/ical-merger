use serde::Deserialize;

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
