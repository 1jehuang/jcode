//! Global Server Load Balancing (GSLB) for cross-region deployment
//!
//! Provides intelligent traffic distribution across multiple geographic regions:
//! - DNS-based global load balancing
//! - Latency-aware routing
//! - Health checking across regions
//! - Failover and disaster recovery
//! - GeoIP-based affinity

use std::collections::HashMap;
use serde::{Serialize, Deserialize};

/// Geographic region identifier
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionId(pub String);

/// Regional cluster information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionalCluster {
    /// Unique cluster ID
    pub cluster_id: String,
    /// Region name (e.g., "us-east-1", "ap-southeast-1")
    pub region: RegionId,
    /// Public endpoint (DNS or IP)
    pub endpoint: String,
    /// Cluster weight (higher = more traffic)
    pub weight: u32,
    /// Current health status
    pub health_status: HealthStatus,
    /// Average latency from this region (ms)
    pub avg_latency_ms: f64,
    /// Current load (0-100%)
    pub load_percent: f64,
    /// Active connections
    pub active_connections: u64,
    /// Maximum capacity (connections)
    pub max_capacity: u64,
}

/// Health status of a regional cluster
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Maintenance,
}

/// GSLB routing strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GslbStrategy {
    /// Route to nearest region based on latency
    LatencyBased,
    /// Route based on geographic location
    GeoBased,
    /// Weighted round-robin across regions
    WeightedRoundRobin,
    /// Route to least loaded region
    LeastLoaded,
    /// Failover to backup regions only when primary is down
    Failover,
}

/// Client location information for geo-routing
#[derive(Debug, Clone)]
pub struct ClientLocation {
    /// Latitude
    pub lat: f64,
    /// Longitude
    pub lon: f64,
    /// Country code (ISO 3166-1 alpha-2)
    pub country: String,
    /// City name
    pub city: Option<String>,
}

/// GSLB router for cross-region traffic management
pub struct GslbRouter {
    /// All registered regional clusters
    clusters: HashMap<String, RegionalCluster>,
    /// Routing strategy
    strategy: GslbStrategy,
    /// Region distance matrix (for geo-routing)
    distance_matrix: HashMap<(String, String), f64>,
    /// Latency cache (client_region -> target_region -> latency_ms)
    latency_cache: HashMap<(String, String), f64>,
}

impl GslbRouter {
    pub fn new(strategy: GslbStrategy) -> Self {
        Self {
            clusters: HashMap::new(),
            strategy,
            distance_matrix: HashMap::new(),
            latency_cache: HashMap::new(),
        }
    }

    /// Register a regional cluster
    pub fn register_cluster(&mut self, cluster: RegionalCluster) {
        self.clusters.insert(cluster.cluster_id.clone(), cluster);
    }

    /// Deregister a regional cluster
    pub fn deregister_cluster(&mut self, cluster_id: &str) {
        self.clusters.remove(cluster_id);
    }

    /// Select the best region for a client request
    pub fn select_region(
        &self,
        client_location: Option<&ClientLocation>,
        client_region: Option<&str>,
    ) -> Option<RegionalCluster> {
        // Filter to healthy clusters only
        let healthy_refs: Vec<&RegionalCluster> = self.clusters.values()
            .filter(|c| c.health_status == HealthStatus::Healthy || c.health_status == HealthStatus::Degraded)
            .collect();

        if healthy_refs.is_empty() {
            return None;
        }

        match self.strategy {
            GslbStrategy::LatencyBased => {
                self.select_by_latency(&healthy_refs, client_region).cloned()
            }
            GslbStrategy::GeoBased => {
                self.select_by_geo(&healthy_refs, client_location).cloned()
            }
            GslbStrategy::WeightedRoundRobin => {
                self.select_weighted(&healthy_refs).cloned()
            }
            GslbStrategy::LeastLoaded => {
                self.select_least_loaded(&healthy_refs).cloned()
            }
            GslbStrategy::Failover => {
                self.select_failover(&healthy_refs).cloned()
            }
        }
    }

