/* MPRIS (media keys, playerctl, waybar...) on Linux; a no-op elsewhere. */

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cmd {
    Play,
    Pause,
    Toggle,
    Stop,
    Next,
    Prev,
    SeekBy(f64),
    SeekTo(f64),
    Quit,
}

#[cfg(target_os = "linux")]
mod imp {
    use std::time::Duration;

    use souvlaki::{
        MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition,
        PlatformConfig, SeekDirection,
    };

    use super::Cmd;
    use crate::app::PlayState;
    use crate::model::Track;

    pub struct Mpris {
        controls: MediaControls,
        track: Option<String>,
        state: PlayState,
        secs: u64,
    }

    impl Mpris {
        /* None when there is no session bus. */
        pub fn new(send: impl Fn(Cmd) + Send + 'static) -> Option<Mpris> {
            let name = format!("drift.instance{}", std::process::id());
            let config = PlatformConfig {
                dbus_name: &name,
                display_name: "drift",
                hwnd: None,
            };
            let mut controls = MediaControls::new(config).ok()?;
            controls
                .attach(move |ev| {
                    if let Some(c) = convert(ev) {
                        send(c);
                    }
                })
                .ok()?;
            Some(Mpris {
                controls,
                track: None,
                state: PlayState::Stopped,
                secs: 0,
            })
        }

        pub fn update(&mut self, track: Option<&Track>, state: PlayState, position: f64) {
            let id = track.map(|t| t.id.as_str());
            if id != self.track.as_deref() {
                self.track = id.map(str::to_string);
                let cover = track.map(|t| format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", t.id));
                let meta = match track {
                    Some(t) => MediaMetadata {
                        title: Some(t.title.as_str()),
                        album: t.album.as_deref(),
                        artist: Some(t.artist.as_str()),
                        cover_url: cover.as_deref(),
                        duration: t.duration.map(|d| Duration::from_secs(u64::from(d))),
                    },
                    None => MediaMetadata::default(),
                };
                let _ = self.controls.set_metadata(meta);
                self.secs = u64::MAX;
            }
            let pos = if position.is_finite() {
                position.max(0.0)
            } else {
                0.0
            };
            let secs = pos as u64;
            if state == self.state && secs == self.secs {
                return;
            }
            self.state = state;
            self.secs = secs;
            let progress = Duration::try_from_secs_f64(pos).ok().map(MediaPosition);
            let _ = self.controls.set_playback(match state {
                PlayState::Playing | PlayState::Loading => MediaPlayback::Playing { progress },
                PlayState::Paused => MediaPlayback::Paused { progress },
                PlayState::Stopped => MediaPlayback::Stopped,
            });
        }
    }

    fn dir(d: SeekDirection, secs: f64) -> f64 {
        if d == SeekDirection::Backward {
            -secs
        } else {
            secs
        }
    }

    pub(super) fn convert(ev: MediaControlEvent) -> Option<Cmd> {
        Some(match ev {
            MediaControlEvent::Play => Cmd::Play,
            MediaControlEvent::Pause => Cmd::Pause,
            MediaControlEvent::Toggle => Cmd::Toggle,
            MediaControlEvent::Next => Cmd::Next,
            MediaControlEvent::Previous => Cmd::Prev,
            MediaControlEvent::Stop => Cmd::Stop,
            MediaControlEvent::Seek(d) => Cmd::SeekBy(dir(d, 5.0)),
            MediaControlEvent::SeekBy(d, by) => Cmd::SeekBy(dir(d, by.as_secs_f64())),
            MediaControlEvent::SetPosition(MediaPosition(p)) => Cmd::SeekTo(p.as_secs_f64()),
            MediaControlEvent::Quit => Cmd::Quit,
            _ => return None,
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn maps_events() {
            assert_eq!(convert(MediaControlEvent::Previous), Some(Cmd::Prev));
            assert_eq!(
                convert(MediaControlEvent::SeekBy(
                    SeekDirection::Backward,
                    Duration::from_secs(10)
                )),
                Some(Cmd::SeekBy(-10.0))
            );
            assert_eq!(
                convert(MediaControlEvent::SetPosition(MediaPosition(
                    Duration::from_secs(90)
                ))),
                Some(Cmd::SeekTo(90.0))
            );
            assert_eq!(convert(MediaControlEvent::Raise), None);
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::Cmd;
    use crate::app::PlayState;
    use crate::model::Track;

    pub struct Mpris;

    impl Mpris {
        pub fn new(_send: impl Fn(Cmd) + Send + 'static) -> Option<Mpris> {
            None
        }

        pub fn update(&mut self, _track: Option<&Track>, _state: PlayState, _position: f64) {}
    }
}

pub use imp::Mpris;
