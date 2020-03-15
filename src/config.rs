use remarkable_oxide_service::Config;
use std::env;

fn default_config() -> Config {
    // The defaults are selected in such a way that the production launch won't require any
    // settings.
    Config {
        persisted_data_path: "persisted_data".to_string(),
        rpc_hostname: "ws://localhost:1234".to_string(),
        fullscreen: true,
        hide_cursor: true,
    }
}

fn persisted_data_path() -> Option<String> {
    env::var("PERSISTED_DATA_PATH").ok()
}

fn rpc_hostname() -> Option<String> {
    env::var("RPC_HOST").ok()
}

fn is_positive_answer(var_text: &str) -> bool {
    match var_text {
        "0" | "false" | "no" | "n" => false,
        _ => true,
    }
}

fn fullscreen_enabled() -> Option<bool> {
    let mut fullscreen_var = env::var("FULLSCREEN").ok()?;
    fullscreen_var.make_ascii_lowercase();
    Some(is_positive_answer(&fullscreen_var))
}

fn hide_cursor() -> Option<bool> {
    let mut hide_cursor_var = env::var("HIDE_CURSOR").ok()?;
    hide_cursor_var.make_ascii_lowercase();
    Some(is_positive_answer(&hide_cursor_var))
}

/// Read the config file.
pub fn obtain() -> Config {
    // First, make sure that we've loaded configuration from the .env file.
    let _ = dotenv::dotenv();

    // Read the config or use the defaults.
    let defaults = default_config();
    Config {
        persisted_data_path: persisted_data_path().unwrap_or(defaults.persisted_data_path),
        rpc_hostname: rpc_hostname().unwrap_or(defaults.rpc_hostname),
        fullscreen: fullscreen_enabled().unwrap_or(defaults.fullscreen),
        hide_cursor: hide_cursor().unwrap_or(defaults.hide_cursor),
    }
}