    /// Select region based on lowest latency
    fn select_by_latency<'a>(
        &self,
        clusters: &'a [&'a RegionalCluster],
        client_region: Option<&str>,
    ) -> Option<&'a RegionalCluster> {
        if let Some(cr) = client_region {
            clusters.iter()
                .min_by(|a, b| {
                    let lat_a = self.get_latency(cr, &a.region.0);
                    let lat_b = self.get_latency(cr, &b.region.0);
                    lat_a.partial_cmp(&lat_b).unwrap_or(std::cmp::Ordering::Equal)
                })
                .copied()
        } else {
            // No client region info, use cluster's own avg latency
            clusters.iter()
                .min_by(|a, b| {
                    a.avg_latency_ms.partial_cmp(&b.avg_latency_ms)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .copied()
        }
    }

    /// Select region based on geographic proximity
    fn select_by_geo<'a>(
        &self,
        clusters: &'a [&'a RegionalCluster],
        client_location: Option<&ClientLocation>,
    ) -> Option<&'a RegionalCluster> {
        if let Some(loc) = client_location {
            clusters.iter()
                .min_by(|a, b| {
                    let dist_a = self.geo_distance(loc.lat, loc.lon, &a.region.0);
                    let dist_b = self.geo_distance(loc.lat, loc.lon, &b.region.0);
                    dist_a.partial_cmp(&dist_b).unwrap_or(std::cmp::Ordering::Equal)
                })
                .copied()
        } else {
            // Fallback to weighted selection
            self.select_weighted(clusters)
        }
    }

    /// Weighted round-robin selection
    fn select_weighted<'a>(&self, clusters: &'a [&'a RegionalCluster]) -> Option<&'a RegionalCluster> {
        let total_weight: u32 = clusters.iter().map(|c| c.weight).sum();
        if total_weight == 0 {
            return clusters.first().copied();
        }

        // Simple weighted random selection
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let mut random = rng.gen_range(0..total_weight);

        for cluster in clusters {
            if random < cluster.weight {
                return Some(*cluster);
            }
            random -= cluster.weight;
        }

        clusters.last().copied()
    }

    /// Select least loaded region
    fn select_least_loaded<'a>(&self, clusters: &'a [&'a RegionalCluster]) -> Option<&'a RegionalCluster> {
        clusters.iter()
            .min_by(|a, b| {
                a.load_percent.partial_cmp(&b.load_percent)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .copied()
    }

    /// Failover selection (primary first, then backups)
    fn select_failover<'a>(&self, clusters: &'a [&'a RegionalCluster]) -> Option<&'a RegionalCluster> {
        // Prefer healthy clusters, then degraded
        clusters.iter()
            .find(|c| c.health_status == HealthStatus::Healthy)
            .or_else(|| clusters.iter().find(|c| c.health_status == HealthStatus::Degraded))
            .copied()
    }

    /// Get cached or estimated latency between regions
    fn get_latency(&self, from_region: &str, to_region: &str) -> f64 {
        if from_region == to_region {
            return 1.0; // Same region, minimal latency
        }

        self.latency_cache.get(&(from_region.to_string(), to_region.to_string()))
            .copied()
            .unwrap_or(100.0) // Default 100ms inter-region latency
    }

    /// Calculate geographic distance (Haversine formula)
    fn geo_distance(&self, lat1: f64, lon1: f64, region: &str) -> f64 {
        // Simplified: use pre-defined coordinates for common regions
        let (lat2, lon2) = match region {
            "us-east-1" => (37.0902, -95.7129),
            "us-west-2" => (45.5231, -122.6765),
            "eu-west-1" => (53.3498, -6.2603),
            "ap-southeast-1" => (1.3521, 103.8198),
            "ap-northeast-1" => (35.6762, 139.6503),
            _ => (0.0, 0.0),
        };

        // Haversine distance in km
        let r = 6371.0; // Earth radius in km
        let dlat = (lat2 - lat1).to_radians();
        let dlon = (lon2 - lon1).to_radians();

        let a = (dlat / 2.0).sin().powi(2)
            + lat1.to_radians().cos() * lat2.to_radians().cos()
            * (dlon / 2.0).sin().powi(2);

        let c = 2.0 * a.sqrt().asin();
        r * c
    }

    /// Update cluster health status
    pub fn update_health(&mut self, cluster_id: &str, status: HealthStatus) {
        if let Some(cluster) = self.clusters.get_mut(cluster_id) {
            cluster.health_status = status;
        }
    }

    /// Update cluster metrics
    pub fn update_metrics(&mut self, cluster_id: &str, load_percent: f64, active_connections: u64, avg_latency_ms: f64) {
        if let Some(cluster) = self.clusters.get_mut(cluster_id) {
            cluster.load_percent = load_percent;
            cluster.active_connections = active_connections;
            cluster.avg_latency_ms = avg_latency_ms;
        }
    }

    /// Get all registered clusters
    pub fn get_clusters(&self) -> &HashMap<String, RegionalCluster> {
        &self.clusters
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gslb_basic() {
        let mut router = GslbRouter::new(GslbStrategy::WeightedRoundRobin);

        router.register_cluster(RegionalCluster {
            cluster_id: "cluster-1".to_string(),
            region: RegionId("us-east-1".to_string()),
            endpoint: "us-east.example.com".to_string(),
            weight: 100,
            health_status: HealthStatus::Healthy,
            avg_latency_ms: 10.0,
            load_percent: 50.0,
            active_connections: 1000,
            max_capacity: 10000,
        });

        assert_eq!(router.get_clusters().len(), 1);
    }
}
