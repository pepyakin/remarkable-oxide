use std::env;

pub struct Config {
    pub persisted_data_path: String,
    pub rpc_hostname: String,
    pub fullscreen: bool,
}

fn persisted_data_path() -> String {
    env::var("PERSISTED_DATA_PATH").unwrap_or_else(|_| "persisted_data".to_string())
}

fn rpc_hostname() -> String {
    env::var("RPC_HOST").unwrap_or_else(|_| "ws://localhost:1234".to_string())
}

fn fullscreen_enabled() -> bool {
    let mut fullscreen_var = match env::var("FULLSCREEN") {
        Ok(fullscreen_var) => fullscreen_var,
        Err(_) => return true,
    };
    fullscreen_var.make_ascii_lowercase();

    match &*fullscreen_var {
        "0" | "false" | "no" | "n" => false,
        _ => true,
    }
}

/// Read the config file.
pub fn obtain() -> Config {
    // First, make sure that we've loaded configuration from the .env file.
    let _ = dotenv::dotenv();

    // Read the config.
    Config {
        persisted_data_path: persisted_data_path(),
        rpc_hostname: rpc_hostname(),
        fullscreen: fullscreen_enabled(),
    }
}
