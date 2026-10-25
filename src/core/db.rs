use rusqlite::{Connection, OptionalExtension};
use rusqlite_migration::{M, Migrations};

use chrono::{DateTime, Utc};

use std::path::Path;

use super::model::{
    Channel, Download, DownloadKind, DownloadStatus, HistoryEntry, Playlist, SubtitleTrack, Video,
};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Migration(#[from] rusqlite_migration::Error),
}

pub type Result<T> = std::result::Result<T, DbError>;

/// Schema migrations. Never edit a released migration; append a new one.
fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(include_str!("../../migrations/001_subscriptions.sql")),
        M::up(include_str!("../../migrations/002_history.sql")),
        M::up(include_str!("../../migrations/003_downloads.sql")),
        M::up(include_str!("../../migrations/004_feed.sql")),
        M::up(include_str!("../../migrations/005_download_subtitles.sql")),
        M::up(include_str!("../../migrations/006_saved_playlists.sql")),
    ])
}

/// Persistent storage for subscriptions, saved playlists, history and downloads.
pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        migrations().to_latest(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn subscribe(&self, channel: &Channel) -> Result<()> {
        self.conn.execute(
            "INSERT INTO subscriptions (channel_id, name, avatar_url) VALUES (?1, ?2, ?3)
             ON CONFLICT (channel_id) DO UPDATE SET name = excluded.name, avatar_url = excluded.avatar_url",
            (&channel.id, &channel.name, &channel.avatar_url),
        )?;
        Ok(())
    }

    pub fn unsubscribe(&self, channel_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM subscriptions WHERE channel_id = ?1",
            [channel_id],
        )?;
        self.conn
            .execute("DELETE FROM feed WHERE channel_id = ?1", [channel_id])?;
        Ok(())
    }

    pub fn is_subscribed(&self, channel_id: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM subscriptions WHERE channel_id = ?1)",
            [channel_id],
            |row| row.get(0),
        )?)
    }

    /// All subscriptions, ordered by name (case-insensitive).
    pub fn subscriptions(&self) -> Result<Vec<Channel>> {
        let mut stmt = self.conn.prepare(
            "SELECT channel_id, name, avatar_url FROM subscriptions ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Channel {
                id: row.get(0)?,
                name: row.get(1)?,
                avatar_url: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Saves `playlist`, or updates it if it is already saved.
    pub fn save_playlist(&self, playlist: &Playlist) -> Result<()> {
        self.conn.execute(
            "INSERT INTO saved_playlists (playlist_id, name, thumbnail_url, video_count)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (playlist_id) DO UPDATE SET name = excluded.name,
                 thumbnail_url = excluded.thumbnail_url, video_count = excluded.video_count",
            (
                &playlist.id,
                &playlist.name,
                &playlist.thumbnail_url,
                playlist.video_count.map(|n| n as i64),
            ),
        )?;
        Ok(())
    }

    pub fn unsave_playlist(&self, playlist_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM saved_playlists WHERE playlist_id = ?1",
            [playlist_id],
        )?;
        Ok(())
    }

    pub fn is_playlist_saved(&self, playlist_id: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM saved_playlists WHERE playlist_id = ?1)",
            [playlist_id],
            |row| row.get(0),
        )?)
    }

    /// All saved playlists, ordered by name (case-insensitive).
    pub fn saved_playlists(&self) -> Result<Vec<Playlist>> {
        let mut stmt = self.conn.prepare(
            "SELECT playlist_id, name, thumbnail_url, video_count FROM saved_playlists
             ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Playlist {
                id: row.get(0)?,
                name: row.get(1)?,
                thumbnail_url: row.get(2)?,
                video_count: row.get::<_, Option<i64>>(3)?.map(|n| n as u64),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Records that `video` was watched at `at`, stopping at `position_secs`.
    /// Watching a video again replaces its previous entry.
    pub fn record_watch(&self, video: &Video, position_secs: u32, at: DateTime<Utc>) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO history (video_id, title, channel_id, channel_name, published,
                 duration_secs, thumbnail_url, watched_at, position_secs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            (
                &video.id,
                &video.title,
                &video.channel_id,
                &video.channel_name,
                video.published.map(|p| p.timestamp()),
                video.duration_secs,
                &video.thumbnail_url,
                at.timestamp(),
                position_secs,
            ),
        )?;
        Ok(())
    }

    /// The watch history, most recently watched first.
    pub fn history(&self) -> Result<Vec<HistoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT video_id, title, channel_id, channel_name, published, duration_secs,
                 thumbnail_url, watched_at, position_secs
             FROM history ORDER BY watched_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(HistoryEntry {
                video: video_from_row(row)?,
                watched_at: timestamp(row.get(7)?).unwrap_or_default(),
                position_secs: row.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn resume_position(&self, video_id: &str) -> Result<Option<u32>> {
        Ok(self
            .conn
            .query_row(
                "SELECT position_secs FROM history WHERE video_id = ?1",
                [video_id],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn remove_from_history(&self, video_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM history WHERE video_id = ?1", [video_id])?;
        Ok(())
    }

    pub fn clear_history(&self) -> Result<()> {
        self.conn.execute("DELETE FROM history", [])?;
        Ok(())
    }

    /// Replaces the cached subscription feed with `videos` (kept in order),
    /// refreshed at `at`.
    pub fn store_feed(&self, videos: &[Video], at: DateTime<Utc>) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM feed", [])?;
        for (position, video) in videos.iter().enumerate() {
            tx.execute(
                "INSERT OR IGNORE INTO feed (video_id, title, channel_id, channel_name, published,
                     duration_secs, thumbnail_url, position)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                (
                    &video.id,
                    &video.title,
                    &video.channel_id,
                    &video.channel_name,
                    video.published.map(|p| p.timestamp()),
                    video.duration_secs,
                    &video.thumbnail_url,
                    position as i64,
                ),
            )?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES ('feed_refreshed_at', ?1)",
            [at.timestamp().to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The cached subscription feed, in the order it was stored.
    pub fn feed(&self) -> Result<Vec<Video>> {
        let mut stmt = self.conn.prepare(
            "SELECT video_id, title, channel_id, channel_name, published, duration_secs,
                 thumbnail_url
             FROM feed ORDER BY position",
        )?;
        let rows = stmt.query_map([], video_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// When the feed was last refreshed.
    pub fn feed_refreshed_at(&self) -> Result<Option<DateTime<Utc>>> {
        let value: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'feed_refreshed_at'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value.and_then(|v| v.parse().ok()).and_then(timestamp))
    }

    /// The language of the subtitles the user last chose; `None` if they
    /// turned subtitles off.
    pub fn subtitle_language(&self) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'subtitle_language'",
                [],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_subtitle_language(&self, lang: Option<&str>) -> Result<()> {
        match lang {
            Some(lang) => self.conn.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('subtitle_language', ?1)",
                [lang],
            )?,
            None => self
                .conn
                .execute("DELETE FROM meta WHERE key = 'subtitle_language'", [])?,
        };
        Ok(())
    }

    /// Adds a queued download and returns its id.
    pub fn add_download(
        &self,
        video: &Video,
        kind: DownloadKind,
        path: &Path,
        at: DateTime<Utc>,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO downloads (video_id, title, channel_id, channel_name, published,
                 duration_secs, thumbnail_url, kind, path, status, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'queued', ?10)",
            (
                &video.id,
                &video.title,
                &video.channel_id,
                &video.channel_name,
                video.published.map(|p| p.timestamp()),
                video.duration_secs,
                &video.thumbnail_url,
                kind_to_str(kind),
                path.to_string_lossy(),
                at.timestamp(),
            ),
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn set_download_status(&self, id: i64, status: &DownloadStatus) -> Result<()> {
        let (status, error) = match status {
            DownloadStatus::Queued => ("queued", None),
            DownloadStatus::Running => ("running", None),
            DownloadStatus::Completed => ("completed", None),
            DownloadStatus::Failed(e) => ("failed", Some(e.as_str())),
            DownloadStatus::Cancelled => ("cancelled", None),
        };
        self.conn.execute(
            "UPDATE downloads SET status = ?2, error = ?3 WHERE id = ?1",
            (id, status, error),
        )?;
        Ok(())
    }

    /// Marks a download as completed, saved at `path`.
    pub fn complete_download(&self, id: i64, path: &Path) -> Result<()> {
        self.conn.execute(
            "UPDATE downloads SET status = 'completed', error = NULL, path = ?2 WHERE id = ?1",
            (id, path.to_string_lossy()),
        )?;
        Ok(())
    }

    /// All downloads, newest first.
    pub fn downloads(&self) -> Result<Vec<Download>> {
        self.query_downloads("ORDER BY created_at DESC, id DESC", [])
    }

    /// A completed download of the given video, if any (for offline playback).
    pub fn completed_download(&self, video_id: &str) -> Result<Option<Download>> {
        Ok(self
            .query_downloads(
                "WHERE video_id = ?1 AND status = 'completed' ORDER BY created_at DESC LIMIT 1",
                [video_id],
            )?
            .pop())
    }

    /// Records the subtitle files saved with a download.
    pub fn set_download_subtitles(&self, id: i64, tracks: &[SubtitleTrack]) -> Result<()> {
        self.conn.execute(
            "DELETE FROM download_subtitles WHERE download_id = ?1",
            [id],
        )?;
        for (position, track) in tracks.iter().enumerate() {
            self.conn.execute(
                "INSERT INTO download_subtitles (download_id, position, name, lang, url, auto_generated)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                (
                    id,
                    position as i64,
                    &track.name,
                    &track.lang,
                    &track.url,
                    track.auto_generated,
                ),
            )?;
        }
        Ok(())
    }

    /// The subtitle files saved with a download, in their original order.
    pub fn download_subtitles(&self, id: i64) -> Result<Vec<SubtitleTrack>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, lang, url, auto_generated FROM download_subtitles
             WHERE download_id = ?1 ORDER BY position",
        )?;
        let rows = stmt.query_map([id], |row| {
            Ok(SubtitleTrack {
                name: row.get(0)?,
                lang: row.get(1)?,
                url: row.get(2)?,
                auto_generated: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn remove_download(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM downloads WHERE id = ?1", [id])?;
        self.conn.execute(
            "DELETE FROM download_subtitles WHERE download_id = ?1",
            [id],
        )?;
        Ok(())
    }

    /// Re-queues downloads left running when the app last exited.
    pub fn requeue_interrupted_downloads(&self) -> Result<()> {
        self.conn.execute(
            "UPDATE downloads SET status = 'queued' WHERE status = 'running'",
            [],
        )?;
        Ok(())
    }

    fn query_downloads(
        &self,
        clause: &str,
        params: impl rusqlite::Params,
    ) -> Result<Vec<Download>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT video_id, title, channel_id, channel_name, published, duration_secs,
                 thumbnail_url, id, kind, path, status, error, created_at
             FROM downloads {clause}"
        ))?;
        let rows = stmt.query_map(params, |row| {
            let status = match row.get_ref(10)?.as_str()? {
                "running" => DownloadStatus::Running,
                "completed" => DownloadStatus::Completed,
                "failed" => {
                    DownloadStatus::Failed(row.get::<_, Option<String>>(11)?.unwrap_or_default())
                }
                "cancelled" => DownloadStatus::Cancelled,
                _ => DownloadStatus::Queued,
            };
            Ok(Download {
                video: video_from_row(row)?,
                id: row.get(7)?,
                kind: if row.get_ref(8)?.as_str()? == "audio" {
                    DownloadKind::Audio
                } else {
                    DownloadKind::Video
                },
                path: row.get::<_, String>(9)?.into(),
                status,
                created_at: timestamp(row.get(12)?).unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

/// Reads a [`Video`] from columns 0..7 (id, title, channel_id, channel_name,
/// published, duration_secs, thumbnail_url).
fn video_from_row(row: &rusqlite::Row) -> rusqlite::Result<Video> {
    Ok(Video {
        id: row.get(0)?,
        title: row.get(1)?,
        channel_id: row.get(2)?,
        channel_name: row.get(3)?,
        published: row.get::<_, Option<i64>>(4)?.and_then(timestamp),
        duration_secs: row.get(5)?,
        thumbnail_url: row.get(6)?,
    })
}

fn kind_to_str(kind: DownloadKind) -> &'static str {
    match kind {
        DownloadKind::Video => "video",
        DownloadKind::Audio => "audio",
    }
}

fn timestamp(secs: i64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(secs, 0)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn channel(id: &str, name: &str) -> Channel {
        Channel {
            id: id.into(),
            name: name.into(),
            avatar_url: None,
        }
    }

    #[test]
    fn migrations_are_valid() {
        migrations().validate().unwrap();
    }

    #[test]
    fn subtitle_language_is_remembered_and_cleared() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.subtitle_language().unwrap(), None);

        db.set_subtitle_language(Some("de-DE")).unwrap();
        db.set_subtitle_language(Some("en")).unwrap();
        assert_eq!(db.subtitle_language().unwrap().as_deref(), Some("en"));

        db.set_subtitle_language(None).unwrap();
        assert_eq!(db.subtitle_language().unwrap(), None);
    }

    #[test]
    fn subscribe_and_list_sorted_by_name() {
        let db = Db::open_in_memory().unwrap();
        db.subscribe(&channel("UC2", "zebra")).unwrap();
        db.subscribe(&channel("UC1", "Alpha")).unwrap();

        let names: Vec<_> = db
            .subscriptions()
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, ["Alpha", "zebra"]);
        assert!(db.is_subscribed("UC1").unwrap());
        assert!(!db.is_subscribed("UC3").unwrap());
    }

    #[test]
    fn subscribe_twice_updates_channel() {
        let db = Db::open_in_memory().unwrap();
        db.subscribe(&channel("UC1", "Old")).unwrap();
        let mut renamed = channel("UC1", "New");
        renamed.avatar_url = Some("https://example.com/a.jpg".into());
        db.subscribe(&renamed).unwrap();

        assert_eq!(db.subscriptions().unwrap(), [renamed]);
    }

    #[test]
    fn unsubscribe_removes_channel() {
        let db = Db::open_in_memory().unwrap();
        db.subscribe(&channel("UC1", "A")).unwrap();
        db.unsubscribe("UC1").unwrap();

        assert!(db.subscriptions().unwrap().is_empty());
        assert!(!db.is_subscribed("UC1").unwrap());
    }

    fn playlist(id: &str, name: &str) -> Playlist {
        Playlist {
            id: id.into(),
            name: name.into(),
            thumbnail_url: Some(format!("https://example.com/{id}.jpg")),
            video_count: Some(3),
        }
    }

    #[test]
    fn save_playlists_and_list_sorted_by_name() {
        let db = Db::open_in_memory().unwrap();
        db.save_playlist(&playlist("PL2", "zebra")).unwrap();
        db.save_playlist(&playlist("PL1", "Alpha")).unwrap();

        assert_eq!(
            db.saved_playlists().unwrap(),
            [playlist("PL1", "Alpha"), playlist("PL2", "zebra")]
        );
        assert!(db.is_playlist_saved("PL1").unwrap());
        assert!(!db.is_playlist_saved("PL3").unwrap());
    }

    #[test]
    fn saving_a_playlist_twice_updates_it() {
        let db = Db::open_in_memory().unwrap();
        db.save_playlist(&playlist("PL1", "Old")).unwrap();
        let renamed = Playlist {
            video_count: None,
            ..playlist("PL1", "New")
        };
        db.save_playlist(&renamed).unwrap();

        assert_eq!(db.saved_playlists().unwrap(), [renamed]);
    }

    #[test]
    fn unsaving_removes_a_playlist() {
        let db = Db::open_in_memory().unwrap();
        db.save_playlist(&playlist("PL1", "A")).unwrap();
        db.unsave_playlist("PL1").unwrap();

        assert!(db.saved_playlists().unwrap().is_empty());
        assert!(!db.is_playlist_saved("PL1").unwrap());
    }

    fn video(id: &str) -> Video {
        Video {
            id: id.into(),
            title: format!("Title {id}"),
            channel_id: "UC1".into(),
            channel_name: "Chan".into(),
            published: Some(at(100)),
            duration_secs: Some(600),
            thumbnail_url: Some("https://example.com/t.jpg".into()),
        }
    }

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn history_is_newest_first() {
        let db = Db::open_in_memory().unwrap();
        db.record_watch(&video("a"), 10, at(1000)).unwrap();
        db.record_watch(&video("b"), 20, at(2000)).unwrap();

        let history = db.history().unwrap();
        assert_eq!(
            history,
            [
                HistoryEntry {
                    video: video("b"),
                    watched_at: at(2000),
                    position_secs: 20
                },
                HistoryEntry {
                    video: video("a"),
                    watched_at: at(1000),
                    position_secs: 10
                },
            ]
        );
    }

    #[test]
    fn rewatching_moves_entry_to_top_and_updates_position() {
        let db = Db::open_in_memory().unwrap();
        db.record_watch(&video("a"), 10, at(1000)).unwrap();
        db.record_watch(&video("b"), 20, at(2000)).unwrap();
        db.record_watch(&video("a"), 30, at(3000)).unwrap();

        let ids: Vec<_> = db
            .history()
            .unwrap()
            .into_iter()
            .map(|e| e.video.id)
            .collect();
        assert_eq!(ids, ["a", "b"]);
        assert_eq!(db.resume_position("a").unwrap(), Some(30));
        assert_eq!(db.resume_position("unknown").unwrap(), None);
    }

    #[test]
    fn remove_and_clear_history() {
        let db = Db::open_in_memory().unwrap();
        db.record_watch(&video("a"), 0, at(1)).unwrap();
        db.record_watch(&video("b"), 0, at(2)).unwrap();

        db.remove_from_history("a").unwrap();
        let ids: Vec<_> = db
            .history()
            .unwrap()
            .into_iter()
            .map(|e| e.video.id)
            .collect();
        assert_eq!(ids, ["b"]);

        db.clear_history().unwrap();
        assert!(db.history().unwrap().is_empty());
    }

    #[test]
    fn add_download_is_queued_and_listed_newest_first() {
        let db = Db::open_in_memory().unwrap();
        let a = db
            .add_download(
                &video("a"),
                DownloadKind::Video,
                Path::new("/d/a.mp4"),
                at(1),
            )
            .unwrap();
        let b = db
            .add_download(
                &video("b"),
                DownloadKind::Audio,
                Path::new("/d/b.opus"),
                at(2),
            )
            .unwrap();
        assert_ne!(a, b);

        assert_eq!(
            db.downloads().unwrap(),
            [
                Download {
                    id: b,
                    video: video("b"),
                    kind: DownloadKind::Audio,
                    path: "/d/b.opus".into(),
                    status: DownloadStatus::Queued,
                    created_at: at(2),
                },
                Download {
                    id: a,
                    video: video("a"),
                    kind: DownloadKind::Video,
                    path: "/d/a.mp4".into(),
                    status: DownloadStatus::Queued,
                    created_at: at(1),
                },
            ]
        );
    }

    #[test]
    fn download_status_roundtrips() {
        let db = Db::open_in_memory().unwrap();
        let id = db
            .add_download(
                &video("a"),
                DownloadKind::Video,
                Path::new("/d/a.mp4"),
                at(1),
            )
            .unwrap();
        for status in [
            DownloadStatus::Running,
            DownloadStatus::Failed("network error".into()),
            DownloadStatus::Cancelled,
            DownloadStatus::Completed,
        ] {
            db.set_download_status(id, &status).unwrap();
            assert_eq!(db.downloads().unwrap()[0].status, status);
        }
    }

    #[test]
    fn completed_download_only_returns_completed() {
        let db = Db::open_in_memory().unwrap();
        let id = db
            .add_download(
                &video("a"),
                DownloadKind::Video,
                Path::new("/d/a.mp4"),
                at(1),
            )
            .unwrap();
        assert_eq!(db.completed_download("a").unwrap(), None);

        db.set_download_status(id, &DownloadStatus::Completed)
            .unwrap();
        assert_eq!(db.completed_download("a").unwrap().map(|d| d.id), Some(id));
    }

    #[test]
    fn download_subtitles_are_stored_and_removed_with_the_download() {
        let db = Db::open_in_memory().unwrap();
        let add = |id| {
            db.add_download(
                &video(id),
                DownloadKind::Video,
                Path::new("/d/a.mp4"),
                at(1),
            )
            .unwrap()
        };
        let (a, b) = (add("a"), add("b"));
        let track = |lang: &str, auto_generated| SubtitleTrack {
            name: format!("Name {lang}"),
            lang: lang.into(),
            url: format!("file:///d/a.{lang}.vtt"),
            auto_generated,
        };
        let tracks = [track("en", false), track("de", true)];
        db.set_download_subtitles(a, &tracks).unwrap();
        db.set_download_subtitles(b, &tracks[..1]).unwrap();

        assert_eq!(db.download_subtitles(a).unwrap(), tracks);
        db.set_download_subtitles(a, &tracks[1..]).unwrap();
        assert_eq!(db.download_subtitles(a).unwrap(), tracks[1..], "replaced");

        db.remove_download(a).unwrap();
        assert!(db.download_subtitles(a).unwrap().is_empty());
        assert_eq!(db.download_subtitles(b).unwrap().len(), 1);
    }

    #[test]
    fn remove_download() {
        let db = Db::open_in_memory().unwrap();
        let id = db
            .add_download(
                &video("a"),
                DownloadKind::Video,
                Path::new("/d/a.mp4"),
                at(1),
            )
            .unwrap();
        db.remove_download(id).unwrap();
        assert!(db.downloads().unwrap().is_empty());
    }

    #[test]
    fn interrupted_downloads_are_requeued() {
        let db = Db::open_in_memory().unwrap();
        let running = db
            .add_download(
                &video("a"),
                DownloadKind::Video,
                Path::new("/d/a.mp4"),
                at(1),
            )
            .unwrap();
        let done = db
            .add_download(
                &video("b"),
                DownloadKind::Video,
                Path::new("/d/b.mp4"),
                at(2),
            )
            .unwrap();
        db.set_download_status(running, &DownloadStatus::Running)
            .unwrap();
        db.set_download_status(done, &DownloadStatus::Completed)
            .unwrap();

        db.requeue_interrupted_downloads().unwrap();

        let statuses: Vec<_> = db
            .downloads()
            .unwrap()
            .into_iter()
            .map(|d| (d.id, d.status))
            .collect();
        assert_eq!(
            statuses,
            [
                (done, DownloadStatus::Completed),
                (running, DownloadStatus::Queued)
            ]
        );
    }

    #[test]
    fn feed_is_stored_in_order_and_replaced() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.feed().unwrap().is_empty());
        assert_eq!(db.feed_refreshed_at().unwrap(), None);

        db.store_feed(&[video("b"), video("a")], at(10)).unwrap();
        assert_eq!(db.feed().unwrap(), [video("b"), video("a")]);
        assert_eq!(db.feed_refreshed_at().unwrap(), Some(at(10)));

        db.store_feed(&[video("c")], at(20)).unwrap();
        assert_eq!(db.feed().unwrap(), [video("c")]);
        assert_eq!(db.feed_refreshed_at().unwrap(), Some(at(20)));
    }

    #[test]
    fn unsubscribing_removes_channel_from_feed() {
        let db = Db::open_in_memory().unwrap();
        let mut other = video("o");
        other.channel_id = "UC2".into();
        db.store_feed(&[video("a"), other.clone()], at(1)).unwrap();

        db.unsubscribe("UC1").unwrap();

        assert_eq!(db.feed().unwrap(), [other]);
    }

    #[test]
    fn completing_download_updates_path() {
        let db = Db::open_in_memory().unwrap();
        let id = db
            .add_download(
                &video("a"),
                DownloadKind::Video,
                Path::new("/d/a.mp4"),
                at(1),
            )
            .unwrap();
        db.complete_download(id, Path::new("/d/a.mkv")).unwrap();

        let download = db.completed_download("a").unwrap().unwrap();
        assert_eq!(download.path, PathBuf::from("/d/a.mkv"));
    }
}
