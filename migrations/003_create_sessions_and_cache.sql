-- Migration: 003_create_sessions_and_cache.sql
-- Description: Create session storage and collaboration state tables
-- Created: 2026-05-21

-- Active sessions
CREATE TABLE IF NOT EXISTS sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    session_token VARCHAR(255) UNIQUE NOT NULL,
    workspace_id UUID,
    is_active BOOLEAN NOT NULL DEFAULT true,
    last_activity_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    metadata JSONB DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Collaboration rooms
CREATE TABLE IF NOT EXISTS collab_rooms (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    room_id VARCHAR(255) UNIQUE NOT NULL,
    document_id VARCHAR(255) NOT NULL,
    owner_id UUID REFERENCES users(id),
    max_participants INT DEFAULT 20,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Collaboration participants
CREATE TABLE IF NOT EXISTS collab_participants (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    room_id UUID NOT NULL REFERENCES collab_rooms(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    participant_id VARCHAR(255) NOT NULL,
    role VARCHAR(20) NOT NULL DEFAULT 'editor',  -- owner, editor, viewer, commenter
    color VARCHAR(7),  -- Hex color for cursor
    cursor_position JSONB,  -- {line: number, column: number}
    selection_range JSONB,  -- {start: {line, column}, end: {line, column}}
    is_typing BOOLEAN DEFAULT false,
    joined_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(room_id, participant_id)
);

-- Document operations log (for CRDT/OT replay)
CREATE TABLE IF NOT EXISTS document_operations (
    id BIGSERIAL PRIMARY KEY,
    room_id UUID NOT NULL REFERENCES collab_rooms(id) ON DELETE CASCADE,
    operation_id BIGINT NOT NULL,
    participant_id VARCHAR(255) NOT NULL,
    op_type VARCHAR(20) NOT NULL,  -- insert, delete, replace
    position JSONB NOT NULL,  -- {line, column} or {start, end}
    text TEXT,
    vector_clock JSONB,  -- Causal ordering
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(room_id, operation_id)
);

-- Indexes
CREATE INDEX idx_sessions_user_id ON sessions(user_id);
CREATE INDEX idx_sessions_token ON sessions(session_token);
CREATE INDEX idx_sessions_expires_at ON sessions(expires_at);
CREATE INDEX idx_collab_rooms_room_id ON collab_rooms(room_id);
CREATE INDEX idx_collab_participants_room_id ON collab_participants(room_id);
CREATE INDEX idx_collab_participants_user_id ON collab_participants(user_id);
CREATE INDEX idx_document_operations_room_id ON document_operations(room_id);
CREATE INDEX idx_document_operations_created_at ON document_operations(created_at);

-- Comments
COMMENT ON TABLE sessions IS 'Active user sessions';
COMMENT ON TABLE collab_rooms IS 'Collaboration editing rooms';
COMMENT ON TABLE collab_participants IS 'Participants in collaboration rooms';
COMMENT ON TABLE document_operations IS 'Document edit operations for CRDT/OT';
COMMENT ON COLUMN collab_participants.role IS 'Participant role: owner, editor, viewer, commenter';
COMMENT ON COLUMN document_operations.vector_clock IS 'Vector clock for causal ordering of operations';
