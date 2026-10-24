//! Download queue: decides which downloads run, and tracks their progress.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::model::{Download, DownloadKind, DownloadStatus, Video};

/// Number of downloads running at the same time.
pub const MAX_PARALLEL: usize = 2;

/// File name (without extension) for a downloaded video: its title, made
/// safe for file systems, followed by its id.
pub fn file_stem(video: &Video) -> String {
    let title: String = video
        .title
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let title: String = title
        .trim_matches(|c: char| c.is_whitespace() || c == '.')
        .chars()
        .take(100)
        .collect();
    if title.is_empty() {
        video.id.clone()
    } else {
        format!("{title} [{}]", video.id)
    }
}

/// Where to save a download of `video` in `dir`.
pub fn download_path(dir: &Path, video: &Video, kind: DownloadKind) -> PathBuf {
    let ext = match kind {
        DownloadKind::Video => "mp4",
        DownloadKind::Audio => "m4a",
    };
    dir.join(format!("{}.{ext}", file_stem(video)))
}

/// The download queue. Downloads are listed newest first; queued downloads
/// start oldest first.
#[derive(Debug)]
pub struct Queue {
    max_parallel: usize,
    pub downloads: Vec<Download>,
    /// Progress of running downloads, from 0 to 1.
    pub progress: HashMap<i64, f64>,
}

#[derive(Debug)]
pub enum Msg {
    /// Downloads restored from the database.
    Load(Vec<Download>),
    /// A newly created (queued) download.
    Add(Download),
    Progress {
        id: i64,
        fraction: f64,
    },
    /// A running download finished, with the final path or an error.
    Finished {
        id: i64,
        result: Result<PathBuf, String>,
    },
    Cancel(i64),
    Retry(i64),
    Remove(i64),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    Start(Download),
    Abort(i64),
    SetStatus(i64, DownloadStatus),
    Complete {
        id: i64,
        path: PathBuf,
    },
    /// Remove the download and delete its file.
    Delete {
        id: i64,
        path: PathBuf,
    },
}

impl Queue {
    pub fn new(max_parallel: usize) -> Self {
        Queue {
            max_parallel,
            downloads: Vec::new(),
            progress: HashMap::new(),
        }
    }

    pub fn update(&mut self, msg: Msg) -> Vec<Effect> {
        let mut effects = Vec::new();
        match msg {
            Msg::Load(mut downloads) => {
                downloads.sort_by_key(|d| std::cmp::Reverse((d.created_at, d.id)));
                self.downloads = downloads;
                self.progress.clear();
            }
            Msg::Add(download) => self.downloads.insert(0, download),
            Msg::Progress { id, fraction } => {
                if self.status(id) == Some(&DownloadStatus::Running) {
                    self.progress.insert(id, fraction);
                }
                return effects;
            }
            Msg::Finished { id, result } => {
                let Some(download) = self.running_mut(id) else {
                    return effects;
                };
                match result {
                    Ok(path) => {
                        download.status = DownloadStatus::Completed;
                        download.path.clone_from(&path);
                        effects.push(Effect::Complete { id, path });
                    }
                    Err(e) => {
                        download.status = DownloadStatus::Failed(e);
                        effects.push(Effect::SetStatus(id, download.status.clone()));
                    }
                }
                self.progress.remove(&id);
            }
            Msg::Cancel(id) => match self.status(id) {
                Some(DownloadStatus::Running) => {
                    effects.push(Effect::Abort(id));
                    effects.extend(self.set_status(id, DownloadStatus::Cancelled));
                    self.progress.remove(&id);
                }
                Some(DownloadStatus::Queued) => {
                    effects.extend(self.set_status(id, DownloadStatus::Cancelled));
                }
                _ => return effects,
            },
            Msg::Retry(id) => match self.status(id) {
                Some(DownloadStatus::Failed(_) | DownloadStatus::Cancelled) => {
                    effects.extend(self.set_status(id, DownloadStatus::Queued));
                }
                _ => return effects,
            },
            Msg::Remove(id) => {
                let Some(index) = self.downloads.iter().position(|d| d.id == id) else {
                    return effects;
                };
                let download = self.downloads.remove(index);
                if download.status == DownloadStatus::Running {
                    effects.push(Effect::Abort(id));
                }
                effects.push(Effect::Delete {
                    id,
                    path: download.path,
                });
                self.progress.remove(&id);
            }
        }
        effects.extend(self.start_next());
        effects
    }

