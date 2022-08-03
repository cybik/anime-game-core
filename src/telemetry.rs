use std::time::Duration;

use crate::curl::fetch;
use crate::consts::TELEMETRY_SERVERS;

/// Check whether telemetry servers disabled
/// 
/// If some of them is not disabled, then this function will return its address
/// 
/// ```
/// use anime_game_core::telemetry;
/// 
/// if let None = telemetry::is_disabled(None) {
///     println!("Telemetry is disabled");
/// }
/// ```
pub fn is_disabled(timeout: Option<Duration>) -> Option<String> {
    for server in TELEMETRY_SERVERS {
        if let Ok(_) = fetch(server, timeout) {
            return Some(server.to_string());
        }
    }

    None
}