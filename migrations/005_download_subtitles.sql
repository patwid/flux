CREATE TABLE download_subtitles (
    download_id INTEGER NOT NULL,
    position INTEGER NOT NULL,
    name TEXT NOT NULL,
    lang TEXT NOT NULL,
    url TEXT NOT NULL,
    auto_generated INTEGER NOT NULL
);
