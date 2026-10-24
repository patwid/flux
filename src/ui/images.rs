//! Loading and caching remote images (thumbnails, avatars).

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

use relm4::adw::prelude::*;
use relm4::gtk::{gdk, glib};
use relm4::{adw, gtk};

/// Number of decoded images kept in memory.
const CACHE_SIZE: usize = 300;

thread_local! {
    static CACHE: RefCell<Cache<String, gdk::Texture>> = RefCell::new(Cache::new(CACHE_SIZE));
}

/// Loads the image at `url` (cached) and passes it to `apply` on the main thread.
pub fn load(url: &str, apply: impl FnOnce(&gdk::Texture) + 'static) {
    if let Some(texture) = CACHE.with_borrow(|c| c.get(&url.to_owned())) {
        apply(&texture);
        return;
    }
    let url = url.to_owned();
    let (tx, rx) = futures::channel::oneshot::channel();
    let fetch_url = url.clone();
    // reqwest needs the tokio runtime that relm4 runs commands on.
    relm4::spawn(async move {
        let bytes = match url::Url::parse(&fetch_url) {
            Ok(u) if u.scheme() == "file" => u
                .to_file_path()
                .ok()
                .and_then(|path| std::fs::read(path).ok())
                .map(Into::into),
            _ => async {
                reqwest::get(&fetch_url)
                    .await?
                    .error_for_status()?
                    .bytes()
                    .await
            }
            .await
            .ok(),
        };
        let _ = tx.send(bytes);
    });
    glib::spawn_future_local(async move {
        let Ok(Some(bytes)) = rx.await else { return };
        let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes.to_vec())) else {
            return;
        };
        CACHE.with_borrow_mut(|c| c.insert(url, texture.clone()));
        apply(&texture);
    });
}

/// Shows the image at `url` in `picture` once loaded.
pub fn set_picture(picture: &gtk::Picture, url: Option<&str>) {
    let Some(url) = url else { return };
    let weak = picture.downgrade();
    load(url, move |texture| {
        if let Some(picture) = weak.upgrade() {
            picture.set_paintable(Some(texture));
        }
    });
}

/// Shows the image at `url` in `avatar` once loaded.
pub fn set_avatar(avatar: &adw::Avatar, url: Option<&str>) {
    let Some(url) = url else { return };
    let weak = avatar.downgrade();
    load(url, move |texture| {
        if let Some(avatar) = weak.upgrade() {
            avatar.set_custom_image(Some(texture));
        }
    });
}

/// A cache keeping the most recently inserted `capacity` entries.
#[derive(Debug)]
pub struct Cache<K, V> {
    capacity: usize,
    entries: HashMap<K, V>,
    order: VecDeque<K>,
}

impl<K: Clone + Eq + Hash, V: Clone> Cache<K, V> {
    pub fn new(capacity: usize) -> Self {
        Cache {
            capacity,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub fn get(&self, key: &K) -> Option<V> {
        self.entries.get(key).cloned()
    }

    pub fn insert(&mut self, key: K, value: V) {
        if self.entries.insert(key.clone(), value).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_oldest_entries() {
        let mut cache = Cache::new(2);
        cache.insert("a", 1);
        cache.insert("b", 2);
        assert_eq!(cache.get(&"a"), Some(1));

        cache.insert("c", 3);
        assert_eq!(cache.get(&"a"), None);
        assert_eq!(cache.get(&"b"), Some(2));
        assert_eq!(cache.get(&"c"), Some(3));
    }

    #[test]
    fn reinserting_does_not_duplicate() {
        let mut cache = Cache::new(2);
        cache.insert("a", 1);
        cache.insert("a", 2);
        cache.insert("b", 3);
        assert_eq!(cache.get(&"a"), Some(2));
        assert_eq!(cache.get(&"b"), Some(3));
    }
}
