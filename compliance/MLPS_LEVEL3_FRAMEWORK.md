# 网络安全等级保护三级(等保三级)合规框架

**版本**: 1.0
**日期**: 2026-05-22
**状态**: 实施指南
**目标认证**: 信息安全等级保护第三级

---

## 执行摘要

本文档提供CarpAI企业服务器获得等保三级认证的完整框架。等保三级是中国国家标准GB/T 22239-2019《信息安全技术 网络安全等级保护基本要求》中的高级别安全要求，适用于处理重要数据的企业系统。

**时间周期**: 4-6个月
**预估成本**: ¥200,000-¥500,000 (测评费+整改费)
**推荐测评机构**: 中国网络安全审查技术与认证中心(CCNNC)

---

## 目录

1. [等保三级概述](#等保三级概述)
2. [安全技术要求](#安全技术要求)
3. [安全管理要求](#安全管理要求)
4. [实施清单](#实施清单)
5. [制度模板](#制度模板)
6. [证据收集指南](#证据收集指南)

---

## 等保三级概述

### 什么是等保三级？

网络安全等级保护制度是中国强制性网络安全标准，分为5个级别：

| 级别 | 适用场景 | 监管要求 |
|------|---------|---------|
| 第一级 | 一般系统 | 自主保护 |
| 第二级 | 重要系统 | 指导保护 |
| **第三级** | **重要信息系统** | **监督保护** |
| 第四级 | 特别重要系统 | 强制保护 |
| 第五级 | 极端重要系统 | 专控保护 |

**CarpAI定级理由**:
- 处理企业源代码(知识产权)
- 存储用户认证凭据
- 日均服务200+开发者
- 一旦受损可能影响企业正常运营

### 等保三级 vs SOC2

| 维度 | 等保三级 | SOC2 |
|------|---------|------|
| 适用范围 | 中国大陆 | 国际(尤其美国) |
| 法律依据 | 《网络安全法》 | AICPA准则 |
| 强制程度 | **强制性** | 自愿性(市场要求) |
| 测评周期 | 每年复测 | Type I一次性,Type II持续 |
| 侧重点 | 技术+管理并重 | 控制有效性 |
| 成本 | ¥20-50万 | $5-10万 |

**建议**: 同时通过等保三级和SOC2，覆盖国内外市场

---

## 安全技术要求

### 1. 安全物理环境

#### 1.1 机房选址与建设

**要求**: 机房应避开自然灾害高发区

**实施方案**:
```markdown
✅ 已实现(使用云服务):
- 阿里云/腾讯云多可用区部署
- 符合GB 50174-2017《数据中心设计规范》A级机房
- 两地三中心灾备架构

📋 需提供证据:
- 云服务商等保证书
- 机房物理访问记录
- 环境监测报告(温湿度、消防)
```

#### 1.2 物理访问控制

**实施方案**:
```yaml
# kubernetes/physical-access-control.yaml
# 云平台提供的物理安全措施
cloud_provider_security:
  aliyun:
    - 7×24小时视频监控
    - 生物识别门禁
    - 访客登记制度
    - 保安巡逻记录
  tencent_cloud:
    - 同等安全措施
```

---

### 2. 安全通信网络

#### 2.1 网络架构

**要求**: 划分不同安全域，实施访问控制

**实施方案**:
```yaml
# kubernetes/network-segmentation.yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: security-domain-isolation
spec:
  # 安全域划分:
  # 1. DMZ区(对外服务区)
  # 2. 应用区(业务逻辑)
  # 3. 数据区(数据库)
  # 4. 管理区(运维管理)

  podSelector:
    matchLabels:
      security-domain: application-zone

  ingress:
    - from:
        - podSelector:
            matchLabels:
              security-domain: dmz-zone
      ports:
        - protocol: TCP
          port: 8081

  egress:
    - to:
        - podSelector:
            matchLabels:
              security-domain: data-zone
      ports:
        - protocol: TCP
          port: 5432  # PostgreSQL
```

**行动项**: 绘制网络拓扑图
```
┌─────────────────────────────────────────┐
│          互联网                          │
└──────────────┬──────────────────────────┘
               │
        ┌──────▼──────┐
        │   WAF防火墙  │  ← DDoS防护、SQL注入拦截
        └──────┬──────┘
               │
        ┌──────▼──────┐
        │   DMZ区     │  ← Nginx反向代理
        │  (公网IP)    │
        └──────┬──────┘
               │
        ┌──────▼──────┐
        │   应用区     │  ← jcode-server pods
        │ (内网隔离)   │
        └──┬───────┬──┘
           │       │
    ┌──────▼┐  ┌──▼──────┐
    │数据区  │  │管理区    │
    │PG/Redis│  │Prometheus│
    └───────┘  └─────────┘
```

#### 2.2 通信加密

**要求**: 采用密码技术保证通信完整性

**实施方案**:
```rust
// src/encryption/tls_config.rs
use rustls::ServerConfig;

pub fn create_soc2_compliant_tls_config() -> ServerConfig {
    let mut config = ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(cert_chain, private_key)
        .unwrap();

    // 强制TLS 1.3
    config.max_protocol_version = Some(rustls::ProtocolVersion::TLSv1_3);

    // 禁用弱密码套件
    config.cipher_suites = vec![
        rustls::cipher_suite::TLS13_AES_256_GCM_SHA384,
        rustls::cipher_suite::TLS13_CHACHA20_POLY1305_SHA256,
    ];

    config
}
```

**配置要求**:
```yaml
# config/tls_policy.yaml
tls:
  minimum_version: "1.3"
  certificate:
    type: SM2  # 国密算法(等保要求)
    provider: CFCA  # 中国金融认证中心
    validity_days: 365
    auto_renewal: true

  cipher_suites:
    - TLS_SM4_GCM_SM3  # 国密套件
    - TLS_AES_256_GCM_SHA384

  hsts:
    enabled: true
    max_age_seconds: 31536000  # 1年
    include_subdomains: true
```

---

### 3. 安全区域边界

#### 3.1 边界防护

**要求**: 在边界部署访问控制设备

**实施方案**:
```yaml
# kubernetes/waf/modsecurity-deployment.yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: modsecurity-waf
spec:
  replicas: 2
  template:
    spec:
      containers:
        - name: modsecurity
          image: owasp/modsecurity-crs:latest
          env:
            - name: BACKEND
              value: "http://jcode-server:8081"
            - name: RULE_ENGINE
              value: "On"
            - name: ANOMALY_INBOUND
              value: "10"  # 严格模式
          ports:
            - containerPort: 80
            - containerPort: 443
```

**防护规则**:
```apache
# ModSecurity规则集
SecRuleEngine On

# SQL注入防护
SecRule ARGS "@detectSQLi" \
    "id:1001,\
    phase:2,\
    deny,\
    status:403,\
    msg:'SQL Injection Detected'"

# XSS防护
SecRule ARGS "@detectXSS" \
    "id:1002,\
    phase:2,\
    deny,\
    status:403,\
    msg:'XSS Attack Detected'"

# 文件上传限制
SecRule FILES_SIZES "@gt 10485760" \
    "id:1003,\
    phase:2,\
    deny,\
    status:413,\
    msg:'File Too Large (>10MB)'"

# 速率限制
SecRule IP:REQUEST_RATE "@gt 100" \
    "id:1004,\
    phase:1,\
    deny,\
    status:429,\
    msg:'Rate Limit Exceeded'"
```

#### 3.2 入侵防范

**要求**: 检测并防止入侵行为

**实施方案**:
```yaml
# kubernetes/ids/falco-deployment.yaml
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: falco-ids
spec:
  template:
    spec:
      containers:
        - name: falco
          image: falcosecurity/falco:latest
          args:
            - "-K"
            - "/var/run/secrets/kubernetes.io/serviceaccount/token"
          volumeMounts:
            - mountPath: /host/var/run/docker.sock
              name: docker-sock
          securityContext:
            privileged: true
```

**检测规则**:
```yaml
# falco_rules.yaml
- rule: Unauthorized Process Execution
  desc: Detect execution of suspicious processes
  condition: >
    spawned_process and
    not proc.name in (allowed_binaries) and
    container.image != trusted_images
  output: "Suspicious process detected (user=%user.name command=%proc.cmdline)"
  priority: CRITICAL

- rule: Sensitive File Access
  desc: Detect access to sensitive files
  condition: >
    open_read and
    fd.name startswith /etc/shadow or
    fd.name startswith /root/.ssh
  output: "Sensitive file accessed (user=%user.name file=%fd.name)"
  priority: HIGH

- rule: Reverse Shell Detection
  desc: Detect potential reverse shell
  condition: >
    spawned_process and
    (proc.name = "bash" or proc.name = "sh") and
    proc.args contains "/dev/tcp/"
  output: "Possible reverse shell (command=%proc.cmdline)"
  priority: CRITICAL
```

---

### 4. 安全计算环境

#### 4.1 身份鉴别

**要求**: 采用两种或以上组合鉴别技术

**实施方案**:
```rust
// src/auth/mfa_implementation.rs
use crate::auth::{Password, Totp, WebAuthn};

pub struct MultiFactorAuth {
    factors: Vec<AuthFactor>,
}

#[derive(Clone)]
pub enum AuthFactor {
    SomethingYouKnow(Password),     // 密码
    SomethingYouHave(TotpDevice),   // 动态令牌
    SomethingYouAre(Biometric),     // 生物特征(可选)
}

impl MultiFactorAuth {
    pub fn soc2_and_mlps_compliant() -> Self {
        Self {
            factors: vec![
                AuthFactor::SomethingYouKnow(Password::new()),
                AuthFactor::SomethingYouHave(TotpDevice::new()),
                // 等保三级要求至少双因素
            ],
        }
    }

    pub async fn authenticate(&self, credentials: &Credentials) -> Result<Session> {
        // Factor 1: Password
        self.verify_password(&credentials.password).await?;

        // Factor 2: TOTP
        self.verify_totp(&credentials.totp_code).await?;

        // Generate session token
        Ok(Session::new(credentials.user_id))
    }
}
```

**配置要求**:
```yaml
# config/authentication_policy.yaml
authentication:
  password_policy:
    min_length: 12
    require_uppercase: true
    require_lowercase: true
    require_numbers: true
    require_special_chars: true
    max_age_days: 90
    history_count: 12  # 不能与前12次相同

  mfa_policy:
    enforce_for_all_users: true  # 等保三级要求
    allowed_methods:
      - totp  # 基于时间的动态口令
      - sms   # 短信验证码(需配合密码使用)
      - hardware_token  # 硬件令牌(如飞天诚信)
    disallowed_methods:
      - email  # 邮箱验证不安全

  session_policy:
    timeout_minutes: 30  # 无操作自动登出
    max_concurrent_sessions: 3
    force_logout_on_password_change: true
```

#### 4.2 访问控制

**要求**: 授予最小权限，强制访问控制

**实施方案**:
```rust
// src/rbac/mac_implementation.rs
// MAC: Mandatory Access Control (强制访问控制)

use std::collections::HashMap;

#[derive(Clone, PartialEq, PartialOrd)]
pub enum SecurityLevel {
    Unclassified,
    Confidential,
    Secret,
    TopSecret,
}

pub struct MacPolicy {
    user_clearance: HashMap<String, SecurityLevel>,
    resource_classification: HashMap<String, SecurityLevel>,
}

impl MacPolicy {
    pub fn check_access(&self, user_id: &str, resource_id: &str) -> bool {
        let user_level = self.user_clearance.get(user_id);
        let resource_level = self.resource_classification.get(resource_id);

        match (user_level, resource_level) {
            (Some(u), Some(r)) => u >= r,  // 用户密级 >= 资源密级
            _ => false,
        }
    }
}

// 示例: 源代码属于"机密"级
let policy = MacPolicy::new();
policy.set_resource_classification("repo:core", SecurityLevel::Confidential);
policy.set_user_clearance("developer_001", SecurityLevel::Confidential);

assert!(policy.check_access("developer_001", "repo:core"));  // ✅ 允许
assert!(!policy.check_access("intern_001", "repo:core"));    // ❌ 拒绝(intern只有Unclassified)
```

#### 4.3 数据完整性

**要求**: 采用校验技术保证数据完整性

**实施方案**:
```rust
// src/integrity/checksum.rs
use sha2::{Sha256, Digest};
use hmac::{Hmac, Mac};

type HmacSha256 = Hmac<Sha256>;

pub struct IntegrityChecker {
    secret_key: Vec<u8>,
}

impl IntegrityChecker {
    pub fn new(secret_key: &[u8]) -> Self {
        Self {
            secret_key: secret_key.to_vec(),
        }
    }

    /// 生成数据完整性标签
    pub fn generate_tag(&self, data: &[u8]) -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(&self.secret_key).unwrap();
        mac.update(data);
        mac.finalize().into_bytes().to_vec()
    }

    /// 验证数据完整性
    pub fn verify(&self, data: &[u8], tag: &[u8]) -> bool {
        let expected = self.generate_tag(data);
        expected == tag
    }
}

// 数据库记录完整性保护
impl AuditLogger {
    pub fn insert_event(&self, event: &AuditEvent) -> Result<()> {
        let serialized = serde_json::to_vec(event)?;
        let tag = self.integrity_checker.generate_tag(&serialized);

        // 存储数据和标签
        self.db.execute(
            "INSERT INTO audit_events (data, integrity_tag) VALUES ($1, $2)",
            &[&serialized, &tag]
        )?;

        Ok(())
    }

    pub fn verify_event_integrity(&self, event_id: &str) -> Result<bool> {
        let row = self.db.query_one(
            "SELECT data, integrity_tag FROM audit_events WHERE id = $1",
            &[&event_id]
        )?;

        let data: Vec<u8> = row.get(0);
        let stored_tag: Vec<u8> = row.get(1);

        Ok(self.integrity_checker.verify(&data, &stored_tag))
    }
}
```

#### 4.4 数据保密性

**要求**: 采用密码技术保证数据存储保密性

**实施方案**:
```rust
// src/encryption/data_at_rest.rs
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::{Argon2, PasswordHasher};

pub struct DataEncryption {
    cipher: Aes256Gcm,
}

impl DataEncryption {
    pub fn encrypt_sensitive_data(&self, plaintext: &str) -> Result<Vec<u8>> {
        // 使用国密SM4算法(等保要求)或AES-256
        let key = Key::<Aes256Gcm>::from_slice(&self.derive_key());
        let nonce = Nonce::from_slice(&rand::random::<[u8; 12]>());

        let ciphertext = self.cipher.encrypt(nonce, plaintext.as_bytes())?;

        // 存储: nonce + ciphertext
        let mut result = nonce.to_vec();
        result.extend_from_slice(&ciphertext);

        Ok(result)
    }

    /// 密钥派生函数(使用Argon2id)
    fn derive_key(&self) -> Vec<u8> {
        let argon2 = Argon2::default();
        let hash = argon2.hash_password(
            b"master_password",
            &salt
        ).unwrap();

        hash.hash.unwrap().as_bytes().to_vec()
    }
}

// 数据库字段级加密
impl UserRepository {
    pub fn save_api_key(&self, user_id: &str, api_key: &str) -> Result<()> {
        let encrypted = self.encryption.encrypt_sensitive_data(api_key)?;

        self.db.execute(
            "UPDATE users SET api_key_encrypted = $1 WHERE id = $2",
            &[&encrypted, &user_id]
        )?;

        Ok(())
    }

    pub fn get_api_key(&self, user_id: &str) -> Result<String> {
        let encrypted: Vec<u8> = self.db.query_one(
            "SELECT api_key_encrypted FROM users WHERE id = $1",
            &[&user_id]
        )?.get(0);

        self.encryption.decrypt(&encrypted)
    }
}
```

**加密策略**:
```yaml
# config/encryption_policy.yaml
encryption:
  algorithm:
    symmetric: SM4  # 国密对称算法
    asymmetric: SM2  # 国密非对称算法
    hash: SM3  # 国密哈希算法

  key_management:
    master_key_storage: HSM  # 硬件安全模块
    key_rotation_days: 90
    backup_enabled: true
    backup_location: "异地备份中心"

  data_classification:
    personal_info:
      encryption_required: true
      algorithm: SM4-GCM
    source_code:
      encryption_required: true
      algorithm: SM4-CBC
    logs:
      encryption_required: false
      integrity_check: true
```

---

### 5. 安全管理中心

#### 5.1 系统管理

**要求**: 对系统进行集中管理

**实施方案**:
```yaml
# kubernetes/monitoring/prometheus-stack.yaml
apiVersion: monitoring.coreos.com/v1
kind: Prometheus
metadata:
  name: carpai-prometheus
spec:
  replicas: 2
  retention: 30d
  resources:
    requests:
      memory: 4Gi
  storage:
    volumeClaimTemplate:
      spec:
        resources:
          requests:
            storage: 100Gi

---
apiVersion: monitoring.coreos.com/v1
kind: Grafana
metadata:
  name: carpai-grafana
spec:
  dashboardProviders:
    dashboardproviders.yaml:
      apiVersion: 1
      providers:
        - name: 'default'
          orgId: 1
          folder: ''
          type: file
          options:
            path: /var/lib/grafana/dashboards
            foldersFromFilesStructure: true
```

**监控仪表板**:
```json
{
  "dashboard": {
    "title": "等保三级合规监控",
    "panels": [
      {
        "title": "身份认证失败率",
        "type": "graph",
        "thresholds": {
          "warning": 0.05,
          "critical": 0.10
        }
      },
      {
        "title": "敏感数据访问审计",
        "type": "table",
        "datasource": "PostgreSQL"
      },
      {
        "title": "入侵检测告警",
        "type": "alertlist",
        "severity": ["critical", "high"]
      },
      {
        "title": "数据完整性校验",
        "type": "stat",
        "targets": [
          {
            "expr": "integrity_check_failures_total"
          }
        ]
      }
    ]
  }
}
```

#### 5.2 审计管理

**要求**: 审计覆盖到每个用户

**实施方案**:
```rust
// src/audit/comprehensive_auditing.rs
use chrono::Utc;

pub struct AuditManager {
    logger: AuditLogger,
}

impl AuditManager {
    pub fn log_all_actions(&self, user: &User, action: &str, resource: &str) {
        // 等保三级要求: 审计记录包括:
        // - 事件日期和时间
        // - 用户标识
        // - 事件类型
        // - 事件结果(成功/失败)

        let event = AuditEvent {
            timestamp: Utc::now(),
            user_id: user.id.clone(),
            username: user.username.clone(),
            action: action.to_string(),
            resource: resource.to_string(),
            ip_address: user.current_ip.clone(),
            result: EventResult::Success,
            details: HashMap::new(),
        };

        self.logger.record(event);
    }

    /// 审计记录留存时间不少于6个月(等保要求)
    pub fn retention_policy(&self) -> Duration {
        Duration::days(180)  // 6个月
    }

    /// 审计记录防篡改
    pub fn protect_audit_trail(&self) {
        // 1. 写入后立即封存
        // 2. 使用WORM存储(Write Once Read Many)
        // 3. 定期导出到离线存储
    }
}
```

**审计日志格式**:
```json
{
  "audit_version": "1.0",
  "event_id": "uuid-v4",
  "timestamp": "2026-05-22T10:30:00Z",
  "actor": {
    "user_id": "usr_123",
    "username": "zhang.san",
    "role": "developer",
    "organization": "acme-corp"
  },
  "action": "read_source_code",
  "resource": {
    "type": "git_repository",
    "id": "repo:backend-api",
    "path": "src/auth/mod.rs"
  },
  "outcome": {
    "status": "success",
    "duration_ms": 45
  },
  "context": {
    "ip_address": "10.0.1.100",
    "user_agent": "CarpAI/1.0",
    "session_id": "sess_xyz"
  },
  "integrity_tag": "hmac-sha256-base64..."
}
```

---

## 安全管理要求

### 1. 安全管理制度

#### 1.1 制度制定

**要求**: 制定网络安全工作的总体方针和安全策略

**制度清单**:
```markdown
📋 必需制度文档:

1. **总体方针**
   - 《网络安全总体方针》
   - 《信息安全管理办法》

2. **管理制度**
   - 《人员安全管理制度》
   - 《系统建设管理制度》
   - 《系统运维管理制度》
   - 《数据安全管理制度》
   - 《应急响应预案》

3. **操作规程**
   - 《服务器安全配置手册》
   - 《数据库安全操作规范》
   - 《代码安全开发规范》
   - 《变更管理流程》

4. **记录表单**
   - 《安全培训记录表》
   - 《安全检查记录表》
   - 《安全事件报告表》
   - 《备份恢复测试记录》
```

**行动项**: 创建制度文档模板
```markdown
# 网络安全总体方针

## 第一章 总则

**第一条** 为保障CarpAI系统网络安全，根据《中华人民共和国网络安全法》
和GB/T 22239-2019《信息安全技术 网络安全等级保护基本要求》，制定本方针。

**第二条** 本方针适用于CarpAI所有系统、数据和人员。

**第三条** 网络安全工作遵循以下原则:
1. 谁主管谁负责
2. 预防为主，综合防范
3. 分级保护，责任到人
4. 持续改进，动态调整

## 第二章 组织体系

**第四条** 成立网络安全领导小组，由CEO担任组长，CTO担任副组长。

**第五条** 设立网络安全管理部门，配备专职安全管理人员不少于2人。

## 第三章 安全目标

**第六条** 安全目标:
1. 不发生较大及以上网络安全事件
2. 系统可用性不低于99.9%
3. 数据泄露事件为零
4. 等保三级测评通过率100%

## 第四章 附则

**第七条** 本方针自发布之日起施行，每年评审一次。

签发人: _____________
日期: 2026-__-__
```

#### 1.2 制度发布

**要求**: 正式发文，传达到相关人员

**实施方案**:
```markdown
📋 发布流程:

1. **审批**
   - 部门负责人审核
   - 法务部门合规审查
   - CEO签发

2. **发布**
   - 公司OA系统发布
   - 全员邮件通知
   - 内部Wiki公示

3. **签收**
   - 员工阅读后电子签名
   - 保存签收记录
   - 未签收者提醒

4. **培训**
   - 新员工入职培训必含
   - 年度复训
   - 考试合格(80分以上)
```

---

### 2. 安全管理机构

#### 2.1 岗位设置

**要求**: 设立专门的安全管理部门

**组织架构**:
```
网络安全领导小组
├── 组长: CEO
├── 副组长: CTO
└── 成员: 各部门负责人

网络安全管理部(专职)
├── 安全经理(1人)
│   ├── 安全架构师(1人)
│   ├── 安全运维工程师(2人)
│   └── 安全审计员(1人)
└── 兼职安全员(各部门1人)

总计: 6专职 + N兼职
```

**岗位职责**:
```markdown
### 安全经理职责
1. 制定安全策略和制度
2. 组织安全培训和演练
3. 协调安全事件处置
4. 对接等保测评机构

### 安全运维工程师职责
1. 日常安全监控
2. 漏洞扫描和修复
3. 安全设备维护
4. 应急响应技术支持

### 安全审计员职责
1. 审计日志分析
2. 合规性检查
3. 内部审计报告
4. 整改措施跟踪
```

#### 2.2 人员配备

**要求**: 配备足够的安全管理人员

**实施方案**:
```yaml
# HR安全岗位要求
security_staff_requirements:
  security_manager:
    education: "本科及以上，计算机相关专业"
    experience: "5年以上网络安全工作经验"
    certifications:
      - CISSP (Certified Information Systems Security Professional)
      - CISP (注册信息安全专业人员)
      - 等保测评师证书

  security_engineer:
    education: "本科及以上"
    experience: "3年以上安全运维经验"
    skills:
      - Kubernetes安全
      - 渗透测试
      - 应急响应
      - 脚本编程(Python/Go)

  background_check:
    required: true
    items:
      - 无犯罪记录证明
      - 学历验证
      - 工作经历核实
      - 信用报告
```

---

### 3. 人员安全管理

#### 3.1 录用管理

**要求**: 对录用人员进行背景审查

**实施方案**:
```markdown
📋 入职流程:

1. **背景调查**
   - 身份证验证
   - 学历学位验证(学信网)
   - 无犯罪记录证明(派出所)
   - 前雇主评价

2. **保密协议**
   - 签署《保密协议》
   - 签署《竞业限制协议》(关键岗位)
   - 明确违约责任

3. **安全培训**
   - 网络安全意识培训(8学时)
   - 等保三级要求培训(4学时)
   - 考试合格后方可上岗

4. **账号开通**
   - 最小权限原则
   - MFA强制启用
   - 试用期观察(3个月)
```

#### 3.2 离岗管理

**要求**: 离岗时立即终止所有访问权限

**实施方案**:
```rust
// src/hr/offboarding.rs
use chrono::Utc;

pub struct OffboardingProcess {
    hr_system: HrSystem,
    iam_system: IdentityAccessManagement,
}

impl OffboardingProcess {
    pub async fn terminate_employee(&self, employee_id: &str) -> Result<()> {
        let now = Utc::now();

        // 1. 立即禁用所有账号
        self.iam_system.disable_all_accounts(employee_id).await?;

        // 2. 回收设备
        self.hr_system.collect_devices(employee_id).await?;

        // 3. 撤销API密钥
        self.iam_system.revoke_api_keys(employee_id).await?;

        // 4. 转移工作资料
        self.hr_system.transfer_work_files(employee_id).await?;

        // 5. 记录离岗审计
        self.log_offboarding(employee_id, now).await?;

        // 6. 发送离岗通知
        self.notify_teams(employee_id).await?;

        Ok(())
    }

    /// SLA: 离岗后1小时内完成所有操作
    pub fn sla_deadline() -> Duration {
        Duration::hours(1)
    }
}
```

---

### 4. 系统建设管理

#### 4.1 定级备案

**要求**: 确定系统安全保护等级并备案

**实施方案**:
```markdown
📋 定级流程:

1. **系统调研**
   - 系统功能描述
   - 数据处理类型
   - 用户规模
   - 业务重要性

2. **等级确定**
   - 自评: 等保三级
   - 理由:
     * 处理企业源代码(知识产权)
     * 日均200+活跃用户
     * 中断将严重影响业务

3. **专家评审**
   - 邀请3名以上等保专家
   - 召开定级评审会
   - 形成评审意见

4. **主管部门审核**
   - 提交定级报告
   - 公安局网安支队审核
   - 取得备案证明

5. **备案材料**
   - 《信息系统安全等级保护定级报告》
   - 《信息系统安全等级保护备案表》
   - 系统拓扑图
   - 安全管理制度清单
```

**定级报告模板**:
```markdown
# 信息系统安全等级保护定级报告

## 一、系统基本情况

**系统名称**: CarpAI企业服务器
**运营单位**: XXX科技有限公司
**系统简介**: AI编程助手平台，服务于200+开发者

## 二、定级对象描述

**业务范围**: 代码智能、AI对话、实时协作
**网络拓扑**: Kubernetes集群，3可用区部署
**数据类型**: 源代码、用户凭据、会话记录

## 三、安全保护等级确定

**受侵害客体**: 公民、法人和其他组织的合法权益
**侵害程度**: 严重损害

**定级结论**: 第三级

**定级依据**: GB/T 22240-2020《信息安全技术 网络安全等级保护定级指南》

## 四、专家评审意见

专家组一致认为该系统定为第三级合理。

专家签字: _____________
日期: 2026-__-__
```

#### 4.2 安全方案设计

**要求**: 编制系统安全方案设计

**实施方案**:
```markdown
📋 安全设计方案:

### 1. 物理安全
- 阿里云多可用区机房
- UPS不间断电源
- 气体灭火系统
- 温湿度监控

### 2. 网络安全
- VPC私有网络
- 安全组隔离
- WAF防火墙
- DDoS防护(100Gbps)

### 3. 主机安全
- CentOS 7.9 hardened
- 定期补丁更新
- HIDS主机入侵检测
- 堡垒机运维审计

### 4. 应用安全
- OWASP Top 10防护
- 输入验证
- 会话管理
- CSRF/XSS防护

### 5. 数据安全
- AES-256/SM4加密
- 数据库透明加密
- 备份加密
- 脱敏处理

### 6. 备份恢复
- 每日全量备份
- binlog实时备份
- 异地备份(北京+上海)
- 季度恢复演练
```

---

### 5. 系统运维管理

#### 5.1 环境管理

**要求**: 指定专门部门负责环境管理

**实施方案**:
```yaml
# 机房环境监控
environment_monitoring:
  temperature:
    min_celsius: 18
    max_celsius: 27
    alert_threshold: 25

  humidity:
    min_percent: 40
    max_percent: 60
    alert_threshold: 55

  power:
    ups_backup_minutes: 30
    generator_auto_start: true

  fire_detection:
    smoke_detector: true
    gas_suppression: FM200
    evacuation_alarm: true
```

#### 5.2 介质管理

**要求**: 妥善保管各类介质

**实施方案**:
```markdown
📋 介质管理流程:

### 存储介质分类
1. **硬盘**
   - 生产环境SSD
   - 备份硬盘
   - 报废硬盘(消磁处理)

2. **移动存储**
   - U盘(禁止接入生产环境)
   - 移动硬盘(加密存储)

3. **纸质介质**
   - 打印的配置文件(碎纸机销毁)
   - 合同文档(保险柜存储)

### 管理要求
- 建立介质台账
- 出入库登记
- 定期盘点(每季度)
- 报废审批流程
```

#### 5.3 设备维护

**要求**: 定期进行维护

**实施方案**:
```yaml
# 维护计划
maintenance_schedule:
  daily:
    - 检查系统日志
    - 监控CPU/内存/磁盘
    - 验证备份完成状态

  weekly:
    - 漏洞扫描
    - 清理临时文件
    - 检查证书有效期

  monthly:
    - 操作系统补丁更新
    - 数据库性能优化
    - 安全策略审查

  quarterly:
    - 渗透测试
    - 灾难恢复演练
    - 等保自查

  annually:
    - 等保三级复测
    - 安全架构评审
    - 制度修订
```

---

### 6. 恶意代码防范

**要求**: 采取防范措施

**实施方案**:
```yaml
# kubernetes/security/clamav-deployment.yaml
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: clamav-antivirus
spec:
  template:
    spec:
      containers:
        - name: clamav
          image: clamav/clamav:latest
          volumeMounts:
            - mountPath: /var/lib/clamav
              name: virus-db
          resources:
            limits:
              memory: 2Gi
              cpu: 1

# 病毒库每日更新
cronjob:
  schedule: "0 2 * * *"  # 每天凌晨2点
  command: freshclam
```

**防护策略**:
```markdown
📋 恶意代码防范措施:

1. **终端防护**
   - ClamAV杀毒软件
   - 实时监控文件变化
   - 每周全盘扫描

2. **邮件网关**
   - 附件病毒扫描
   - 钓鱼邮件检测
   - 垃圾邮件过滤

3. **Web防护**
   - WAF拦截恶意上传
   - 文件类型白名单
   - 沙箱检测可疑文件

4. **应急响应**
   - 发现病毒立即隔离
   - 溯源分析
   - 全网查杀
```

---

### 7. 应急预案

**要求**: 制定应急预案并演练

**实施方案**:
```markdown
# 网络安全事件应急预案

## 一、事件分级

### 特别重大事件(I级)
- 核心数据泄露(>10万条)
- 服务中断>24小时
- 被监管机构通报

### 重大事件(II级)
- 重要数据泄露(1-10万条)
- 服务中断4-24小时
- 媒体负面报道

### 较大事件(III级)
- 一般数据泄露(<1万条)
- 服务中断1-4小时
- 用户投诉

### 一般事件(IV级)
- 未造成实际损失
- 服务中断<1小时
- 内部发现

## 二、应急处置流程

### 1. 事件报告
- 发现者 → 安全经理(15分钟内)
- 安全经理 → 领导小组(30分钟内)
- 领导小组 → 上级主管部门(1小时内)

### 2. 先期处置
- 隔离受影响系统
- 保存现场证据
- 启动备用系统

### 3. 应急响应
- 成立应急小组
- 制定处置方案
- 实施技术措施

### 4. 后期处置
- 系统恢复验证
- 损失评估
- 总结报告

## 三、应急演练

### 演练频率
- I级事件: 每半年1次
- II级事件: 每季度1次
- III/IV级事件: 每月1次

### 演练记录
- 演练方案
- 参演人员签到
- 演练过程记录
- 总结评估报告
```

---

## 实施清单

### Phase 1: 差距分析(第1个月)

- [ ] 聘请等保咨询机构
- [ ] 开展现状调研
- [ ] 识别差距项
- [ ] 制定整改计划
- [ ] 预算审批

### Phase 2: 技术整改(第2-3个月)

- [ ] 部署WAF防火墙
- [ ] 实施MFA双因素认证
- [ ] 配置数据库加密
- [ ] 部署IDS/IPS
- [ ] 完善审计日志
- [ ] 配置备份策略

### Phase 3: 管理整改(第3-4个月)

- [ ] 编写安全管理制度
- [ ] 成立安全管理机构
- [ ] 人员背景审查
- [ ] 安全培训(全员)
- [ ] 签订保密协议
- [ ] 制定应急预案

### Phase 4: 自查自纠(第4个月)

- [ ] 内部模拟测评
- [ ] 渗透测试
- [ ] 漏洞修复
- [ ] 制度演练
- [ ] 整改验收

### Phase 5: 正式测评(第5-6个月)

- [ ] 选择测评机构(CCNNC授权)
- [ ] 提交测评申请
- [ ] 现场测评(5-10天)
- [ ] 问题整改(如有)
- [ ] 取得测评报告
- [ ] 公安机关备案 ✅

---

## 制度模板

### 1. 数据安全管理制度

```markdown
# 数据安全管理制度

## 第一章 总则

**第一条** 为规范数据安全管理，根据《网络安全法》和等保三级要求，制定本制度。

**第二条** 本制度适用于公司所有数据的采集、存储、使用、加工、传输、提供、公开等活动。

## 第二章 数据分类分级

**第三条** 数据分为四级:
1. 公开数据: 可对外公开
2. 内部数据: 仅限内部使用
3. 敏感数据: 泄露可能造成损害
4. 核心数据: 泄露可能造成严重损害

**第四条** 源代码、用户凭据属于敏感数据，加密存储。

## 第三章 数据访问控制

**第五条** 实行最小权限原则，仅授权必要人员访问。

**第六条** 敏感数据访问需审批，并记录审计日志。

**第七条** 批量导出数据需部门负责人批准。

## 第四章 数据备份与恢复

**第八条** 每日自动备份，保留30天。

**第九条** 每季度进行恢复演练，确保备份有效。

**第十条** 备份数据异地存储，防火防灾。

## 第五章 数据销毁

**第十一条** 存储介质报废前，进行数据擦除或物理销毁。

**第十二条** 数据销毁需两人以上在场，并记录。

## 第六章 附则

**第十三条** 违反本制度，视情节给予警告、罚款、解除劳动合同等处理。

**第十四条** 本制度自发布之日起施行。
```

### 2. 应急响应预案

```markdown
# 网络安全事件应急响应预案

## 一、组织机构

**应急领导小组**:
- 组长: CEO
- 副组长: CTO
- 成员: 各部门负责人

**应急技术小组**:
- 组长: 安全经理
- 成员: 安全工程师、运维工程师、开发人员

## 二、响应流程

### 1. 事件发现
- 监控系统告警
- 用户报告
- 安全团队巡检

### 2. 事件定级
- 初步判断事件等级
- 上报相应层级领导

### 3. 应急处置
- 隔离受影响系统
- 收集证据
- 修复漏洞
- 恢复服务

### 4. 事后总结
- 编写事件报告
- 分析根本原因
- 制定改进措施
- 更新应急预案

## 三、联系方式

**7×24小时值班电话**: XXX-XXXX-XXXX

**应急邮箱**: security@carpai.example.com

**外部支持**:
- 阿里云安全团队: 400-XXX-XXXX
- 等保测评机构: XXX-XXXX-XXXX
- 公安机关网安支队: 110
```

---

## 证据收集指南

### 技术类证据

1. **网络拓扑图**
   - Visio绘制的详细拓扑
   - 标注安全设备位置
   - IP地址规划

2. **配置截图**
   - WAF规则配置
   - 防火墙策略
   - MFA启用状态
   - 加密算法配置

3. **日志样本**
   - 审计日志(JSON格式)
   - 入侵检测告警
   - 备份成功记录

4. **测试报告**
   - 渗透测试报告(第三方)
   - 漏洞扫描报告(Nessus/OpenVAS)
   - 代码审计报告(SonarQube)

### 管理类证据

1. **制度文档**
   - PDF正式版(带签章)
   - 发布通知邮件
   - 员工签收记录

2. **培训记录**
   - 培训课件
   - 参训人员签到表
   - 考试成绩单

3. **会议纪要**
   - 安全领导小组会议
   - 风险评估会议
   - 应急演练总结

4. **合同协议**
   - 保密协议样本
   - 云服务商SLA
   - 等保咨询服务合同

---

## 下一步行动

1. **本周内**:
   - 任命等保项目负责人
   - 联系等保咨询机构
   - 启动差距分析

2. **本月内**:
   - 完成制度文档初稿
   - 部署技术控制措施
   - 开展全员安全培训

3. **本季度内**:
   - 完成内部模拟测评
   - 整改发现的问题
   - 提交正式测评申请

4. **半年内**:
   - 通过等保三级测评
   - 取得备案证明
   - 建立持续改进机制

---

## 资源

- **国家标准**:
  - GB/T 22239-2019《信息安全技术 网络安全等级保护基本要求》
  - GB/T 22240-2020《信息安全技术 网络安全等级保护定级指南》
  - GB/T 25070-2019《信息安全技术 网络安全等级保护安全设计技术要求》

- **官方机构**:
  - 公安部网络安全保卫局
  - 中国网络安全审查技术与认证中心(CCNNC)
  - 各省市公安局网安支队

- **服务机构**:
  - 等保咨询: 奇安信、深信服、天融信
  - 渗透测试: 阿里云安全、腾讯云安全
  - 测评机构: CCNC授权机构列表

---

**文档所有者**: 网络安全管理部
**最后更新**: 2026-05-22
**下次评审**: 2026-08-22(每季度)
