-- Synthetic M0 state with committed data; never a captured user database.
PRAGMA application_id = 0x43414645;
PRAGMA user_version = 1;
CREATE TABLE legacy_probe (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
INSERT INTO legacy_probe VALUES (1, 'Fixture preserved'), (2, 'Résumé 茶');
