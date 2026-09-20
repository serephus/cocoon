CREATE TABLE pastes (
    id         BLOB    PRIMARY KEY NOT NULL,
    title      TEXT    NOT NULL,
    content    BLOB    NOT NULL,
    publish_at BIGINT  NOT NULL,
    created_at BIGINT  NOT NULL
);

CREATE INDEX idx_pastes_publish_at ON pastes (publish_at);
CREATE INDEX idx_pastes_created_at ON pastes (created_at);