    fn status(&self, id: i64) -> Option<&DownloadStatus> {
        self.downloads
            .iter()
            .find(|d| d.id == id)
            .map(|d| &d.status)
    }

    fn running_mut(&mut self, id: i64) -> Option<&mut Download> {
        self.downloads
            .iter_mut()
            .find(|d| d.id == id && d.status == DownloadStatus::Running)
    }

    fn set_status(&mut self, id: i64, status: DownloadStatus) -> Option<Effect> {
        let download = self.downloads.iter_mut().find(|d| d.id == id)?;
        download.status = status.clone();
        Some(Effect::SetStatus(id, status))
    }

    /// Starts the oldest queued downloads while there are free slots.
    fn start_next(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        let mut running = self
            .downloads
            .iter()
            .filter(|d| d.status == DownloadStatus::Running)
            .count();
        while running < self.max_parallel {
            let Some(download) = self
                .downloads
                .iter_mut()
                .rev()
                .find(|d| d.status == DownloadStatus::Queued)
            else {
                break;
            };
            download.status = DownloadStatus::Running;
            effects.push(Effect::SetStatus(download.id, DownloadStatus::Running));
            effects.push(Effect::Start(download.clone()));
            running += 1;
        }
        effects
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;

    use super::*;

    fn video(id: &str, title: &str) -> Video {
        Video {
            id: id.into(),
            title: title.into(),
            channel_id: "UC1".into(),
            channel_name: "Chan".into(),
            published: None,
            duration_secs: None,
            thumbnail_url: None,
        }
    }

    fn download(id: i64, status: DownloadStatus) -> Download {
        Download {
            id,
            video: video(&format!("v{id}"), "t"),
            kind: DownloadKind::Video,
            path: format!("/d/{id}.mp4").into(),
            status,
            created_at: DateTime::from_timestamp(id, 0).unwrap(),
        }
    }

    fn started(effects: &[Effect]) -> Vec<i64> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Start(d) => Some(d.id),
                _ => None,
            })
            .collect()
    }

    fn status(queue: &Queue, id: i64) -> DownloadStatus {
        queue
            .downloads
            .iter()
            .find(|d| d.id == id)
            .unwrap()
            .status
            .clone()
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_stem(&video("abc", "Hello World")), "Hello World [abc]");
        assert_eq!(
            file_stem(&video("abc", "a/b\\c:d*e?f\"g<h>i|j")),
            "a_b_c_d_e_f_g_h_i_j [abc]"
        );
        assert_eq!(file_stem(&video("abc", "  .hidden. ")), "hidden [abc]");
        assert_eq!(file_stem(&video("abc", "")), "abc");
        let long = "x".repeat(300);
        assert_eq!(
            file_stem(&video("abc", &long)),
            format!("{} [abc]", "x".repeat(100))
        );
        assert_eq!(
            download_path(Path::new("/dl"), &video("abc", "T"), DownloadKind::Audio),
            PathBuf::from("/dl/T [abc].m4a")
        );
    }

    #[test]
    fn added_downloads_start_up_to_limit() {
        let mut queue = Queue::new(2);
        let a = queue.update(Msg::Add(download(1, DownloadStatus::Queued)));
        let b = queue.update(Msg::Add(download(2, DownloadStatus::Queued)));
        let c = queue.update(Msg::Add(download(3, DownloadStatus::Queued)));

        assert_eq!(
            a,
            [
                Effect::SetStatus(1, DownloadStatus::Running),
                Effect::Start(download(1, DownloadStatus::Running)),
            ]
        );
        assert_eq!(started(&b), [2]);
        assert!(c.is_empty());
        assert_eq!(status(&queue, 3), DownloadStatus::Queued);
        let ids: Vec<_> = queue.downloads.iter().map(|d| d.id).collect();
        assert_eq!(ids, [3, 2, 1], "newest first");
    }

    #[test]
    fn loading_starts_oldest_queued_first() {
        let mut queue = Queue::new(1);
        let effects = queue.update(Msg::Load(vec![
            download(3, DownloadStatus::Queued),
            download(2, DownloadStatus::Completed),
            download(1, DownloadStatus::Queued),
        ]));
        assert_eq!(started(&effects), [1]);
    }

    #[test]
    fn finishing_starts_next_and_tracks_progress() {
        let mut queue = Queue::new(1);
        queue.update(Msg::Add(download(1, DownloadStatus::Queued)));
        queue.update(Msg::Add(download(2, DownloadStatus::Queued)));
        queue.update(Msg::Progress {
            id: 1,
            fraction: 0.5,
        });
        assert_eq!(queue.progress.get(&1), Some(&0.5));

        let effects = queue.update(Msg::Finished {
            id: 1,
            result: Ok("/d/1.mkv".into()),
        });

        assert_eq!(
            effects[0],
            Effect::Complete {
                id: 1,
                path: "/d/1.mkv".into()
            }
        );
        assert_eq!(started(&effects), [2]);
        assert_eq!(status(&queue, 1), DownloadStatus::Completed);
        assert_eq!(
            queue.downloads.iter().find(|d| d.id == 1).unwrap().path,
            PathBuf::from("/d/1.mkv")
        );
        assert_eq!(queue.progress.get(&1), None);
    }

    #[test]
    fn failures_are_recorded_and_can_be_retried() {
        let mut queue = Queue::new(1);
        queue.update(Msg::Add(download(1, DownloadStatus::Queued)));

        let effects = queue.update(Msg::Finished {
            id: 1,
            result: Err("403".into()),
        });
        let failed = DownloadStatus::Failed("403".into());
        assert_eq!(effects, [Effect::SetStatus(1, failed.clone())]);
        assert_eq!(status(&queue, 1), failed);

        assert_eq!(started(&queue.update(Msg::Retry(1))), [1]);
        assert_eq!(status(&queue, 1), DownloadStatus::Running);
    }

    #[test]
    fn cancelling_running_download_aborts_it() {
        let mut queue = Queue::new(1);
        queue.update(Msg::Add(download(1, DownloadStatus::Queued)));
        queue.update(Msg::Add(download(2, DownloadStatus::Queued)));

        let effects = queue.update(Msg::Cancel(1));

        assert_eq!(
            effects[..2],
            [
                Effect::Abort(1),
                Effect::SetStatus(1, DownloadStatus::Cancelled)
            ]
        );
        assert_eq!(started(&effects), [2]);
        // A late result of the aborted task is ignored.
        assert!(
            queue
                .update(Msg::Finished {
                    id: 1,
                    result: Ok("/x".into())
                })
                .is_empty()
        );
        assert_eq!(status(&queue, 1), DownloadStatus::Cancelled);
    }

    #[test]
    fn cancelling_queued_download() {
        let mut queue = Queue::new(1);
        queue.update(Msg::Add(download(1, DownloadStatus::Queued)));
        queue.update(Msg::Add(download(2, DownloadStatus::Queued)));
        assert_eq!(
            queue.update(Msg::Cancel(2)),
            [Effect::SetStatus(2, DownloadStatus::Cancelled)]
        );
        assert!(queue.update(Msg::Cancel(2)).is_empty(), "already cancelled");
    }

    #[test]
    fn removing_deletes_file_and_aborts_running() {
        let mut queue = Queue::new(1);
        queue.update(Msg::Add(download(1, DownloadStatus::Queued)));
        queue.update(Msg::Add(download(2, DownloadStatus::Queued)));

        let effects = queue.update(Msg::Remove(1));

        assert_eq!(
            effects[..2],
            [
                Effect::Abort(1),
                Effect::Delete {
                    id: 1,
                    path: "/d/1.mp4".into()
                }
            ]
        );
        assert_eq!(started(&effects), [2]);
        assert!(queue.downloads.iter().all(|d| d.id != 1));
        assert!(queue.update(Msg::Remove(1)).is_empty());
    }
}
