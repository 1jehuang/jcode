-- Migration: 002_create_users_and_roles.sql
-- Description: Create users, roles, and permissions tables for RBAC
-- Created: 2026-05-21

-- Users table
CREATE TABLE IF NOT EXISTS users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    username VARCHAR(255) UNIQUE NOT NULL,
    email VARCHAR(255) UNIQUE,
    password_hash VARCHAR(255) NOT NULL,
    display_name VARCHAR(255),
    is_active BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_login_at TIMESTAMPTZ,
    metadata JSONB DEFAULT '{}'::jsonb
);

-- Roles table
CREATE TABLE IF NOT EXISTS roles (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(100) UNIQUE NOT NULL,
    description TEXT,
    permissions BIGINT NOT NULL DEFAULT 0,  -- Bit flags for permissions
    parent_role_id UUID REFERENCES roles(id),
    is_system BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- User-Role assignments
CREATE TABLE IF NOT EXISTS user_roles (
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role_id UUID NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    assigned_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    assigned_by UUID REFERENCES users(id),
    expires_at TIMESTAMPTZ,
    PRIMARY KEY (user_id, role_id)
);

-- API Tokens
CREATE TABLE IF NOT EXISTS api_tokens (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash VARCHAR(255) NOT NULL,
    name VARCHAR(255),
    scopes TEXT[] DEFAULT '{}',
    expires_at TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    revoked_at TIMESTAMPTZ
);

-- OAuth2 Sessions
CREATE TABLE IF NOT EXISTS oauth_sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider VARCHAR(50) NOT NULL,  -- google, github, azuread, etc.
    provider_user_id VARCHAR(255) NOT NULL,
    access_token TEXT,
    refresh_token TEXT,
    token_expires_at TIMESTAMPTZ,
    scopes TEXT[] DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(provider, provider_user_id)
);

-- GDPR Consent records
CREATE TABLE IF NOT EXISTS gdpr_consents (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    consent_type VARCHAR(50) NOT NULL,  -- data_processing, analytics, marketing
    granted BOOLEAN NOT NULL,
    timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ip_address INET,
    user_agent TEXT,
    withdrawal_timestamp TIMESTAMPTZ,
    metadata JSONB DEFAULT '{}'::jsonb,
    UNIQUE(user_id, consent_type)
);

-- Indexes
CREATE INDEX idx_users_username ON users(username);
CREATE INDEX idx_users_email ON users(email);
CREATE INDEX idx_user_roles_user_id ON user_roles(user_id);
CREATE INDEX idx_user_roles_role_id ON user_roles(role_id);
CREATE INDEX idx_api_tokens_user_id ON api_tokens(user_id);
CREATE INDEX idx_oauth_sessions_user_id ON oauth_sessions(user_id);
CREATE INDEX idx_gdpr_consents_user_id ON gdpr_consents(user_id);

-- Insert default system roles
INSERT INTO roles (id, name, description, permissions, is_system) VALUES
    ('00000000-0000-0000-0000-000000000001', 'admin', 'Administrator with full system access', 9223372036854775807, true),  -- ALL permissions
    ('00000000-0000-0000-0000-000000000002', 'developer', 'Developer with code and file access', 1048847, true),
    ('00000000-0000-0000-0000-000000000003', 'viewer', 'Read-only access', 2305, true),
    ('00000000-0000-0000-0000-000000000004', 'auditor', 'Audit and compliance access', 393217, true)
ON CONFLICT (name) DO NOTHING;

-- Comments
COMMENT ON TABLE users IS 'User accounts';
COMMENT ON TABLE roles IS 'RBAC roles with permission bit flags';
COMMENT ON TABLE user_roles IS 'User to role assignments';
COMMENT ON TABLE api_tokens IS 'API authentication tokens';
COMMENT ON TABLE oauth_sessions IS 'OAuth2 provider sessions';
COMMENT ON TABLE gdpr_consents IS 'GDPR consent records for compliance';
COMMENT ON COLUMN roles.permissions IS 'Bit flags representing permissions (see PermissionFlags enum)';
