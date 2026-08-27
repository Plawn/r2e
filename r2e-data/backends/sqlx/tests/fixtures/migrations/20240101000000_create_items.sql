-- Fixture migration: the table the datasource tests look for to prove that
-- `migrate-at-start` ran (or did not).
CREATE TABLE items (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL
);
