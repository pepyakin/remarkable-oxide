#[derive(Clone)]
pub struct Config {
    /// The path where to store the data to persist. See persist.rs
    pub persisted_data_path: String,
    /// The hostname where to find the RPC endpoint exposed by a substrate node.
    pub rpc_hostname: String,
    /// Should the app go fullscreen?
    pub fullscreen: bool,
    /// Should the app hide cursor?
    pub hide_cursor: bool,
}
