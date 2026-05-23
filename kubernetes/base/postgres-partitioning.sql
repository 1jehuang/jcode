-- Database partitioning strategy for audit_logs and high-volume tables
-- Monthly partitioning to improve query performance and maintenance

-- Enable pg_partman extension for automatic partition management
CREATE EXTENSION IF NOT EXISTS pg_partman;

-- Create partitioned audit_logs table
CREATE TABLE audit_logs_partitioned (
    id BIGSERIAL,
    event_id UUID NOT NULL DEFAULT gen_random_uuid(),
    user_id VARCHAR(255) NOT NULL,
    tenant_id VARCHAR(255),
    action VARCHAR(100) NOT NULL,
    resource_type VARCHAR(100),
    resource_id VARCHAR(255),
    ip_address INET,
    user_agent TEXT,
    request_payload JSONB,
    response_payload JSONB,
    status_code INTEGER,
    error_message TEXT,
    duration_ms INTEGER,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    metadata JSONB,
    PRIMARY KEY (id, created_at)
) PARTITION BY RANGE (created_at);

-- Create indexes on partitioned table
CREATE INDEX idx_audit_logs_user_id ON audit_logs_partitioned (user_id, created_at DESC);
CREATE INDEX idx_audit_logs_tenant_id ON audit_logs_partitioned (tenant_id, created_at DESC);
CREATE INDEX idx_audit_logs_action ON audit_logs_partitioned (action, created_at DESC);
CREATE INDEX idx_audit_logs_event_id ON audit_logs_partitioned (event_id);
CREATE INDEX idx_audit_logs_resource ON audit_logs_partitioned (resource_type, resource_id, created_at DESC);

-- Migrate existing data from old table
INSERT INTO audit_logs_partitioned SELECT * FROM audit_logs;

-- Rename tables
ALTER TABLE audit_logs RENAME TO audit_logs_legacy;
ALTER TABLE audit_logs_partitioned RENAME TO audit_logs;

-- Grant permissions
GRANT ALL PRIVILEGES ON audit_logs TO carpai;
GRANT ALL PRIVILEGES ON SEQUENCE audit_logs_id_seq TO carpai;

-- Configure automatic partition creation (monthly partitions, retain 12 months)
SELECT partman.create_parent(
    p_parent_table := 'public.audit_logs',
    p_control := 'created_at',
    p_partition_type := 'range',
    p_interval := 'monthly',
    p_premake := 3,
    p_retention := '12 months',
    p_retention_keep_table := false,
    p_retention_keep_index := true
);

-- Update pg_partman configuration
UPDATE partman.part_config
SET
    retention = '12 months',
    retention_keep_table = false,
    retention_keep_index = true,
    automatic_maintenance = 'on'
WHERE parent_table = 'public.audit_logs';

-- Create similar partitioned tables for other high-volume tables

-- Session events partitioning
CREATE TABLE session_events_partitioned (
    id BIGSERIAL,
    session_id UUID NOT NULL,
    user_id VARCHAR(255) NOT NULL,
    event_type VARCHAR(50) NOT NULL,
    event_data JSONB,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    PRIMARY KEY (id, created_at)
) PARTITION BY RANGE (created_at);

CREATE INDEX idx_session_events_session_id ON session_events_partitioned (session_id, created_at DESC);
CREATE INDEX idx_session_events_user_id ON session_events_partitioned (user_id, created_at DESC);
CREATE INDEX idx_session_events_type ON session_events_partitioned (event_type, created_at DESC);

-- Token usage logs partitioning
CREATE TABLE token_usage_logs_partitioned (
    id BIGSERIAL,
    user_id VARCHAR(255) NOT NULL,
    tenant_id VARCHAR(255),
    model_name VARCHAR(100) NOT NULL,
    prompt_tokens INTEGER NOT NULL,
    completion_tokens INTEGER NOT NULL,
    total_tokens INTEGER NOT NULL,
    cost_usd DECIMAL(10, 6),
    request_id UUID,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    PRIMARY KEY (id, created_at)
) PARTITION BY RANGE (created_at);

