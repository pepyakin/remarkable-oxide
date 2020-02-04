use std::env;

pub struct Config {
    /// The path where to store the data to persist. See persist.rs
    pub persisted_data_path: String,
    /// The hostname where to find the RPC endpoint exposed by a substrate node.
    pub rpc_hostname: String,
    /// Should the app go fullscreen?
    pub fullscreen: bool,
}

impl Default for Config {
    fn default() -> Self {
        // The defaults are selected in such a way that the production launch won't require any
        // settings.
        Config {
            persisted_data_path: "persisted_data".to_string(),
            rpc_hostname: "ws://localhost:1234".to_string(),
            fullscreen: true,
        }
    }
}

fn persisted_data_path() -> Option<String> {
    env::var("PERSISTED_DATA_PATH").ok()
}

fn rpc_hostname() -> Option<String> {
    env::var("RPC_HOST").ok()
}

fn fullscreen_enabled() -> Option<bool> {
    let mut fullscreen_var = env::var("FULLSCREEN").ok()?;
    fullscreen_var.make_ascii_lowercase();
    Some(match &*fullscreen_var {
        "0" | "false" | "no" | "n" => false,
        _ => true,
    })
}

/// Read the config file.
pub fn obtain() -> Config {
    // First, make sure that we've loaded configuration from the .env file.
    let _ = dotenv::dotenv();

    // Read the config or use the defaults.
    let defaults = Config::default();
    Config {
        persisted_data_path: persisted_data_path().unwrap_or(defaults.persisted_data_path),
        rpc_hostname: rpc_hostname().unwrap_or(defaults.rpc_hostname),
        fullscreen: fullscreen_enabled().unwrap_or(defaults.fullscreen),
    }
}
