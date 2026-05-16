//! 用量统计和配额管理模块

use crate::auth::{Organization, OrgPlan};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 用量记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    pub id: String,
    pub org_id: String,
    pub user_id: Option<String>,
    pub model_name: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub latency_ms: u64,
    pub request_type: String,
    pub created_at: DateTime<Utc>,
}

/// 组织用量统计
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrgDailyUsage {
    pub org_id: String,
    pub date: String,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_tokens: u64,
    pub request_count: u64,
    pub avg_latency_ms: f64,
    pub models_used: HashMap<String, UsageByModel>,
}

/// 按模型统计用量
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageByModel {
    pub total_tokens: u64,
    pub request_count: u64,
}

/// 用量管理器
pub struct UsageManager {
    records: Arc<RwLock<Vec<UsageRecord>>>,
    daily_cache: Arc<RwLock<HashMap<String, OrgDailyUsage>>>,
}

impl UsageManager {
    pub fn new() -> Self {
        Self {
            records: Arc::new(RwLock::new(Vec::new())),
            daily_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 记录一次用量
    pub async fn record_usage(&self, record: UsageRecord) {
        let mut records = self.records.write().await;
        records.push(record.clone());

        // 更新日缓存
        let date = record.created_at.format("%Y-%m-%d").to_string();
        let mut cache = self.daily_cache.write().await;
        let daily = cache.entry(record.org_id.clone()).or_insert_with(|| OrgDailyUsage {
            org_id: record.org_id.clone(),
            date: date,
            ..Default::default()
        });

        daily.total_prompt_tokens += record.prompt_tokens as u64;
        daily.total_completion_tokens += record.completion_tokens as u64;
        daily.total_tokens += record.total_tokens as u64;
        daily.request_count += 1;
        daily.avg_latency_ms = (daily.avg_latency_ms * (daily.request_count - 1) as f64
            + record.latency_ms as f64) / daily.request_count as f64;

        let model_entry = daily.models_used
            .entry(record.model_name)
            .or_insert(UsageByModel { total_tokens: 0, request_count: 0 });
        model_entry.total_tokens += record.total_tokens as u64;
        model_entry.request_count += 1;
    }

    /// 检查是否超出配额
    pub async fn check_quota(&self, org: &Organization, current_tokens: u64) -> QuotaResult {
        let date = Utc::now().format("%Y-%m-%d").to_string();
        let cache = self.daily_cache.read().await;
        let daily = cache.get(&org.id);

        let today_tokens = daily.map(|d| d.total_tokens).unwrap_or(0);
        let today_requests = daily.map(|d| d.request_count).unwrap_or(0);

        // 检查 Token 配额
        if org.daily_token_limit > 0 && today_tokens + current_tokens > org.daily_token_limit {
            return QuotaResult::Exceeded {
                kind: QuotaType::TokenLimit,
                used: today_tokens + current_tokens,
                limit: org.daily_token_limit,
            };
        }

        // 检查并发配额
        if org.concurrent_limit > 0 && today_requests >= org.concurrent_limit as u64 {
            return QuotaResult::Exceeded {
                kind: QuotaType::ConcurrentLimit,
                used: today_requests,
                limit: org.concurrent_limit as u64,
            };
        }

        QuotaResult::Ok {
            remaining_tokens: if org.daily_token_limit > 0 {
                org.daily_token_limit.saturating_sub(today_tokens)
            } else {
                u64::MAX
            },
        }
    }

    /// 获取组织用量统计
    pub async fn get_org_usage(&self, org_id: &str, days: u32) -> Vec<OrgDailyUsage> {
        let records = self.records.read().await;
        let cutoff = Utc::now() - chrono::Duration::days(days as i64);

        let mut daily_map: HashMap<String, OrgDailyUsage> = HashMap::new();
        for record in records.iter().filter(|r| r.org_id == org_id && r.created_at > cutoff) {
            let date = record.created_at.format("%Y-%m-%d").to_string();
            let entry = daily_map.entry(date.clone()).or_insert_with(|| OrgDailyUsage {
                org_id: org_id.to_string(),
                date,
                ..Default::default()
            });
            entry.total_prompt_tokens += record.prompt_tokens as u64;
            entry.total_completion_tokens += record.completion_tokens as u64;
            entry.total_tokens += record.total_tokens as u64;
            entry.request_count += 1;
        }

        let mut result: Vec<OrgDailyUsage> = daily_map.into_values().collect();
        result.sort_by(|a, b| a.date.cmp(&b.date));
        result
    }
}

/// 配额检查结果
#[derive(Debug, Clone)]
pub enum QuotaResult {
    Ok {
        remaining_tokens: u64,
    },
    Exceeded {
        kind: QuotaType,
        used: u64,
        limit: u64,
    },
}

/// 配额类型
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum QuotaType {
    TokenLimit,
    ConcurrentLimit,
}

impl std::fmt::Display for QuotaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TokenLimit => write!(f, "token_limit"),
            Self::ConcurrentLimit => write!(f, "concurrent_limit"),
        }
    }
}
