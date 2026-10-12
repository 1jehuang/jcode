-- Jcode Desktop fleet telemetry (desktop_active, desktop_upgrade, desktop_update).
-- Rows go into `events` (version = Desktop version, from_version for upgrades
-- and updates). Production `events` is at 99 of D1's 100-column cap, so the
-- desktop_update outcome fields live in a detail table keyed by event_id,
-- like install_details / turn_details.
CREATE TABLE IF NOT EXISTS desktop_update_details (
    event_id TEXT PRIMARY KEY,
    install_kind TEXT,
    update_outcome TEXT,
    update_failure_stage TEXT,
    FOREIGN KEY (event_id) REFERENCES events(event_id)
);

CREATE INDEX IF NOT EXISTS idx_desktop_update_details_kind_outcome
    ON desktop_update_details(install_kind, update_outcome);
