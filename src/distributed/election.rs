use std::time::{Duration, Instant};
use super::node::NodeRole;
use super::cluster::ClusterManager;

pub struct ElectionService {
    election_timeout: Duration,
    heartbeat_interval: Duration,
    current_term: u64,
    voted_for: Option<String>,
    votes_received: Vec<String>,
    last_heartbeat: Instant,
    vote_request_timeout: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VoteRequest {
    pub term: u64,
    pub candidate_id: String,
    pub last_log_index: u64,
    pub last_log_term: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VoteResponse {
    pub term: u64,
    pub granted: bool,
}

#[derive(Debug)]
pub enum ElectionError {
    Timeout,
    ConnectionFailed(String),
    VoteDenied(String),
    NotLeader,
}

impl std::fmt::Display for ElectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ElectionError::Timeout => write!(f, "Election timeout"),
            ElectionError::ConnectionFailed(node) => write!(f, "Failed to connect to node: {}", node),
            ElectionError::VoteDenied(node) => write!(f, "Vote denied by node: {}", node),
            ElectionError::NotLeader => write!(f, "Not the leader"),
        }
    }
}

impl std::error::Error for ElectionError {}

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
            vote_request_timeout: Duration::from_millis(500),
        }
    }

    pub async fn start_election(
        &mut self,
        node_id: &str,
        cluster: &ClusterManager,
        last_log_index: u64,
        last_log_term: u64,
    ) -> Result<NodeRole, ElectionError> {
        self.current_term += 1;
        self.voted_for = Some(node_id.to_string());
        self.votes_received = vec![node_id.to_string()];

        let healthy_nodes = cluster.healthy_nodes();
        let term = self.current_term;
        let candidate_id = node_id.to_string();
        
        let mut vote_futures = Vec::new();

        for node in &healthy_nodes {
            if node.id != node_id {
                let request = VoteRequest {
                    term,
                    candidate_id: candidate_id.clone(),
                    last_log_index,
                    last_log_term,
                };
                vote_futures.push(self.request_vote(node.id.as_str(), request));
            }
        }

        let results = futures::future::join_all(vote_futures).await;

        for result in results {
            if let Ok((node_id, response)) = result {
                if response.granted {
                    self.votes_received.push(node_id);
                } else if response.term > self.current_term {
                    self.current_term = response.term;
                    self.voted_for = None;
                    return Ok(NodeRole::Follower);
                }
            }
        }

        let quorum = (healthy_nodes.len() / 2) + 1;

        if self.votes_received.len() >= quorum {
            Ok(NodeRole::Leader)
        } else {
            Ok(NodeRole::Candidate)
        }
    }

    async fn request_vote(&self, node_id: &str, request: VoteRequest) -> Result<(String, VoteResponse), ElectionError> {
        let response = simulate_rpc_call(node_id, &request, self.vote_request_timeout).await?;
        Ok((node_id.to_string(), response))
    }

    pub fn handle_vote_request(&mut self, request: &VoteRequest) -> VoteResponse {
        if request.term < self.current_term {
            return VoteResponse {
                term: self.current_term,
                granted: false,
            };
        }

        if request.term > self.current_term {
            self.current_term = request.term;
            self.voted_for = None;
        }

        let can_vote = self.voted_for.is_none() || self.voted_for.as_ref() == Some(&request.candidate_id);

        if can_vote {
            self.voted_for = Some(request.candidate_id.clone());
            VoteResponse {
                term: self.current_term,
                granted: true,
            }
        } else {
            VoteResponse {
                term: self.current_term,
                granted: false,
            }
        }
    }

    pub fn receive_vote(&mut self, voter_id: &str) {
        if !self.votes_received.contains(&voter_id.to_string()) {
            self.votes_received.push(voter_id.to_string());
        }
    }

    pub fn check_quorum(&self, total_nodes: usize) -> bool {
        let required = (total_nodes / 2) + 1;
        self.votes_received.len() >= required
    }

    pub fn should_start_election(&self) -> bool {
        self.last_heartbeat.elapsed() > self.election_timeout
    }

    pub fn reset_heartbeat(&mut self) {
        self.last_heartbeat = Instant::now();
    }

    pub fn get_term(&self) -> u64 {
        self.current_term
    }

    pub fn set_term(&mut self, term: u64) {
        self.current_term = term;
    }

    pub fn reset_votes(&mut self) {
        self.votes_received.clear();
    }

    pub fn votes_count(&self) -> usize {
        self.votes_received.len()
    }
}

async fn simulate_rpc_call(
    node_id: &str,
    request: &VoteRequest,
    _timeout: Duration,
) -> Result<VoteResponse, ElectionError> {
    let _ = tokio::time::sleep(Duration::from_millis(rand::random::<u64>() % 100)).await;
    
    let success = rand::random::<f64>() > 0.1;
    
    if success {
        Ok(VoteResponse {
            term: request.term,
            granted: rand::random::<f64>() > 0.3,
        })
    } else {
        Err(ElectionError::ConnectionFailed(node_id.to_string()))
    }
}