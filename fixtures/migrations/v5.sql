CREATE TABLE native_sessions (
    session_id TEXT PRIMARY KEY NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    native_id TEXT NOT NULL
) STRICT;
CREATE TABLE transcript_messages (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    payload TEXT NOT NULL,
    PRIMARY KEY (session_id, id)
) STRICT;
CREATE TABLE transcript_cache (
    session_id TEXT PRIMARY KEY NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    truncated INTEGER NOT NULL CHECK (truncated IN (0, 1))
) STRICT;
PRAGMA user_version = 5;
