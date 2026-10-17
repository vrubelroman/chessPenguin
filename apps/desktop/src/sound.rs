//! Sound playback on a dedicated thread; missing audio devices are tolerated silently.

use std::io::Cursor;
use std::sync::mpsc::{Sender, channel};

use crate::assets::{Sound, SoundSet, sound_bytes};

pub struct SoundPlayer {
    tx: Option<Sender<(SoundSet, Sound, f32)>>,
}

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
