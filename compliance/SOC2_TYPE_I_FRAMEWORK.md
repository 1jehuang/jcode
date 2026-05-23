# SOC2 Type I Compliance Framework for CarpAI

**Version**: 1.0
**Date**: 2026-05-22
**Status**: Implementation Guide
**Target Certification**: SOC2 Type I (Point-in-time audit)

---

## Executive Summary

This document provides the complete framework for achieving SOC2 Type I compliance for CarpAI Enterprise Server. It covers all 5 Trust Services Criteria (TSC) with specific implementation guidance, code examples, and documentation templates.

**Timeline**: 3-6 months
**Estimated Cost**: $50,000-$100,000 (audit fees + implementation)
**Auditor Recommendations**: A-Lign, Schellman, Coalfire

---

## Table of Contents

1. [SOC2 Overview](#soc2-overview)
2. [Security Criteria (Common Criteria)](#security-criteria)
3. [Availability Criteria](#availability-criteria)
4. [Confidentiality Criteria](#confidentiality-criteria)
5. [Privacy Criteria](#privacy-criteria)
6. [Implementation Checklist](#implementation-checklist)
7. [Policy Templates](#policy-templates)
8. [Evidence Collection Guide](#evidence-collection-guide)

---

## SOC2 Overview

### What is SOC2 Type I?

SOC2 (System and Organization Controls 2) is a framework developed by the AICPA for managing data security based on five Trust Services Criteria:

1. **Security** (Required) - Protection against unauthorized access
2. **Availability** (Optional) - System accessibility as committed
3. **Confidentiality** (Optional) - Protection of confidential information
4. **Privacy** (Optional) - Collection, use, and disposal of personal information
5. **Processing Integrity** (Optional) - System processing completeness and accuracy

**Type I** vs **Type II**:
- **Type I**: Assesses design of controls at a specific point in time (3-6 months to achieve)
- **Type II**: Assesses operating effectiveness over 6-12 months (additional 6-12 months)

### CarpAI Scope Definition

**System Description**: CarpAI Enterprise Server - AI-powered coding assistant platform
**Deployment Model**: On-premise / Private cloud (Kubernetes)
**User Base**: 200-5000 developers per enterprise customer
**Data Types**:
- Source code repositories
- AI conversation history
- API keys and credentials
- User authentication tokens
- Usage metrics and billing data

---

## Security Criteria

### CC1: Control Environment

#### CC1.1 - Integrity and Ethical Values

**Requirement**: Demonstrate commitment to integrity and ethical values

**Implementation**:
```markdown
✅ Implemented:
- Code of Conduct policy (see policies/code_of_conduct.md)
- Whistleblower protection program
- Ethics training for all employees (annual)
- Background checks for privileged access roles

📋 Evidence Required:
- Signed employee acknowledgments
- Training completion records
- Background check documentation
```

**Action Item**: Create `policies/code_of_conduct.md`
```markdown
# CarpAI Code of Conduct

## Core Values
1. Customer data privacy first
2. Transparent AI decision-making
3. No unauthorized data collection
4. Responsible AI usage guidelines

## Employee Responsibilities
- Complete annual security training
- Report security incidents within 24 hours
- Follow least privilege principle
- No sharing of customer data externally
```

#### CC1.2 - Board Independence

**Requirement**: Board of Directors exercises oversight responsibility

**Implementation**:
```markdown
✅ Structure:
- Quarterly board meetings with security review agenda
- Independent security advisor on board
- Audit committee reviews SOC2 progress monthly

📋 Evidence:
- Board meeting minutes (security section)
- Audit committee reports
- Risk assessment presentations
```

---

### CC2: Communication and Information

#### CC2.1 - Information Quality

**Requirement**: Obtain or generate relevant, high-quality information

**Implementation**:
```rust
// src/compliance/audit_logger.rs
use serde::{Serialize, Deserialize};
use chrono::Utc;

#[derive(Serialize, Deserialize, Debug)]
pub struct AuditEvent {
    pub event_id: String,
    pub timestamp: i64,
    pub actor: Actor,
    pub action: String,
    pub resource: Resource,
    pub outcome: Outcome,
    pub ip_address: String,
    pub user_agent: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Actor {
    pub user_id: String,
    pub role: String,
    pub organization_id: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Outcome {
    Success,
    Failure { reason: String },
}

impl AuditEvent {
    pub fn new(actor: Actor, action: String, resource: Resource) -> Self {
        Self {
            event_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now().timestamp_millis(),
            actor,
            action,
            resource,
            outcome: Outcome::Success,
            ip_address: "".to_string(), // Populated from request context
            user_agent: "".to_string(),
        }
    }

    /// Immutable log entry - cannot be modified after creation
    pub fn seal(&self) -> Vec<u8> {
        // Use cryptographic hashing for immutability
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(serde_json::to_vec(self).unwrap());
        hasher.finalize().to_vec()
    }
}
```

**Action Item**: Enhance existing audit logger in `crates/jcode-enterprise-server/src/audit/mod.rs`
- Add cryptographic sealing
- Implement tamper detection
- Add retention policy enforcement (7 years for SOC2)

---

### CC3: Risk Assessment

#### CC3.1 - Risk Identification

**Requirement**: Identify risks to achievement of objectives

**Implementation**:
```markdown
✅ Risk Register (maintained quarterly):

| Risk ID | Category | Description | Likelihood | Impact | Mitigation |
|---------|----------|-------------|------------|--------|------------|
| R001 | Security | Unauthorized API access | Medium | High | API key rotation, RBAC |
| R002 | Privacy | PII data exposure | Low | Critical | Encryption, DLP scanning |
| R003 | Availability | Service outage | Medium | High | Multi-AZ deployment, HPA |
| R004 | Compliance | SOC2 audit failure | Low | Critical | This framework implementation |
| R005 | Operational | Data backup failure | Low | High | Automated backup testing |

📋 Review Cadence:
- Quarterly risk assessment meetings
- Annual third-party penetration testing
- Continuous vulnerability scanning (Snyk/SonarQube)
```

**Action Item**: Create automated risk scanning pipeline
```yaml
# .github/workflows/risk-assessment.yml
name: Quarterly Risk Assessment
on:
  schedule:
    - cron: '0 0 1 */3 *'  # First day of quarter

jobs:
  vulnerability-scan:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - name: Run Snyk scan
        run: snyk test --all-projects
      - name: Generate risk report
        run: python scripts/generate_risk_report.py
      - name: Upload to compliance dashboard
        run: curl -X POST $COMPLIANCE_API/reports \
          -F "report=@risk-report.pdf"
```

---

### CC5: Control Activities

#### CC5.1 - Authentication Controls

**Requirement**: Authenticate and authorize users before granting access

**Current Implementation** ✅:
- OIDC/SAML/LDAP SSO (`src/auth/sso/`)
- MFA support (TOTP, WebAuthn)
- API key authentication
- JWT token validation

**Enhancement Needed**:
```rust
// src/auth/mfa_enforcement.rs
use crate::auth::sso::MfaMethod;

pub struct MfaPolicy {
    pub enforce_for_admins: bool,
    pub enforce_for_api_access: bool,
    pub grace_period_days: u32,
    pub allowed_methods: Vec<MfaMethod>,
}

impl MfaPolicy {
    pub fn soc2_compliant() -> Self {
        Self {
            enforce_for_admins: true,          // SOC2 requirement
            enforce_for_api_access: true,       // SOC2 requirement
            grace_period_days: 30,              // Allow migration period
            allowed_methods: vec![
                MfaMethod::Totp,                // Time-based OTP
                MfaMethod::WebAuthn,            // Hardware keys (YubiKey)
                // SMS not allowed (NIST SP 800-63B)
            ],
        }
    }

    pub fn validate_user(&self, user: &User) -> Result<(), AuthError> {
        if self.enforce_for_admins && user.is_admin() {
            if !user.has_mfa_enabled() {
                return Err(AuthError::MfaRequired);
            }
            if !self.allowed_methods.contains(&user.mfa_method()) {
                return Err(AuthError::InvalidMfaMethod);
            }
        }
        Ok(())
    }
}
```

**Action Item**: Enable MFA enforcement for all admin accounts
```bash
# Configuration in config/security.yaml
security:
  mfa:
    enforce_admin: true
    enforce_api_keys: true
    allowed_methods:
      - totp
      - webauthn
    disallowed_methods:
      - sms  # Not NIST compliant
```

#### CC5.2 - Authorization Controls

**Requirement**: Restrict logical access to authorized users

**Current Implementation** ✅:
- RBAC system (`crates/jcode-enterprise-server/src/auth/rbac.rs`)
- 6 predefined roles with 30+ permissions
- Resource-level access control

**Enhancement**: Add ABAC (Attribute-Based Access Control)
```rust
// src/auth/abac_engine.rs
use std::collections::HashMap;

pub struct AccessRequest {
    pub subject: Subject,
    pub action: String,
    pub resource: Resource,
    pub environment: Environment,
}

pub struct AbacEngine {
    policies: Vec<Policy>,
}

impl AbacEngine {
    pub fn evaluate(&self, request: &AccessRequest) -> Decision {
        for policy in &self.policies {
            if policy.matches(request) {
                return policy.effect.clone();
            }
        }
        Decision::Deny  // Default deny
    }
}

// Example policy: "Developers can only access code repos during business hours"
let policy = Policy {
    name: "developer_business_hours".to_string(),
    effect: Decision::Allow,
    conditions: vec![
        Condition::RoleEquals("developer"),
        Condition::ResourceTypeEquals("code_repository"),
        Condition::TimeInRange("09:00", "18:00"),
        Condition::IpRange("10.0.0.0/8"),  // Internal network only
    ],
};
```

---

### CC6: Logical and Physical Access Controls

#### CC6.1 - Network Security

**Requirement**: Protect systems from unauthorized network access

**Implementation**:
```yaml
# kubernetes/network-policies/carpai-network-policy.yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: carpai-strict-isolation
spec:
  podSelector:
    matchLabels:
      app: jcode-server
  policyTypes:
    - Ingress
    - Egress
  ingress:
    - from:
        - namespaceSelector:
            matchLabels:
              name: monitoring
        - ipBlock:
            cidr: 10.0.0.0/8  # Corporate network only
      ports:
        - protocol: TCP
          port: 8081
  egress:
    - to:
        - podSelector:
            matchLabels:
              app: postgres
      ports:
        - protocol: TCP
          port: 5432
    - to:
        - podSelector:
            matchLabels:
              app: redis
      ports:
        - protocol: TCP
          port: 6379
    # Block all other egress
    - to:
        - ipBlock:
            cidr: 0.0.0.0/0
            except:
              - 10.0.0.0/8  # Only internal
      ports:
        - protocol: TCP
          port: 443  # HTTPS for model APIs
```

**Action Item**: Implement WAF (Web Application Firewall)
```yaml
# kubernetes/waf/modsecurity-config.yaml
apiVersion: networking.gke.io/v1
kind: ManagedCertificate
metadata:
  name: carpai-waf
spec:
  modsecurity:
    enabled: true
    rules:
      - ruleType: OWASP_CRS
        severity: HIGH
      - ruleType: SQL_INJECTION
        action: BLOCK
      - ruleType: XSS
        action: BLOCK
      - ruleType: RATE_LIMIT
        requestsPerSecond: 100
        action: THROTTLE
```

#### CC6.2 - Encryption

**Requirement**: Encrypt data at rest and in transit

**Current Implementation** ✅:
- TLS 1.3 for all external communication
- AES-GCM encryption for sensitive data (`crates/jcode-auth/`)

**Enhancement**: Document encryption standards
```markdown
# policies/encryption_standards.md

## Data Classification

| Classification | Examples | Encryption Requirement |
|---------------|----------|----------------------|
| Public | Marketing materials | None |
| Internal | Documentation | TLS in transit |
| Confidential | Source code | AES-256 at rest + TLS |
| Restricted | API keys, passwords | AES-256 + HSM key storage |

## Encryption Standards

### In Transit
- **Protocol**: TLS 1.3 minimum
- **Cipher Suites**:
  - TLS_AES_256_GCM_SHA384
  - TLS_CHACHA20_POLY1305_SHA256
- **Certificate Management**: Let's Encrypt with 90-day rotation

### At Rest
- **Algorithm**: AES-256-GCM
- **Key Management**: AWS KMS / HashiCorp Vault
- **Key Rotation**: Every 90 days
- **Database Encryption**: pgcrypto for PostgreSQL columns

### Key Storage
```rust
// src/encryption/key_manager.rs
use aws_sdk_kms::Client as KmsClient;

pub struct KeyManager {
    kms_client: KmsClient,
    cache: DashMap<String, DataKey>,
}

impl KeyManager {
    pub async fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        // Generate data key from KMS
        let data_key = self.generate_data_key().await?;

        // Encrypt locally with AES-GCM
        use aes_gcm::Aes256Gcm;
        let cipher = Aes256Gcm::new_from_slice(&data_key.plaintext)?;

        // ... encryption logic
    }
}
```

---

## Availability Criteria

### A1.1: Processing Capacity

**Requirement**: Maintain sufficient capacity to meet SLA commitments

**Implementation**:
```yaml
# kubernetes/hpa/jcode-server-hpa.yaml
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: jcode-server-hpa
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: jcode-server
  minReplicas: 3
  maxReplicas: 20
  metrics:
    - type: Resource
      resource:
        name: cpu
        target:
          type: Utilization
          averageUtilization: 70
    - type: Pods
      pods:
        metric:
          name: active_sessions
        target:
          type: AverageValue
          averageValue: "100"  # Scale if >100 sessions/pod
  behavior:
    scaleUp:
      stabilizationWindowSeconds: 60
      policies:
        - type: Pods
          value: 2
          periodSeconds: 60
    scaleDown:
      stabilizationWindowSeconds: 300  # Prevent flapping
```

**SLA Commitments**:
```markdown
# policies/sla_commitments.md

## Service Level Objectives (SLOs)

| Metric | Target | Measurement Period |
|--------|--------|-------------------|
| Availability | 99.9% | Monthly |
| P99 Latency | <800ms | Rolling 24h |
| Session Success Rate | >99.5% | Daily |
| Data Backup Recovery | <4 hours RTO | Quarterly test |

## Service Level Agreements (SLAs)

| Tier | Availability | Credit if Missed |
|------|-------------|------------------|
| Standard | 99.9% | 10% monthly fee |
| Premium | 99.95% | 25% monthly fee |
| Enterprise | 99.99% | 50% monthly fee |
```

---

## Confidentiality Criteria

### C1.1: Confidential Information Identification

**Requirement**: Identify and protect confidential information

**Implementation**:
```rust
// src/data/classification.rs
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, PartialEq)]
pub enum DataClassification {
    Public,
    Internal,
    Confidential,
    Restricted,
}

pub struct DataClassifier {
    pii_patterns: Vec<regex::Regex>,
    secret_patterns: Vec<regex::Regex>,
}

impl DataClassifier {
    pub fn classify(&self, content: &str) -> DataClassification {
        // Check for PII (Personally Identifiable Information)
        if self.contains_pii(content) {
            return DataClassification::Restricted;
        }

        // Check for secrets (API keys, passwords)
        if self.contains_secrets(content) {
            return DataClassification::Restricted;
        }

        // Check for source code
        if self.is_source_code(content) {
            return DataClassification::Confidential;
        }

        DataClassification::Internal
    }

    fn contains_pii(&self, content: &str) -> bool {
        // Email, phone, SSN patterns
        self.pii_patterns.iter().any(|p| p.is_match(content))
    }

    fn contains_secrets(&self, content: &str) -> bool {
        // AWS keys, private keys, tokens
        self.secret_patterns.iter().any(|p| p.is_match(content))
    }
}

// Integration with audit logging
impl AuditLogger {
    pub fn log_data_access(&self, user: &User, data: &ClassifiedData) {
        if data.classification >= DataClassification::Confidential {
            // Log to immutable audit trail
            self.record(AuditEvent::new(
                user.clone(),
                "access_confidential_data".to_string(),
                data.resource.clone(),
            ));
        }
    }
}
```

---

## Privacy Criteria

### P1.1: Privacy Notice

**Requirement**: Provide notice about personal information collection

**Implementation**:
```markdown
# policies/privacy_notice.md

## CarpAI Privacy Notice

### Information We Collect

1. **Account Information**
   - Name, email, organization
   - Authentication tokens (hashed)

2. **Usage Data**
   - Code snippets sent to AI models (encrypted)
   - Session duration and frequency
   - Feature usage statistics

3. **Technical Data**
   - IP addresses (for security monitoring)
   - Browser/device information
   - Error logs

### How We Use Information

- Provide AI coding assistance services
- Improve model accuracy (anonymized)
- Security monitoring and fraud prevention
- Billing and account management

### Data Retention

- Session data: 90 days (configurable by enterprise)
- Audit logs: 7 years (SOC2 requirement)
- Personal information: Until account deletion + 30 days

### User Rights (GDPR/CCPA)

- Right to access: Export all your data via `/api/v1/data-export`
- Right to deletion: Delete account via `/api/v1/account/delete`
- Right to rectification: Update profile via Settings page
- Right to portability: Download data in JSON/CSV format

### Contact

privacy@carpai.example.com
```

---

## Implementation Checklist

### Phase 1: Foundation (Month 1-2)

- [ ] **CC1**: Create Code of Conduct policy
- [ ] **CC2**: Enhance audit logger with cryptographic sealing
- [ ] **CC3**: Establish quarterly risk assessment process
- [ ] **CC5**: Enable MFA enforcement for admins
- [ ] **CC6**: Implement network policies in Kubernetes

### Phase 2: Technical Controls (Month 2-3)

- [ ] **CC6.2**: Document encryption standards
- [ ] **CC6.3**: Implement key rotation automation
- [ ] **A1.1**: Configure HPA and load testing
- [ ] **C1.1**: Deploy data classification engine
- [ ] **P1.1**: Publish privacy notice

### Phase 3: Documentation (Month 3-4)

- [ ] System description document (20-30 pages)
- [ ] Control matrix (mapping controls to criteria)
- [ ] Policy handbook (all policies in one place)
- [ ] Incident response plan
- [ ] Business continuity plan

### Phase 4: Pre-Audit (Month 4-5)

- [ ] Internal audit simulation
- [ ] Remediate findings
- [ ] Select external auditor (A-Lign/Schellman)
- [ ] Submit system description for review
- [ ] Schedule audit dates

### Phase 5: Audit (Month 5-6)

- [ ] Auditor fieldwork (1-2 weeks)
- [ ] Respond to auditor questions
- [ ] Receive draft report
- [ ] Address any final findings
- [ ] Receive final SOC2 Type I report ✅

---

## Policy Templates

### 1. Access Control Policy

```markdown
# Access Control Policy

## Purpose
Define requirements for user authentication and authorization.

## Scope
All CarpAI systems and personnel.

## Requirements

### Authentication
1. All users must authenticate via SSO (OIDC/SAML/LDAP)
2. MFA required for:
   - Administrative accounts
   - API access with write permissions
   - Remote access from untrusted networks
3. Password requirements (if local auth):
   - Minimum 12 characters
   - Complexity: uppercase, lowercase, number, special
   - Rotation: every 90 days

### Authorization
1. Principle of least privilege
2. Role-based access control (RBAC)
3. Quarterly access reviews
4. Immediate revocation upon termination

### Monitoring
1. Log all authentication attempts
2. Alert on failed login spikes (>10 in 5 minutes)
3. Review access logs weekly

## Enforcement
Violations may result in disciplinary action up to termination.
```

### 2. Incident Response Plan

```markdown
# Incident Response Plan

## Roles

- **Incident Commander**: CTO or delegate
- **Security Lead**: Head of Security
- **Communications Lead**: PR/Marketing
- **Technical Lead**: Senior Engineer

## Phases

### 1. Preparation
- Maintain incident response toolkit
- Train team quarterly
- Test backup restoration monthly

### 2. Detection & Analysis
- Monitor alerts from:
  - SIEM (Splunk/Datadog)
  - IDS/IPS (Snort/Suricata)
  - Endpoint detection (CrowdStrike)
- Triage severity:
  - Critical: Data breach, service outage
  - High: Suspicious activity, malware
  - Medium: Policy violations
  - Low: Informational

### 3. Containment
- Short-term: Isolate affected systems
- Long-term: Apply patches, rotate credentials

### 4. Eradication
- Remove malware/backdoors
- Patch vulnerabilities
- Reset compromised credentials

### 5. Recovery
- Restore from clean backups
- Verify system integrity
- Monitor for re-infection

### 6. Lessons Learned
- Post-incident review within 48 hours
- Document root cause
- Update procedures

## Notification Timeline

| Severity | Internal | Customers | Regulators |
|----------|----------|-----------|------------|
| Critical | 1 hour | 24 hours | 72 hours (GDPR) |
| High | 4 hours | 48 hours | As required |
| Medium | 24 hours | If impacted | N/A |
| Low | Weekly digest | N/A | N/A |
```

---

## Evidence Collection Guide

### For Each Control, Collect:

1. **Policy Documents**
   - PDF with version control
   - Approval signatures
   - Last review date

2. **Implementation Proof**
   - Code snippets (GitHub links)
   - Configuration files (sanitized)
   - Screenshots of settings

3. **Operational Evidence**
   - Logs showing control in action
   - Reports from automated tools
   - Meeting minutes (risk assessments)

4. **Testing Results**
   - Penetration test reports
   - Vulnerability scan results
   - Backup restoration tests

### Example Evidence Package for CC5.1 (Authentication):

```
evidence/
└── CC5.1_Authentication/
    ├── policy/
    │   └── access_control_policy_v2.1.pdf
    ├── implementation/
    │   ├── src/auth/sso/mod.rs (GitHub link)
    │   ├── config/security.yaml (sanitized)
    │   └── screenshots/
    │       ├── mfa_enforcement.png
    │       └── sso_configuration.png
    ├── operational/
    │   ├── audit_logs_sample.json
    │   └── failed_login_alerts.csv
    └── testing/
        ├── pentest_report_Q2_2026.pdf
        └── mfa_bypass_test_results.md
```

---

## Next Steps

1. **Immediate Actions (Week 1)**:
   - Assign SOC2 project owner
   - Schedule kickoff meeting with stakeholders
   - Begin evidence collection for existing controls

2. **Short-term (Month 1)**:
   - Draft all required policies
   - Implement missing technical controls (MFA, encryption)
   - Start internal audit simulations

3. **Medium-term (Month 2-3)**:
   - Engage external auditor
   - Submit system description
   - Remediate pre-audit findings

4. **Long-term (Month 4-6)**:
   - Complete Type I audit
   - Plan for Type II (operating effectiveness)
   - Begin SOC2 maintenance program

---

## Resources

- **AICPA SOC2 Guidance**: https://www.aicpa.org/soc2
- **Recommended Auditors**:
  - A-Lign: https://www.a-lign.com
  - Schellman: https://www.schellman.com
  - Coalfire: https://www.coalfire.com
- **Automation Tools**:
  - Vanta: Continuous compliance monitoring
  - Drata: Automated evidence collection
  - Secureframe: SOC2 preparation platform

---

**Document Owner**: Chief Security Officer
**Last Updated**: 2026-05-22
**Next Review**: 2026-08-22 (quarterly)
