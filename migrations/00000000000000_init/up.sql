CREATE TABLE pastes (
    id         TEXT    PRIMARY KEY NOT NULL,
    owner      TEXT    NOT NULL,
    content    TEXT    NOT NULL,
    title      TEXT    NOT NULL,
    publish_at BIGINT  NOT NULL,
    created_at BIGINT  NOT NULL,
    updated_at BIGINT  NOT NULL,
    notified   BOOLEAN NOT NULL DEFAULT 0
);

CREATE INDEX idx_pastes_owner ON pastes (owner);
CREATE INDEX idx_pastes_reveal ON pastes (notified, publish_at);

CREATE TABLE conversations (
    user_id    BIGINT  PRIMARY KEY NOT NULL,
    kind       TEXT    NOT NULL,
    step       TEXT    NOT NULL,
    paste_id   TEXT    NOT NULL DEFAULT '',
    content    TEXT    NOT NULL DEFAULT '',
    title      TEXT    NOT NULL DEFAULT '',
    publish_at BIGINT,
    updated_at BIGINT  NOT NULL
);

CREATE TABLE subscriptions (
    user_id    BIGINT NOT NULL,
    paste_id   TEXT   NOT NULL,
    created_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, paste_id)
);

CREATE INDEX idx_subscriptions_paste ON subscriptions (paste_id);

CREATE TABLE updates (
    update_id   BIGINT PRIMARY KEY NOT NULL,
    received_at BIGINT NOT NULL
);
