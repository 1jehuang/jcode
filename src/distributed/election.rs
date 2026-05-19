use std::time::{Duration, Instant};
use super::node::NodeRole;
use super::cluster::ClusterManager;

pub struct ElectionService {
    election_timeout: Duration,
    #[allow(dead_code)]
    heartbeat_interval: Duration,
    current_term: u64,
    voted_for: Option<String>,
    votes_received: Vec<String>,
    last_heartbeat: Instant,
}

impl ElectionService {
    pub fn new() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let jitter = SystemTime::now().duration_since(UNIX_EPOCH)
            .unwrap_or(std::time::Duration::from_secs(0))
            .as_millis() as u64 % 150;
        ElectionService {
            election_timeout: Duration::from_millis(150 + jitter),
            heartbeat_interval: Duration::from_millis(50),
            current_term: 0,
            voted_for: None,
            votes_received: vec![],
            last_heartbeat: Instant::now(),
        }
    }

    pub fn start_election(&mut self, node_id: &str, cluster: &ClusterManager) -> Result<NodeRole, String> {
        self.current_term += 1;
        self.voted_for = Some(node_id.to_string());
        self.votes_received = vec![node_id.to_string()];

        let healthy_nodes = cluster.healthy_nodes();
        let quorum = (healthy_nodes.len() / 2) + 1;

        if self.votes_received.len() >= quorum {
            Ok(NodeRole::Leader)
        } else {
            Ok(NodeRole::Candidate)
        }
    }

    pub fn receive_vote(&mut self, voter_id: &str) { self.votes_received.push(voter_id.to_string()); }

    pub fn check_quorum(&self, total_nodes: usize) -> bool {
        let required = (total_nodes / 2) + 1;
        self.votes_received.len() >= required
    }

    pub fn should_start_election(&self) -> bool {
        self.last_heartbeat.elapsed() > self.election_timeout
    }

    pub fn reset_heartbeat(&mut self) { self.last_heartbeat = Instant::now(); }
    pub fn get_term(&self) -> u64 { self.current_term }
}
