//! Downloads page: runs the download queue and lists downloads.

use std::collections::HashMap;

use relm4::adw::prelude::*;
use relm4::{Component, ComponentParts, ComponentSender, RelmWidgetExt, adw, gtk};

use super::{Services, format, log_db_error, widgets};
use crate::core::download::{self, Effect, MAX_PARALLEL, Msg, Queue};
use crate::core::model::{
    Download, DownloadKind, DownloadStatus, SavedDownload, SubtitleTrack, Video,
};

pub struct DownloadsPage {
    queue: Queue,
    services: Services,
    list: gtk::ListBox,
    /// Running download tasks, for cancelling.
    tasks: HashMap<i64, relm4::JoinHandle<()>>,
    /// Rows of the listed downloads, to update progress in place.
    rows: HashMap<i64, adw::ActionRow>,
    /// Subtitles saved by finished downloads, until they are completed.
    saved_subtitles: HashMap<i64, Vec<SubtitleTrack>>,
}

#[derive(Debug)]
pub enum Input {
    Queue(Msg),
    /// A download task finished.
    Finished {
        id: i64,
        result: Result<SavedDownload, String>,
    },
    Add(Video, DownloadKind),
    Activate(usize),
}

#[derive(Debug)]
pub enum Output {
    OpenVideo(Video),
    /// A download finished.
    Completed(Video),
    /// A download failed; it can be retried with [`Msg::Retry`].
    Failed {
        id: i64,
        video: Video,
    },
}

/// What to report about a download that just finished, if anything.
pub fn finished(download: &Download) -> Option<Output> {
    match download.status {
        DownloadStatus::Completed => Some(Output::Completed(download.video.clone())),
        DownloadStatus::Failed(_) => Some(Output::Failed {
            id: download.id,
            video: download.video.clone(),
        }),
        _ => None,
    }
}

#[relm4::component(pub)]
impl Component for DownloadsPage {
    type Init = Services;
    type Input = Input;
    type Output = Output;
    type CommandOutput = ();

    view! {
        adw::ToolbarView {
            add_top_bar = &adw::HeaderBar {
                pack_end: &widgets::primary_menu(),
                #[wrap(Some)]
                set_title_widget = &adw::WindowTitle {
                    set_title: "Downloads",
                },
            },

            #[wrap(Some)]
            set_content = &gtk::Stack {
                add_named[Some("empty")] = &adw::StatusPage {
                    set_icon_name: Some("folder-download-symbolic"),
                    set_title: "No Downloads",
                    set_description: Some("Download videos to watch them offline"),
                },
                add_named[Some("list")] = &gtk::ScrolledWindow {
                    set_hscrollbar_policy: gtk::PolicyType::Never,

                    adw::Clamp {
                        set_maximum_size: 800,
                        set_margin_all: 12,

                        #[local_ref]
                        list -> gtk::ListBox {
                            connect_row_activated[sender] => move |_, row| {
                                sender.input(Input::Activate(row.index() as usize));
                            },
                        },
                    },
                },

                #[watch]
                set_visible_child_name: if model.queue.downloads.is_empty() { "empty" } else { "list" },
            },
        }
    }

    fn init(
        services: Services,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let downloads = log_db_error(services.db.downloads()).unwrap_or_default();
        let model = DownloadsPage {
            queue: Queue::new(MAX_PARALLEL),
            services,
            list: widgets::card_list(),
            tasks: HashMap::new(),
            rows: HashMap::new(),
            saved_subtitles: HashMap::new(),
        };
        let list = &model.list;
        let widgets = view_output!();
        sender.input(Input::Queue(Msg::Load(downloads)));
        ComponentParts { model, widgets }
    }

    fn update(&mut self, input: Input, sender: ComponentSender<Self>, _root: &Self::Root) {
        let mut finished_id = None;
        let msg = match input {
            Input::Queue(msg) => msg,
            Input::Finished { id, result } => {
                let result = result.map(|saved| {
                    self.saved_subtitles.insert(id, saved.subtitles);
                    saved.path
                });
                finished_id = Some(id);
                Msg::Finished { id, result }
            }
            Input::Add(video, kind) => {
                let dir = &self.services.download_dir;
                if let Err(e) = std::fs::create_dir_all(dir) {
                    relm4::gtk::glib::g_warning!("flux", "cannot create {}: {e}", dir.display());
                }
                let path = download::download_path(dir, &video, kind);
                let created_at = chrono::Utc::now();
                let Some(id) = log_db_error(
                    self.services
                        .db
                        .add_download(&video, kind, &path, created_at),
                ) else {
                    return;
                };
                Msg::Add(Download {
                    id,
                    video,
                    kind,
                    path,
                    status: DownloadStatus::Queued,
                    created_at,
                })
            }
            Input::Activate(index) => {
                if let Some(d) = self.queue.downloads.get(index)
                    && d.status == DownloadStatus::Completed
                {
                    let _ = sender.output(Output::OpenVideo(d.video.clone()));
                }
                return;
            }
        };

        if let Msg::Progress { id, .. } = msg {
            self.queue.update(msg);
            self.update_progress(id);
            return;
        }
        for effect in self.queue.update(msg) {
            self.run(effect, &sender);
        }
        self.fill(&sender);
        if let Some(output) = finished_id
            .and_then(|id| self.queue.downloads.iter().find(|d| d.id == id))
            .and_then(finished)
        {
            let _ = sender.output(output);
        }
    }
}

