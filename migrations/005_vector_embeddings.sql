-- Migration: 005_vector_embeddings.sql
-- Description: Create vector embedding tables for semantic search and model caching
-- Created: 2026-05-21
-- Requires: pgvector extension (migration 004)

-- ============================================================================
// Vector embeddings for semantic code search
-- ============================================================================
CREATE TABLE IF NOT EXISTS code_embeddings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    file_path TEXT NOT NULL,
    file_hash VARCHAR(64) NOT NULL,  -- SHA256 hash for change detection
    language VARCHAR(50),
    symbol_name TEXT,  -- Function/class name if applicable
    symbol_type VARCHAR(20),  -- function, class, interface, etc.
    content_hash VARCHAR(64) NOT NULL,  -- Hash of the actual code content
    embedding vector(1536),  -- OpenAI text-embedding-ada-002 dimension
    metadata JSONB DEFAULT '{}'::jsonb,  -- Additional context
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    -- Ensure unique per file+symbol
    UNIQUE(file_path, symbol_name)
);

-- HNSW index for fast approximate nearest neighbor search
CREATE INDEX IF NOT EXISTS idx_code_embeddings_embedding
ON code_embeddings USING hnsw (embedding vector_cosine_ops)
WITH (m = 16, ef_construction = 64);

-- GIN index for metadata filtering
CREATE INDEX IF NOT EXISTS idx_code_embeddings_metadata
ON code_embeddings USING GIN(metadata);

-- B-tree indexes for common queries
CREATE INDEX IF NOT EXISTS idx_code_embeddings_file_path ON code_embeddings(file_path);
CREATE INDEX IF NOT EXISTS idx_code_embeddings_language ON code_embeddings(language);
CREATE INDEX IF NOT EXISTS idx_code_embeddings_symbol_type ON code_embeddings(symbol_type);
CREATE INDEX IF NOT EXISTS idx_code_embeddings_updated_at ON code_embeddings(updated_at);

-- ============================================================================
// Model response cache with vector similarity
-- ============================================================================
CREATE TABLE IF NOT EXISTS model_response_cache (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    model_name VARCHAR(100) NOT NULL,
    prompt_hash VARCHAR(64) NOT NULL,
    prompt_embedding vector(1536),  -- For similarity-based cache lookup
    response_text TEXT,
    response_metadata JSONB DEFAULT '{}'::jsonb,
    tokens_used INTEGER DEFAULT 0,
    cache_hit_count INTEGER DEFAULT 0,
    last_hit_at TIMESTAMPTZ,
    ttl_secs INTEGER NOT NULL DEFAULT 3600,  -- Cache TTL in seconds
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,

    UNIQUE(model_name, prompt_hash)
);

-- Index for similarity search on prompts
CREATE INDEX IF NOT EXISTS idx_model_cache_embedding
ON model_response_cache USING hnsw (prompt_embedding vector_cosine_ops)
WITH (m = 16, ef_construction = 64);

-- Indexes for cleanup and hit tracking
CREATE INDEX IF NOT EXISTS idx_model_cache_expires_at ON model_response_cache(expires_at);
CREATE INDEX IF NOT EXISTS idx_model_cache_model_name ON model_response_cache(model_name);
CREATE INDEX IF NOT EXISTS idx_model_cache_hit_count ON model_response_cache(cache_hit_count DESC);

-- ============================================================================
// KV Cache metadata for inference state persistence
-- ============================================================================
CREATE TABLE IF NOT EXISTS kv_cache_snapshots (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    instance_id VARCHAR(100) NOT NULL,
    model_name VARCHAR(100) NOT NULL,
    request_id VARCHAR(100) NOT NULL,
    snapshot_path TEXT NOT NULL,  -- Path to binary snapshot file
    storage_tier VARCHAR(20) NOT NULL DEFAULT 'ssd',  -- memory, ssd, object_storage
    sequence_length INTEGER NOT NULL DEFAULT 0,
    layer_count INTEGER NOT NULL DEFAULT 0,
    size_bytes BIGINT NOT NULL DEFAULT 0,
    embedding vector(768),  -- Optional: embedding of the conversation context
    metadata JSONB DEFAULT '{}'::jsonb,
    is_active BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_accessed_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ,

    UNIQUE(instance_id, request_id)
);

