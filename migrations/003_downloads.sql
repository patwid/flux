CREATE TABLE downloads (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    video_id TEXT NOT NULL,
    title TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    channel_name TEXT NOT NULL,
    published INTEGER,
    duration_secs INTEGER,
    thumbnail_url TEXT,
    kind TEXT NOT NULL,
    path TEXT NOT NULL,
    status TEXT NOT NULL,
    error TEXT,
    created_at INTEGER NOT NULL
);
