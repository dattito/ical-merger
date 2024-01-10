use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub urls: Vec<String>,
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u32,
}

fn default_port() -> u32 {
    3000
}

fn default_host() -> String {
    "0.0.0.0".into()
}
