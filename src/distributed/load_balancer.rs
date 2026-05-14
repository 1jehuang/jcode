use super::node::ClusterNode;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone)]
pub enum LoadBalancingStrategy {
    RoundRobin,
    LeastConnections,
    Random,
    WeightedRoundRobin,
    IPHash,
}

pub struct LoadBalancer {
    strategy: LoadBalancingStrategy,
    counter: AtomicUsize,
}

impl LoadBalancer {
    pub fn new(strategy: LoadBalancingStrategy) -> Self {
        LoadBalancer {
            strategy,
            counter: AtomicUsize::new(0),
        }
    }

    pub fn select_node(&self, nodes: &[&ClusterNode]) -> Option<&ClusterNode> {
        if nodes.is_empty() { return None; }

        match &self.strategy {
            LoadBalancingStrategy::RoundRobin => self.round_robin(nodes),
            LoadBalancingStrategy::LeastConnections => self.least_connections(nodes),
            LoadBalancingStrategy::Random => self.random_select(nodes),
            _ => self.round_robin(nodes),
        }
    }

    fn round_robin<'a>(&self, nodes: &[&'a ClusterNode]) -> Option<&'a ClusterNode> {
        let index = self.counter.fetch_add(1, Ordering::Relaxed) % nodes.len();
        nodes.get(index).copied()
    }

    fn least_connections<'a>(&self, nodes: &[&'a ClusterNode]) -> Option<&'a ClusterNode> {
        nodes.iter()
            .min_by(|a, b| {
                a.metadata.load_average.partial_cmp(&b.metadata.load_average)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .copied()
    }

    fn random_select<'a>(&self, nodes: &[&'a ClusterNode]) -> Option<&'a ClusterNode> {
        if nodes.is_empty() { return None; }
        use std::time::{SystemTime, UNIX_EPOCH};
        let seed = SystemTime::now().duration_since(UNIX_EPOCH)
            .unwrap_or(std::time::Duration::from_secs(0))
            .as_nanos() as usize;
        let index = seed % nodes.len();
        nodes.get(index).copied()
    }

    pub fn set_strategy(&mut self, strategy: LoadBalancingStrategy) {
        self.strategy = strategy;
        self.counter.store(0, Ordering::Relaxed);
    }
}
