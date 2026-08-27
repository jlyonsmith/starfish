//! Messages exchanged between `starfish_admin` and the controller over a Unix
//! domain socket.
//!
//! The admin tool reads and writes database records itself; this socket exists
//! only so it can nudge the controller, and through it the agents, to pick a
//! change up immediately rather than at the next connection.

use serde::{Deserialize, Serialize};

/// A request from `starfish_admin` to the controller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AdminRequest {
    /// Rebuild host configurations from the database and push them to the
    /// connected agents.
    Refresh {
        /// Limits the refresh to one host. `None` refreshes every host.
        hostname: Option<String>,
    },
}

/// The controller's reply to an [`AdminRequest`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AdminResponse {
    Refreshed {
        /// Hosts whose agents were sent a new configuration.
        notified: Vec<String>,

        /// Hosts with no agent connected. These pick the change up when their
        /// agent next connects.
        offline: Vec<String>,
    },

    Error {
        message: String,
    },
}