-- Index for finding snapshots by model
CREATE INDEX IF NOT EXISTS idx_kv_cache_model_name ON kv_cache_snapshots(model_name);

-- Index for expiration-based cleanup
CREATE INDEX IF NOT EXISTS idx_kv_cache_expires_at ON kv_cache_snapshots(expires_at);

-- Index for active snapshots only
CREATE INDEX IF NOT EXISTS idx_kv_cache_is_active ON kv_cache_snapshots(is_active)
WHERE is_active = true;

-- ============================================================================
// Tenant resource pools for isolation
-- ============================================================================
CREATE TABLE IF NOT EXISTS tenant_resource_pools (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id VARCHAR(100) NOT NULL UNIQUE,
    tenant_name VARCHAR(255) NOT NULL,
    pool_config JSONB NOT NULL DEFAULT '{}'::jsonb,  -- Resource limits
    max_concurrent_requests INTEGER NOT NULL DEFAULT 10,
    max_daily_tokens BIGINT NOT NULL DEFAULT 1000000,
    priority_weight FLOAT NOT NULL DEFAULT 1.0,
    allowed_models TEXT[] DEFAULT '{}',  -- Whitelist of accessible models
    embedding vector(768),  -- Tenant profile embedding for smart routing
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Index for tenant lookups
CREATE INDEX IF NOT EXISTS idx_tenant_pools_tenant_id ON tenant_resource_pools(tenant_id);

-- ============================================================================
// Session affinity tracking with cache awareness
-- ============================================================================
CREATE TABLE IF NOT EXISTS session_affinity (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id VARCHAR(100) NOT NULL UNIQUE,
    tenant_id VARCHAR(100) NOT NULL,
    model_name VARCHAR(100) NOT NULL,
    assigned_node_id VARCHAR(100) NOT NULL,
    cache_status VARCHAR(20) NOT NULL DEFAULT 'warm',  -- hot, warm, cold, expired
    sticky_until TIMESTAMPTZ NOT NULL,  -- Session stickiness expiry
    cache_valid_until TIMESTAMPTZ NOT NULL,  -- KV Cache validity
    last_request_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    total_requests INTEGER NOT NULL DEFAULT 0,
    metadata JSONB DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Composite index for session lookups
CREATE INDEX IF NOT EXISTS idx_session_affinity_session_id ON session_affinity(session_id);

-- Index for finding sessions by node (for drain operations)
CREATE INDEX IF NOT EXISTS idx_session_affinity_node ON session_affinity(assigned_node_id);

-- Index for expired sessions cleanup
CREATE INDEX IF NOT EXISTS idx_session_affinity_sticky_until ON session_affinity(sticky_until);

-- Partial index for active sticky sessions
CREATE INDEX IF NOT EXISTS idx_session_affinity_active
ON session_affinity(session_id, tenant_id, model_name)
WHERE cache_status IN ('hot', 'warm');

-- ============================================================================
// Comments
-- ============================================================================
COMMENT ON TABLE code_embeddings IS 'Vector embeddings for semantic code search';
COMMENT ON TABLE model_response_cache IS 'Cache for model responses with similarity lookup';
COMMENT ON TABLE kv_cache_snapshots IS 'Metadata for persisted KV Cache snapshots';
COMMENT ON TABLE tenant_resource_pools IS 'Tenant isolation and resource pool configuration';
COMMENT ON TABLE session_affinity IS 'Session-to-node affinity with cache awareness';

COMMENT ON COLUMN code_embeddings.embedding IS 'Vector embedding of code content';
COMMENT ON COLUMN model_response_cache.prompt_embedding IS 'Embedding for similarity-based cache hits';
COMMENT ON COLUMN kv_cache_snapshots.storage_tier IS 'Storage tier: memory, ssd, object_storage';
COMMENT ON COLUMN tenant_resource_pools.embedding IS 'Tenant profile for intelligent routing';
COMMENT ON COLUMN session_affinity.cache_status IS 'Current cache warmth: hot, warm, cold, expired';
