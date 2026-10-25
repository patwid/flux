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
        // Decoded here, off the main thread: GDK has no WebP loader of its own.
        let image = bytes.map(|bytes| match decode_webp(&bytes) {
            Some(pixels) => Image::Pixels(pixels),
            None => Image::Encoded(bytes.to_vec()),
        });
        let _ = tx.send(image);
    });
    glib::spawn_future_local(async move {
        let Ok(Some(image)) = rx.await else { return };
        let Some(texture) = image.into_texture() else {
            return;
        };
        CACHE.with_borrow_mut(|c| c.insert(url, texture.clone()));
        apply(&texture);
    });
}

/// A fetched image, ready to become a texture on the main thread.
enum Image {
    /// In a format GDK decodes itself (PNG, JPEG, ...).
    Encoded(Vec<u8>),
    Pixels(Pixels),
}

impl Image {
    fn into_texture(self) -> Option<gdk::Texture> {
        match self {
            Image::Encoded(bytes) => gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)).ok(),
            Image::Pixels(p) => {
                let format = if p.has_alpha {
                    gdk::MemoryFormat::R8g8b8a8
                } else {
                    gdk::MemoryFormat::R8g8b8
                };
                let stride = p.width as usize * if p.has_alpha { 4 } else { 3 };
                let texture = gdk::MemoryTexture::new(
                    p.width as i32,
                    p.height as i32,
                    format,
                    &glib::Bytes::from_owned(p.data),
                    stride,
                );
                Some(texture.upcast())
            }
        }
    }
}

/// Decoded pixels, RGB or RGBA with rows packed tightly.
#[derive(Debug)]
struct Pixels {
    width: u32,
    height: u32,
    has_alpha: bool,
    data: Vec<u8>,
}

/// Decodes `bytes` if they are a WebP image, which YouTube serves for its
/// signed thumbnail URLs (even those ending in `.jpg`).
fn decode_webp(bytes: &[u8]) -> Option<Pixels> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return None;
    }
    let mut decoder = image_webp::WebPDecoder::new(std::io::Cursor::new(bytes)).ok()?;
    let (width, height) = decoder.dimensions();
    let mut data = vec![0; decoder.output_buffer_size()?];
    decoder.read_image(&mut data).ok()?;
    Some(Pixels {
        width,
        height,
        has_alpha: decoder.has_alpha(),
        data,
    })
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
    fn decodes_webp() {
        let rgb = [255, 0, 0, 0, 0, 255];
        let mut webp = Vec::new();
        image_webp::WebPEncoder::new(&mut webp)
            .encode(&rgb, 2, 1, image_webp::ColorType::Rgb8)
            .unwrap();

        let pixels = decode_webp(&webp).unwrap();
        assert_eq!((pixels.width, pixels.height), (2, 1));
        assert_eq!(pixels.data, rgb);
    }

    #[test]
    fn leaves_other_formats_to_gdk() {
        assert!(decode_webp(b"\xff\xd8\xff\xe0 not a webp").is_none());
    }

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
