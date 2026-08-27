use starfish_msg::ControllerMsg;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;

/// How many messages may be queued for an agent before the controller gives up
/// on it. An agent that has not drained this many is not reading its socket.
const SEND_QUEUE_LEN: usize = 16;

/// Identifies one agent connection.
///
/// An agent that reconnects before the controller notices the old socket died
/// briefly has two sessions for the same host. The newer one replaces the older
/// in the registry, and the id lets the older session recognise that it is no
/// longer the registered connection and leave the entry alone as it exits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionId(u64);

/// The reason a message could not be handed to an agent.
#[derive(Debug, PartialEq, Eq)]
pub enum SendError {
    /// No agent is connected for that host.
    NotConnected,
    /// The agent is connected but not keeping up.
    QueueFull,
}

/// The agents currently connected, keyed by the host they belong to.
pub struct AgentRegistry {
    next_session_id: AtomicU64,
    agents: Mutex<HashMap<u64, Agent>>,
}

struct Agent {
    session_id: SessionId,
    tx: mpsc::Sender<ControllerMsg>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            next_session_id: AtomicU64::new(1),
            agents: Mutex::new(HashMap::new()),
        }
    }

    /// Creates the channel a session writes to and claims the host's slot.
    ///
    /// Returns the session's id and the receiving half, which the caller's
    /// writer task drains. Any agent already registered for `host_id` is
    /// dropped, which closes its channel and ends its session.
    pub fn register(
        &self,
        host_id: u64,
        hostname: &str,
    ) -> (SessionId, mpsc::Receiver<ControllerMsg>) {
        let session_id = SessionId(self.next_session_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = mpsc::channel(SEND_QUEUE_LEN);

        let replaced = self
            .agents
            .lock()
            .unwrap()
            .insert(host_id, Agent { session_id, tx })
            .is_some();

        if replaced {
            log::warn!("Replacing an existing agent connection for host '{hostname}'");
        }

        (session_id, rx)
    }

    /// Releases the host's slot, but only if `session_id` still holds it. A
    /// session that was replaced by a reconnect leaves the newer one in place.
    pub fn unregister(&self, host_id: u64, session_id: SessionId) {
        let mut agents = self.agents.lock().unwrap();

        if agents.get(&host_id).map(|agent| agent.session_id) == Some(session_id) {
            agents.remove(&host_id);
        }
    }

    pub fn is_connected(&self, host_id: u64) -> bool {
        self.agents.lock().unwrap().contains_key(&host_id)
    }

    pub fn connected_count(&self) -> usize {
        self.agents.lock().unwrap().len()
    }

    /// Queues a message for a host's agent.
    ///
    /// This never blocks: a full queue means the agent is not reading, and
    /// waiting on it would stall whoever is pushing configuration.
    pub fn send(&self, host_id: u64, msg: ControllerMsg) -> Result<(), SendError> {
        let agents = self.agents.lock().unwrap();

        let Some(agent) = agents.get(&host_id) else {
            return Err(SendError::NotConnected);
        };

        agent.tx.try_send(msg).map_err(|err| match err {
            mpsc::error::TrySendError::Full(_) => SendError::QueueFull,
            mpsc::error::TrySendError::Closed(_) => SendError::NotConnected,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ControllerMsg {
        ControllerMsg::Config(starfish_msg::HostConfig {
            hostname: "web-1".to_string(),
            generation: 1,
            groups: vec![],
            users: vec![],
        })
    }

    #[tokio::test]
    async fn delivers_to_a_registered_agent() {
        let registry = AgentRegistry::new();
        let (_session, mut rx) = registry.register(1, "web-1");

        registry.send(1, config()).unwrap();

        assert!(matches!(
            rx.recv().await,
            Some(ControllerMsg::Config(config)) if config.hostname == "web-1"
        ));
    }

    #[test]
    fn reports_an_unknown_host_as_not_connected() {
        let registry = AgentRegistry::new();

        assert_eq!(registry.send(1, config()), Err(SendError::NotConnected));
        assert!(!registry.is_connected(1));
    }

    #[test]
    fn reports_a_backed_up_agent_as_queue_full() {
        let registry = AgentRegistry::new();
        let (_session, _rx) = registry.register(1, "web-1");

        for _ in 0..SEND_QUEUE_LEN {
            registry.send(1, config()).unwrap();
        }

        assert_eq!(registry.send(1, config()), Err(SendError::QueueFull));
    }

    #[test]
    fn a_reconnect_replaces_the_older_session() {
        let registry = AgentRegistry::new();
        let (first, _first_rx) = registry.register(1, "web-1");
        let (second, _second_rx) = registry.register(1, "web-1");

        assert_ne!(first, second);

        // The session that was replaced must not take the new one's slot with
        // it when it exits.
        registry.unregister(1, first);

        assert!(registry.is_connected(1));

        registry.unregister(1, second);

        assert!(!registry.is_connected(1));
    }

    #[test]
    fn dropping_the_receiver_makes_the_host_unreachable() {
        let registry = AgentRegistry::new();
        let (_session, rx) = registry.register(1, "web-1");

        drop(rx);

        assert_eq!(registry.send(1, config()), Err(SendError::NotConnected));
    }
}
