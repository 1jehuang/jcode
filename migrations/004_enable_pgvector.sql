-- Migration: 004_enable_pgvector.sql
-- Description: Enable pgvector extension for vector similarity search
-- Created: 2026-05-21
-- Note: Requires PostgreSQL with pgvector extension installed

-- Enable pgvector extension
CREATE EXTENSION IF NOT EXISTS vector;

-- Verify extension is available
SELECT extname, extversion FROM pg_extension WHERE extname = 'vector';
