CREATE TABLE projects (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    root TEXT NOT NULL,
    identity TEXT NOT NULL UNIQUE,
    cwd_fence TEXT NOT NULL
) STRICT;
PRAGMA user_version = 3;
