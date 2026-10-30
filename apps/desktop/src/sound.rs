//! Sound playback: on a computer on a dedicated thread (missing audio devices are
//! tolerated silently); in a browser by its audio elements, each sound's file made once.

#[cfg(not(web))]
use std::io::Cursor;
#[cfg(not(web))]
use std::sync::mpsc::{Sender, channel};

use crate::assets::{Sound, SoundSet, sound_bytes};

#[cfg(not(web))]
pub struct SoundPlayer {
    tx: Option<Sender<(SoundSet, Sound, f32)>>,
}

#[cfg(not(web))]
impl SoundPlayer {
    pub fn new() -> Self {
        let (tx, rx) = channel::<(SoundSet, Sound, f32)>();
        let spawned = std::thread::Builder::new()
            .name("sound".into())
            .spawn(move || {
                let sink = match rodio::DeviceSinkBuilder::open_default_sink() {
                    Ok(mut s) => {
                        s.log_on_drop(false);
                        s
                    }
                    Err(e) => {
                        eprintln!("sound disabled: {e}");
                        // Drain so senders never block; nothing is played.
                        for _ in rx {}
                        return;
                    }
                };
                for (set, sound, volume) in rx {
                    let Ok(decoder) =
                        rodio::Decoder::try_from(Cursor::new(sound_bytes(set, sound)))
                    else {
                        continue;
                    };
                    let player = rodio::Player::connect_new(sink.mixer());
                    player.set_volume(volume);
                    player.append(decoder);
                    player.detach();
                }
            });
        Self {
            tx: spawned.ok().map(|_| tx),
        }
    }

    pub fn play(&self, set: SoundSet, sound: Sound, volume: f32) {
        if let Some(tx) = &self.tx {
            let _ = tx.send((set, sound, volume));
        }
    }
}

/// The sounds' files for the page's audio elements (`blob:` addresses), by the sound's
/// bytes (each sound is a file of its own).
#[cfg(web)]
pub struct SoundPlayer {
    urls: std::cell::RefCell<std::collections::HashMap<usize, String>>,
}

#[cfg(web)]
impl SoundPlayer {
    pub fn new() -> Self {
        Self {
            urls: Default::default(),
        }
    }

    fn url(&self, bytes: &'static [u8]) -> Option<String> {
        let key = bytes.as_ptr() as usize;
        if let Some(u) = self.urls.borrow().get(&key) {
            return Some(u.clone());
        }
        let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
        let opts = web_sys::BlobPropertyBag::new();
        opts.set_type("audio/ogg");
        let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).ok()?;
        let url = web_sys::Url::create_object_url_with_blob(&blob).ok()?;
        self.urls.borrow_mut().insert(key, url.clone());
        Some(url)
    }

    /// Plays the sound; a browser that cannot (no audio, not allowed yet) stays silent.
    pub fn play(&self, set: SoundSet, sound: Sound, volume: f32) {
        let Some(url) = self.url(sound_bytes(set, sound)) else {
            return;
        };
        if let Ok(audio) = web_sys::HtmlAudioElement::new_with_src(&url) {
            audio.set_volume(f64::from(volume.clamp(0.0, 1.0)));
            // The promise fails when the page may not play yet: nothing to do then.
            let _ = audio.play();
        }
    }
}
