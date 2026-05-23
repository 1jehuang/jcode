//! W2: Domain Analyzer — 将代码映射到业务域
//! 移植自: Understand-Anything agents/domain-analyzer
//! 基于文件路径语义和命名约定的业务域识别

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::file_analyzer::FileAnalysis;
use super::{KGNode, KnowledgeGraph};

/// 业务域规则
struct DomainRule {
    keywords: Vec<&'static str>,
    domain_name: &'static str,
    description: &'static str,
}

const DOMAIN_RULES: &[DomainRule] = &[
    DomainRule { keywords: vec!["auth", "login", "logout", "session", "token", "password", "oauth",
        "认证", "登录", "授权", "鉴权"],
        domain_name: "Authentication", description: "用户认证与授权" },
    DomainRule { keywords: vec!["user", "profile", "account", "member", "customer",
        "用户", "会员", "账户"],
        domain_name: "User Management", description: "用户与账户管理" },
    DomainRule { keywords: vec!["payment", "billing", "invoice", "transaction", "order", "checkout",
        "支付", "账单", "订单", "结算"],
        domain_name: "Payment & Billing", description: "支付与计费" },
    DomainRule { keywords: vec!["notification", "email", "sms", "push", "alert", "message",
        "通知", "邮件", "短信", "推送"],
        domain_name: "Notification", description: "通知与消息" },
    DomainRule { keywords: vec!["search", "index", "query", "检索", "搜索", "索引"],
        domain_name: "Search", description: "搜索与索引" },
    DomainRule { keywords: vec!["report", "analytics", "dashboard", "statistic", "metric",
        "报表", "统计", "分析", "看板"],
        domain_name: "Analytics & Reports", description: "分析与报表" },
    DomainRule { keywords: vec!["admin", "management", "dashboard", "后台", "管理"],
        domain_name: "Admin", description: "后台管理" },
    DomainRule { keywords: vec!["file", "upload", "download", "storage", "image", "文档", "文件", "上传", "下载"],
        domain_name: "File Management", description: "文件与存储管理" },
    DomainRule { keywords: vec!["log", "logging", "audit", "monitor", "日志", "审计", "监控"],
        domain_name: "Logging & Audit", description: "日志与审计" },
    DomainRule { keywords: vec!["cache", "redis", "缓存"],
        domain_name: "Cache", description: "缓存管理" },
    DomainRule { keywords: vec!["api", "graphql", "rest", "grpc", "rpc", "endpoint", "接口"],
        domain_name: "API Gateway", description: "API网关与接口" },
    DomainRule { keywords: vec!["workflow", "pipeline", "job", "task", "schedule", "cron",
        "工作流", "流水线", "定时", "调度"],
        domain_name: "Workflow", description: "工作流与任务调度" },
    DomainRule { keywords: vec!["content", "article", "post", "blog", "page",
        "内容", "文章", "博客"],
        domain_name: "Content Management", description: "内容管理" },
    DomainRule { keywords: vec!["product", "inventory", "catalog", "product", "sku",
        "商品", "库存", "产品", "目录"],
        domain_name: "Product & Inventory", description: "商品与库存管理" },
];

/// 检测文件的业务域
pub fn detect_domain(file_path: &str) -> Option<&'static str> {
    let path = file_path.to_lowercase();
    let path_parts: Vec<&str> = path.split(&['/', '\\'][..]).collect();
    let filename = path_parts.last().unwrap_or(&"");

    for rule in DOMAIN_RULES {
        for keyword in &rule.keywords {
            if filename.contains(keyword) || path_parts.iter().any(|p| p.contains(keyword)) {
                return Some(rule.domain_name);
            }
        }
    }

    None
}

/// Agent 4: 分析业务域
pub async fn analyze_domains(
    _analysis: &[FileAnalysis],
    graph: &Arc<RwLock<KnowledgeGraph>>,
) -> Result<(), String> {
    let mut g = graph.write().await;

    for node in &mut g.nodes {
        let domain = detect_domain(&node.file_path)
            .map(|d| d.to_string());
        node.domain = domain;
    }

    Ok(())
}

pub fn domain_help() -> String {
    let mut help = String::from("Supported Business Domains:\n");
    for rule in DOMAIN_RULES {
        help.push_str(&format!("  - {}: {}\n", rule.domain_name, rule.description));
    }
    help
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_domain() {
        assert_eq!(detect_domain("src/auth/login.rs"), Some("Authentication"));
        assert_eq!(detect_domain("src/models/user.rs"), Some("User Management"));
        assert_eq!(detect_domain("src/payment/checkout.ts"), Some("Payment & Billing"));
        assert_eq!(detect_domain("src/notification/email.rs"), Some("Notification"));
        assert_eq!(detect_domain("src/search/index.rs"), Some("Search"));
        assert_eq!(detect_domain("src/admin/dashboard.rs"), Some("Admin"));
        // 不匹配的应该返回 None
        assert_eq!(detect_domain("src/lib.rs"), None);
    }
}