impl DownloadsPage {
    fn run(&mut self, effect: Effect, sender: &ComponentSender<Self>) {
        let db = &self.services.db;
        match effect {
            Effect::Start(download) => {
                let downloader = self.services.downloader.clone();
                let input = sender.input_sender().clone();
                let id = download.id;
                let task = relm4::spawn(async move {
                    let progress = input.clone();
                    let result = downloader
                        .download(
                            &download.video.id,
                            download.kind,
                            &download.path,
                            Box::new(move |fraction| {
                                let _ = progress.send(Input::Queue(Msg::Progress { id, fraction }));
                            }),
                        )
                        .await
                        .map_err(|e| e.to_string());
                    let _ = input.send(Input::Finished { id, result });
                });
                self.tasks.insert(id, task);
            }
            Effect::Abort(id) => {
                if let Some(task) = self.tasks.remove(&id) {
                    task.abort();
                }
            }
            Effect::SetStatus(id, status) => {
                log_db_error(db.set_download_status(id, &status));
            }
            Effect::Complete { id, path } => {
                self.tasks.remove(&id);
                log_db_error(db.complete_download(id, &path));
                let subtitles = self.saved_subtitles.remove(&id).unwrap_or_default();
                log_db_error(db.set_download_subtitles(id, &subtitles));
            }
            Effect::Delete { id, path } => {
                self.saved_subtitles.remove(&id);
                let subtitles = log_db_error(db.download_subtitles(id)).unwrap_or_default();
                for track in subtitles {
                    if let Some(file) = url::Url::parse(&track.url)
                        .ok()
                        .and_then(|u| u.to_file_path().ok())
                    {
                        let _ = std::fs::remove_file(file);
                    }
                }
                log_db_error(db.remove_download(id));
                // The file may not exist yet (queued) or anymore.
                let _ = std::fs::remove_file(&path);
                // Parts of an unfinished download, kept for resuming it.
                let parts = path.parent().and_then(|dir| std::fs::read_dir(dir).ok());
                for file in parts.into_iter().flatten().flatten() {
                    if download::is_part_of(&path, &file.path()) {
                        let _ = std::fs::remove_file(file.path());
                    }
                }
            }
        }
    }

    fn fill(&mut self, sender: &ComponentSender<Self>) {
        self.list.remove_all();
        self.rows.clear();
        for download in &self.queue.downloads {
            let row = widgets::video_row(&download.video);
            let id = download.id;
            let button = |icon: &str, tooltip: &str, msg: fn(i64) -> Msg| {
                let button = gtk::Button::builder()
                    .icon_name(icon)
                    .tooltip_text(tooltip)
                    .valign(gtk::Align::Center)
                    .css_classes(["flat"])
                    .build();
                let input = sender.input_sender().clone();
                button.connect_clicked(move |_| {
                    let _ = input.send(Input::Queue(msg(id)));
                });
                button
            };
            match download.status {
                DownloadStatus::Queued | DownloadStatus::Running => {
                    row.add_suffix(&button("process-stop-symbolic", "Cancel", Msg::Cancel));
                }
                DownloadStatus::Failed(_) | DownloadStatus::Cancelled => {
                    row.add_suffix(&button("view-refresh-symbolic", "Retry", Msg::Retry));
                    row.add_suffix(&button("user-trash-symbolic", "Remove", Msg::Remove));
                }
                DownloadStatus::Completed => {
                    row.add_suffix(&button("user-trash-symbolic", "Delete", Msg::Remove));
                }
            }
            row.set_activatable(download.status == DownloadStatus::Completed);
            self.list.append(&row);
            self.rows.insert(id, row);
            self.update_progress(id);
        }
    }

    fn update_progress(&self, id: i64) {
        let (Some(row), Some(download)) = (
            self.rows.get(&id),
            self.queue.downloads.iter().find(|d| d.id == id),
        ) else {
            return;
        };
        let progress = self.queue.progress.get(&id).copied();
        row.set_subtitle(&format::download_status(download, progress));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn download(status: DownloadStatus) -> Download {
        Download {
            id: 7,
            video: Video {
                id: "v".into(),
                title: "t".into(),
                channel_id: "UC1".into(),
                channel_name: "Chan".into(),
                published: None,
                duration_secs: None,
                thumbnail_url: None,
            },
            kind: DownloadKind::Video,
            path: "/tmp/t.webm".into(),
            status,
            created_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        }
    }

    #[test]
    fn finished_downloads_are_reported() {
        let video = download(DownloadStatus::Completed).video;
        assert!(matches!(
            finished(&download(DownloadStatus::Completed)),
            Some(Output::Completed(v)) if v == video
        ));
        assert!(matches!(
            finished(&download(DownloadStatus::Failed("offline".into()))),
            Some(Output::Failed { id: 7, video: v }) if v == video
        ));
        assert!(finished(&download(DownloadStatus::Running)).is_none());
    }
}
