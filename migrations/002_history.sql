CREATE TABLE history (
    video_id TEXT PRIMARY KEY NOT NULL,
    title TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    channel_name TEXT NOT NULL,
    published INTEGER,
    duration_secs INTEGER,
    thumbnail_url TEXT,
    watched_at INTEGER NOT NULL,
    position_secs INTEGER NOT NULL
);
CREATE INDEX history_watched_at ON history (watched_at);
