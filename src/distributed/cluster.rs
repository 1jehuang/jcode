use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use super::node::{ClusterNode, NodeRole, NodeStatus};

pub struct ClusterManager {
    nodes: HashMap<String, ClusterNode>,
    leader_id: Option<String>,
    cluster_id: String,
    self_id: String,
}

impl ClusterManager {
    pub fn new(self_node: &ClusterNode) -> Self {
        let mut manager = ClusterManager {
            nodes: HashMap::new(),
            leader_id: None,
            cluster_id: Uuid::new_v4().to_string(),
            self_id: self_node.id.clone(),
        };
        manager.register_node(self_node.clone());
        manager
    }

    pub fn register_node(&mut self, node: ClusterNode) -> Result<(), String> {
        if self.nodes.contains_key(&node.id) {
            return Err(format!("Node {} already registered", node.id));
        }
        self.nodes.insert(node.id.clone(), node);
        Ok(())
    }

    pub fn unregister_node(&mut self, node_id: &str) -> Result<(), String> {
        if !self.nodes.contains_key(node_id) {
            return Err(format!("Node {} not found", node_id));
        }

        if Some(node_id.to_string()) == self.leader_id {
            self.leader_id = None;
        }

        self.nodes.remove(node_id);
        Ok(())
    }

    pub fn get_node(&self, id: &str) -> Option<&ClusterNode> { self.nodes.get(id) }
    pub fn get_leader(&self) -> Option<&ClusterNode> { self.leader_id.as_ref().and_then(|id| self.nodes.get(id)) }
    pub fn get_self(&self) -> Option<&ClusterNode> { self.nodes.get(&self.self_id) }

    pub fn set_leader(&mut self, node_id: &str) -> Result<(), String> {
        if !self.nodes.contains_key(node_id) {
            return Err(format!("Node {} not found", node_id));
        }

        let old_leader_id = self.leader_id.clone().unwrap_or_default();
        if let Some(leader) = self.get_mut_node(&old_leader_id) {
            leader.role = NodeRole::Follower;
        }

        self.leader_id = Some(node_id.to_string());
        if let Some(new_leader) = self.get_mut_node(node_id) {
            new_leader.role = NodeRole::Leader;
        }

        Ok(())
    }

    fn get_mut_node(&mut self, id: &str) -> Option<&mut ClusterNode> { self.nodes.get_mut(id) }

    pub fn healthy_nodes(&self) -> Vec<&ClusterNode> {
        self.nodes.values().filter(|n| n.is_healthy()).collect()
    }

    pub fn unhealthy_nodes(&self) -> Vec<&ClusterNode> {
        self.nodes.values().filter(|n| !n.is_healthy()).collect()
    }

    pub fn node_count(&self) -> usize { self.nodes.len() }
    pub fn healthy_count(&self) -> usize { self.healthy_nodes().len() }

    pub fn is_leader(&self) -> bool { self.leader_id.as_deref() == Some(&self.self_id) }
    pub fn has_quorum(&self) -> bool { self.healthy_count() > (self.node_count() / 2) }

    pub fn broadcast<F>(&self, mut handler: F)
    where
        F: FnMut(&ClusterNode),
    {
        for node in self.healthy_nodes() {
            if node.id != self.self_id {
                handler(node);
            }
        }
    }

    pub fn get_cluster_info(&self) -> ClusterInfo {
        ClusterInfo {
            cluster_id: self.cluster_id.clone(),
            total_nodes: self.node_count(),
            healthy_nodes: self.healthy_count(),
            leader: self.leader_id.clone(),
            self_id: self.self_id.clone(),
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
