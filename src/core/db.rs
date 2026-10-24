use rusqlite::{Connection, OptionalExtension};
use rusqlite_migration::{M, Migrations};

use chrono::{DateTime, Utc};

use super::model::{Channel, HistoryEntry, Video};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Migration(#[from] rusqlite_migration::Error),
}

pub type Result<T> = std::result::Result<T, DbError>;

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(
            "CREATE TABLE subscriptions (
            channel_id TEXT PRIMARY KEY NOT NULL,
            name TEXT NOT NULL,
            avatar_url TEXT
        );",
        ),
        M::up(
            "CREATE TABLE history (
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
        CREATE INDEX history_watched_at ON history (watched_at);",
        ),
    ])
}

/// Persistent storage for subscriptions, history and downloads.
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
                video: Video {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    channel_id: row.get(2)?,
                    channel_name: row.get(3)?,
                    published: row.get::<_, Option<i64>>(4)?.and_then(timestamp),
                    duration_secs: row.get(5)?,
                    thumbnail_url: row.get(6)?,
                },
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
}

fn timestamp(secs: i64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(secs, 0)
}

#[cfg(test)]
mod tests {
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
}
