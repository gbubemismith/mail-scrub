-- Machine-owned state: a cache that a Full Run can rebuild.

CREATE TABLE run (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at      INTEGER NOT NULL,
    kind            TEXT    NOT NULL CHECK (kind IN ('full', 'incremental')),
    cursor_before   TEXT,
    cursor_after    TEXT,
    labelled        INTEGER NOT NULL DEFAULT 0,
    filters_changed INTEGER NOT NULL DEFAULT 0,
    unknown         INTEGER NOT NULL DEFAULT 0,
    dry_run         INTEGER NOT NULL DEFAULT 0
);

-- One row per message, overwritten by each applying Run that touches it, so
-- it always matches the labels in the mailbox. Dry runs write none.
-- NULL sender_category / email_type = Unknown.
CREATE TABLE decision (
    message_id      TEXT    PRIMARY KEY,
    sender_category TEXT,
    sender_source   TEXT,
    email_type      TEXT,
    type_signal     TEXT,
    signal_key      TEXT,
    run_id          INTEGER NOT NULL REFERENCES run (id)
);
