-- Safe to run against an already-migrated database: duplicate-column errors
-- indicate the column is already present.

ALTER TABLE events ADD COLUMN transport_https INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN transport_persistent_ws_fresh INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN transport_persistent_ws_reuse INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN transport_cli_subprocess INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN transport_native_http2 INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN transport_other INTEGER DEFAULT 0;
