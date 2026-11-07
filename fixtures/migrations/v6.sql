CREATE TABLE session_activity (
    session_id TEXT PRIMARY KEY NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    last_activity_at INTEGER CHECK (
        last_activity_at IS NULL OR last_activity_at BETWEEN 1 AND 8640000000000000
    ),
    preview TEXT CHECK (
        preview IS NULL OR length(CAST(preview AS BLOB)) BETWEEN 1 AND 256
    ),
    last_user_message_id TEXT CHECK (
        last_user_message_id IS NULL OR length(CAST(last_user_message_id AS BLOB)) BETWEEN 1 AND 128
    ),
    last_user_message_order INTEGER CHECK (
        last_user_message_order IS NULL OR last_user_message_order > 0
    ),
    CHECK ((last_user_message_id IS NULL) = (last_user_message_order IS NULL)),
    CHECK (preview IS NULL OR last_user_message_id IS NOT NULL)
) STRICT;
CREATE TABLE session_message_receipts (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    message_id TEXT NOT NULL CHECK (length(CAST(message_id AS BLOB)) BETWEEN 1 AND 128),
    display_digest BLOB CHECK (display_digest IS NULL OR length(display_digest) = 32),
    user_message_order INTEGER CHECK (user_message_order IS NULL OR user_message_order > 0),
    preview_digest BLOB CHECK (preview_digest IS NULL OR length(preview_digest) = 32),
    PRIMARY KEY (session_id, message_id),
    UNIQUE (session_id, user_message_order),
    CHECK ((user_message_order IS NULL) = (preview_digest IS NULL)),
    CHECK (display_digest IS NOT NULL OR preview_digest IS NOT NULL)
) STRICT;
PRAGMA user_version = 6;
