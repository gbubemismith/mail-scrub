-- Machine-owned state: a cache that a Full Run can rebuild, except Pins.

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

CREATE TABLE message (
    id             TEXT    PRIMARY KEY,
    thread_id      TEXT    NOT NULL,
    sender_address TEXT    NOT NULL,
    sender_domain  TEXT    NOT NULL,
    internal_date  INTEGER NOT NULL
);

-- One row per message, overwritten by each Run that touches it.
-- NULL sender_category / email_type = Unknown.
CREATE TABLE decision (
    message_id      TEXT    PRIMARY KEY REFERENCES message (id),
    sender_category TEXT,
    email_type      TEXT,
    type_signal     TEXT,
    signal_key      TEXT,
    run_id          INTEGER NOT NULL REFERENCES run (id),
    applied         INTEGER NOT NULL DEFAULT 0
);

-- A message can carry one Pin per dimension, hence the composite key.
-- Rows are never deleted.
CREATE TABLE pin (
    message_id      TEXT    NOT NULL REFERENCES message (id),
    dimension       TEXT    NOT NULL CHECK (dimension IN ('sender', 'type')),
    value           TEXT    NOT NULL,
    against_signal  TEXT,
    seen_run_id     INTEGER NOT NULL REFERENCES run (id),
    PRIMARY KEY (message_id, dimension)
);

-- NULL sender_category = Unknown, the only rows retried on later Runs.
CREATE TABLE domain_cache (
    sender_domain   TEXT    PRIMARY KEY,
    sender_category TEXT,
    source          TEXT    NOT NULL,
    resolved_run_id INTEGER NOT NULL REFERENCES run (id)
);

-- Mirror of the owned filters the tool believes are in Gmail.
CREATE TABLE filter (
    gmail_filter_id TEXT    PRIMARY KEY,
    label           TEXT    NOT NULL,
    criteria_kind   TEXT    NOT NULL CHECK (criteria_kind IN ('from', 'subject', 'list')),
    criteria        TEXT    NOT NULL,
    run_id          INTEGER NOT NULL REFERENCES run (id)
);
