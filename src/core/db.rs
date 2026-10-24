use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};

use super::model::Channel;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Migration(#[from] rusqlite_migration::Error),
}

pub type Result<T> = std::result::Result<T, DbError>;

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(
        "CREATE TABLE subscriptions (
            channel_id TEXT PRIMARY KEY NOT NULL,
            name TEXT NOT NULL,
            avatar_url TEXT
        );",
    )])
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
}
