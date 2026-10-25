CREATE TABLE saved_playlists (
    playlist_id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    thumbnail_url TEXT,
    video_count INTEGER
);
