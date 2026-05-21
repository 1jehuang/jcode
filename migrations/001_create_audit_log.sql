-- Migration: 001_create_audit_log.sql
-- Description: Create audit log table for enterprise compliance and GDPR
-- Created: 2026-05-21

CREATE TABLE IF NOT EXISTS audit_logs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_type VARCHAR(50) NOT NULL,
    severity VARCHAR(20) NOT NULL DEFAULT 'info',
    timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    user_id VARCHAR(255),
    session_id VARCHAR(255),
    source_ip INET,
    resource TEXT,
    action TEXT NOT NULL,
    result TEXT NOT NULL DEFAULT 'success',
    metadata JSONB DEFAULT '{}'::jsonb,
    pii_data JSONB DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes for common queries
CREATE INDEX idx_audit_logs_user_id ON audit_logs(user_id);
CREATE INDEX idx_audit_logs_session_id ON audit_logs(session_id);
CREATE INDEX idx_audit_logs_timestamp ON audit_logs(timestamp);
CREATE INDEX idx_audit_logs_event_type ON audit_logs(event_type);
CREATE INDEX idx_audit_logs_severity ON audit_logs(severity);
CREATE INDEX idx_audit_logs_resource ON audit_logs(resource);

-- GIN index for JSONB metadata queries
CREATE INDEX idx_audit_logs_metadata ON audit_logs USING GIN(metadata);

-- Comment on table
COMMENT ON TABLE audit_logs IS 'Audit log table for tracking all security-relevant events';
COMMENT ON COLUMN audit_logs.event_type IS 'Type of audit event (login_success, permission_denied, etc.)';
COMMENT ON COLUMN audit_logs.severity IS 'Severity level: info, warning, error, critical';
COMMENT ON COLUMN audit_logs.metadata IS 'Additional event metadata as JSON';
COMMENT ON COLUMN audit_logs.pii_data IS 'Personally Identifiable Information (should be anonymized in production)';
