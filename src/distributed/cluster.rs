use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use uuid::Uuid;
use super::node::{ClusterNode, NodeRole, NodeStatus};

pub struct ClusterManager {
    nodes: RwLock<HashMap<String, ClusterNode>>,
    leader_id: RwLock<Option<String>>,
    cluster_id: String,
    self_id: String,
}

impl ClusterManager {
    pub fn new(self_node: &ClusterNode) -> Self {
        let mut nodes = HashMap::new();
        nodes.insert(self_node.id.clone(), self_node.clone());
        
        ClusterManager {
            nodes: RwLock::new(nodes),
            leader_id: RwLock::new(None),
            cluster_id: Uuid::new_v4().to_string(),
            self_id: self_node.id.clone(),
        }
    }

    pub fn register_node(&self, node: ClusterNode) -> Result<(), String> {
        let mut nodes = self.nodes.write().map_err(|_| "Failed to acquire write lock")?;
        
        if nodes.contains_key(&node.id) {
            return Err(format!("Node {} already registered", node.id));
        }
        nodes.insert(node.id.clone(), node);
        Ok(())
    }

    pub fn unregister_node(&self, node_id: &str) -> Result<(), String> {
        let mut nodes = self.nodes.write().map_err(|_| "Failed to acquire write lock")?;
        let mut leader_id = self.leader_id.write().map_err(|_| "Failed to acquire write lock")?;
        
        if !nodes.contains_key(node_id) {
            return Err(format!("Node {} not found", node_id));
        }

        if Some(node_id.to_string()) == *leader_id {
            *leader_id = None;
        }

        nodes.remove(node_id);
        Ok(())
    }

    pub fn get_node(&self, id: &str) -> Option<ClusterNode> {
        self.nodes.read().ok().and_then(|nodes| nodes.get(id).cloned())
    }

    pub fn get_leader(&self) -> Option<ClusterNode> {
        self.leader_id.read().ok().and_then(|leader_id| {
            leader_id.as_ref().and_then(|id| self.get_node(id))
        })
    }

    pub fn get_self(&self) -> Option<ClusterNode> {
        self.get_node(&self.self_id)
    }

    pub fn get_self_id(&self) -> String {
        self.self_id.clone()
    }

    pub fn set_leader(&self, node_id: &str) -> Result<(), String> {
        let mut nodes = self.nodes.write().map_err(|_| "Failed to acquire write lock")?;
        let mut leader_id = self.leader_id.write().map_err(|_| "Failed to acquire write lock")?;
        
        if !nodes.contains_key(node_id) {
            return Err(format!("Node {} not found", node_id));
        }

        if let Some(old_leader_id) = &*leader_id {
            if let Some(leader) = nodes.get_mut(old_leader_id) {
                leader.role = NodeRole::Follower;
            }
        }

        *leader_id = Some(node_id.to_string());
        
        if let Some(new_leader) = nodes.get_mut(node_id) {
            new_leader.role = NodeRole::Leader;
        }

        Ok(())
    }

    pub fn healthy_nodes(&self) -> Vec<ClusterNode> {
        self.nodes.read().ok().map_or(Vec::new(), |nodes| {
            nodes.values().filter(|n| n.is_healthy()).cloned().collect()
        })
    }

    pub fn unhealthy_nodes(&self) -> Vec<ClusterNode> {
        self.nodes.read().ok().map_or(Vec::new(), |nodes| {
            nodes.values().filter(|n| !n.is_healthy()).cloned().collect()
        })
    }

    pub fn node_count(&self) -> usize {
        self.nodes.read().map_or(0, |nodes| nodes.len())
    }

    pub fn healthy_count(&self) -> usize {
        self.healthy_nodes().len()
    }

    pub fn is_leader(&self) -> bool {
        self.leader_id.read().map_or(false, |leader_id| {
            leader_id.as_deref() == Some(&self.self_id)
        })
    }

    pub fn has_quorum(&self) -> bool {
        self.healthy_count() > (self.node_count() / 2)
    }

    pub fn broadcast<F>(&self, mut handler: F)
    where
        F: FnMut(&ClusterNode),
    {
        if let Ok(nodes) = self.nodes.read() {
            let self_id = self.self_id.clone();
            for node in nodes.values().filter(|n| n.is_healthy()) {
                if node.id != self_id {
                    handler(node);
                }
            }
        }
    }

    pub fn get_cluster_info(&self) -> ClusterInfo {
        let leader = self.leader_id.read().ok().and_then(|l| l.clone());
        
        ClusterInfo {
            cluster_id: self.cluster_id.clone(),
            total_nodes: self.node_count(),
            healthy_nodes: self.healthy_count(),
            leader,
            self_id: self.self_id.clone(),
        }
    }

    pub fn update_node_heartbeat(&self, node_id: &str) -> Result<(), String> {
        let mut nodes = self.nodes.write().map_err(|_| "Failed to acquire write lock")?;
        
        if let Some(node) = nodes.get_mut(node_id) {
            node.heartbeat();
            Ok(())
        } else {
            Err(format!("Node {} not found", node_id))
        }
    }

    pub fn update_node_status(&self, node_id: &str, status: NodeStatus) -> Result<(), String> {
        let mut nodes = self.nodes.write().map_err(|_| "Failed to acquire write lock")?;
        
        if let Some(node) = nodes.get_mut(node_id) {
            node.status = status;
            Ok(())
        } else {
            Err(format!("Node {} not found", node_id))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterInfo {
    pub cluster_id: String,
    pub total_nodes: usize,
    pub healthy_nodes: usize,
    pub leader: Option<String>,
    pub self_id: String,
}

pub type SharedClusterManager = Arc<ClusterManager>;

pub fn create_shared_cluster_manager(self_node: &ClusterNode) -> SharedClusterManager {
    Arc::new(ClusterManager::new(self_node))
}