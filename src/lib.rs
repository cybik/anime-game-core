pub mod network;
pub mod filesystem;
pub mod game;
pub mod archive;
pub mod updater;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
