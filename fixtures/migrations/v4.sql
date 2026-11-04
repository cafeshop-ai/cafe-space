UPDATE sessions SET snapshot = json_insert(snapshot, '$.capabilities.toolApproval', 'unknown');
PRAGMA user_version = 4;
