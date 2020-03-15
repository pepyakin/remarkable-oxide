//! A crate that provides a service that connects to a remote substrate node, downloads blocks
//! and extract commands from it.

#![recursion_limit = "1024"]

mod block;
mod block_query;
mod chain_data;
mod comm;
mod command;
mod config;
mod extendable_range;
mod latest;
mod persist;
mod watchdog;
mod service;

pub use config::Config;
pub use service::{Service, StatusReport, start};
pub use command::Command;
pub use comm::Status as CommStatus;
