CREATE TABLE feed (
    video_id TEXT PRIMARY KEY NOT NULL,
    title TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    channel_name TEXT NOT NULL,
    published INTEGER,
    duration_secs INTEGER,
    thumbnail_url TEXT,
    position INTEGER NOT NULL
);
CREATE TABLE meta (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