CREATE INDEX idx_token_usage_user_id ON token_usage_logs_partitioned (user_id, created_at DESC);
CREATE INDEX idx_token_usage_tenant_id ON token_usage_logs_partitioned (tenant_id, created_at DESC);
CREATE INDEX idx_token_usage_model ON token_usage_logs_partitioned (model_name, created_at DESC);
CREATE INDEX idx_token_usage_request_id ON token_usage_logs_partitioned (request_id);

-- Code completion events partitioning
CREATE TABLE completion_events_partitioned (
    id BIGSERIAL,
    user_id VARCHAR(255) NOT NULL,
    tenant_id VARCHAR(255),
    file_path TEXT,
    language VARCHAR(50),
    trigger_type VARCHAR(50),
    accepted BOOLEAN,
    latency_ms INTEGER,
    suggestion_length INTEGER,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    PRIMARY KEY (id, created_at)
) PARTITION BY RANGE (created_at);

CREATE INDEX idx_completion_events_user_id ON completion_events_partitioned (user_id, created_at DESC);
CREATE INDEX idx_completion_events_tenant_id ON completion_events_partitioned (tenant_id, created_at DESC);
CREATE INDEX idx_completion_events_language ON completion_events_partitioned (language, created_at DESC);

-- Apply pg_partman to new partitioned tables
SELECT partman.create_parent(
    p_parent_table := 'public.session_events',
    p_control := 'created_at',
    p_partition_type := 'range',
    p_interval := 'monthly',
    p_premake := 3,
    p_retention := '6 months',
    p_retention_keep_table := false
);

SELECT partman.create_parent(
    p_parent_table := 'public.token_usage_logs',
    p_control := 'created_at',
    p_partition_type := 'range',
    p_interval := 'monthly',
    p_premake := 3,
    p_retention := '12 months',
    p_retention_keep_table := false
);

SELECT partman.create_parent(
    p_parent_table := 'public.completion_events',
    p_control := 'created_at',
    p_partition_type := 'range',
    p_interval := 'monthly',
    p_premake := 3,
    p_retention := '6 months',
    p_retention_keep_table := false
);

-- Create function to get partition statistics
CREATE OR REPLACE FUNCTION get_partition_stats()
RETURNS TABLE (
    table_name TEXT,
    partition_name TEXT,
    row_count BIGINT,
    size_bytes BIGINT,
    start_date TIMESTAMP WITH TIME ZONE,
    end_date TIMESTAMP WITH TIME ZONE
) AS $$
BEGIN
    RETURN QUERY
    SELECT
        parent.relname AS table_name,
        child.relname AS partition_name,
        pg_stat_user_tables.n_live_tup AS row_count,
        pg_total_relation_size(child.oid) AS size_bytes,
        pg_get_expr(relpartbound, child.oid) AS partition_bounds
    FROM pg_inherits
    JOIN pg_class parent ON pg_inherits.inhparent = parent.oid
    JOIN pg_class child ON pg_inherits.inhrelid = child.oid
    LEFT JOIN pg_stat_user_tables ON pg_stat_user_tables.relname = child.relname
    WHERE parent.relname IN ('audit_logs', 'session_events', 'token_usage_logs', 'completion_events')
    ORDER BY parent.relname, child.relname;
END;
$$ LANGUAGE plpgsql;

-- Create view for quick partition monitoring
CREATE VIEW partition_monitoring AS
SELECT
    table_name,
    partition_name,
    row_count,
    pg_size_pretty(size_bytes) AS size_human_readable,
    ROUND(row_count::numeric / NULLIF(size_bytes, 0) * 1024 * 1024, 2) AS rows_per_mb
FROM get_partition_stats();

-- Schedule automatic maintenance (requires pg_cron extension)
-- This runs daily to create new partitions and drop old ones
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'pg_cron') THEN
        -- Run partition maintenance daily at 2 AM
        SELECT cron.schedule(
            'partition-maintenance',
            '0 2 * * *',
            'SELECT partman.run_maintenance()'
        );

        -- Vacuum analyze partitioned tables weekly
        SELECT cron.schedule(
            'vacuum-partitions',
            '0 3 * * 0',
            'VACUUM ANALYZE audit_logs; VACUUM ANALYZE session_events; VACUUM ANALYZE token_usage_logs; VACUUM ANALYZE completion_events;'
        );
    END IF;
END $$;
