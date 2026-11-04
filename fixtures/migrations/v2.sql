CREATE TABLE sessions (
    id TEXT PRIMARY KEY NOT NULL,
    create_params TEXT NOT NULL,
    snapshot TEXT NOT NULL
) STRICT;
CREATE TABLE migration_history (
    to_version INTEGER PRIMARY KEY NOT NULL,
    from_version INTEGER NOT NULL,
    backup_file TEXT NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;
PRAGMA user_version = 2;
