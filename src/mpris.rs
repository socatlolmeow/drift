/* System media controls: MPRIS on Linux, the Now Playing center on macOS and
the System Media Transport Controls on Windows (all through souvlaki). */

#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", windows)),
    allow(dead_code)
)]
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

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
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
        /* declared first so it is dropped before the window it is bound to */
        controls: MediaControls,
        #[cfg(windows)]
        _window: super::hidden_window::HiddenWindow,
        track: Option<String>,
        state: PlayState,
        secs: u64,
    }

    impl Mpris {
        /* None when the platform has no usable media session (on Linux: no
        session bus). */
        pub fn new(send: impl Fn(Cmd) + Send + 'static) -> Option<Mpris> {
            let name = format!("drift.instance{}", std::process::id());

            /* Windows binds the controls to a window; a terminal app has none
            of its own, so give it an invisible one. */
            #[cfg(windows)]
            let window = super::hidden_window::HiddenWindow::new()?;
            #[cfg(windows)]
            let hwnd = Some(window.handle());
            #[cfg(not(windows))]
            let hwnd = None;

            let config = PlatformConfig {
                dbus_name: &name,
                display_name: "drift",
                hwnd,
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
                #[cfg(windows)]
                _window: window,
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

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
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

/* An invisible top-level window on its own thread. Windows only hands out
media controls for a window, and the thread keeps the window alive and
pumping messages for as long as the controls exist. Plain FFI, so no extra
crates are needed. */
#[cfg(windows)]
mod hidden_window {
    use std::ffi::c_void;
    use std::mem::zeroed;
    use std::ptr::{null, null_mut};
    use std::sync::mpsc;
    use std::thread::{self, JoinHandle};

    type Hwnd = *mut c_void;

    const WM_DESTROY: u32 = 0x0002;
    const WM_CLOSE: u32 = 0x0010;

    type WndProc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;

    #[repr(C)]
    struct WndClassW {
        style: u32,
        wnd_proc: Option<WndProc>,
        cls_extra: i32,
        wnd_extra: i32,
        instance: *mut c_void,
        icon: *mut c_void,
        cursor: *mut c_void,
        background: *mut c_void,
        menu_name: *const u16,
        class_name: *const u16,
    }

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    struct Msg {
        hwnd: Hwnd,
        message: u32,
        w_param: usize,
        l_param: isize,
        time: u32,
        pt: Point,
    }

    #[link(name = "user32")]
    extern "system" {
        fn RegisterClassW(class: *const WndClassW) -> u16;
        #[allow(clippy::too_many_arguments)]
        fn CreateWindowExW(
            ex_style: u32,
            class_name: *const u16,
            window_name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            parent: Hwnd,
            menu: *mut c_void,
            instance: *mut c_void,
            param: *mut c_void,
        ) -> Hwnd;
        fn DefWindowProcW(hwnd: Hwnd, msg: u32, w_param: usize, l_param: isize) -> isize;
        fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
        fn TranslateMessage(msg: *const Msg) -> i32;
        fn DispatchMessageW(msg: *const Msg) -> isize;
        fn PostMessageW(hwnd: Hwnd, msg: u32, w_param: usize, l_param: isize) -> i32;
        fn PostQuitMessage(code: i32);
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    }

    unsafe extern "system" fn wnd_proc(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize {
        if msg == WM_DESTROY {
            PostQuitMessage(0);
            return 0;
        }
        DefWindowProcW(hwnd, msg, w, l)
    }

    pub struct HiddenWindow {
        hwnd: usize,
        thread: Option<JoinHandle<()>>,
    }

    impl HiddenWindow {
        pub fn new() -> Option<HiddenWindow> {
            let (tx, rx) = mpsc::channel::<usize>();
            let thread = thread::Builder::new()
                .name("media-window".into())
                .spawn(move || unsafe {
                    let class: Vec<u16> = "drift_media_controls\0".encode_utf16().collect();
                    let instance = GetModuleHandleW(null());
                    let wc = WndClassW {
                        style: 0,
                        wnd_proc: Some(wnd_proc),
                        cls_extra: 0,
                        wnd_extra: 0,
                        instance,
                        icon: null_mut(),
                        cursor: null_mut(),
                        background: null_mut(),
                        menu_name: null(),
                        class_name: class.as_ptr(),
                    };
                    /* fails harmlessly if the class already exists */
                    RegisterClassW(&wc);
                    /* never shown: no WS_VISIBLE and no ShowWindow call */
                    let hwnd = CreateWindowExW(
                        0,
                        class.as_ptr(),
                        class.as_ptr(),
                        0,
                        0,
                        0,
                        0,
                        0,
                        null_mut(),
                        null_mut(),
                        instance,
                        null_mut(),
                    );
                    let _ = tx.send(hwnd as usize);
                    if hwnd.is_null() {
                        return;
                    }
                    let mut msg: Msg = zeroed();
                    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                })
                .ok()?;
            match rx.recv() {
                Ok(hwnd) if hwnd != 0 => Some(HiddenWindow {
                    hwnd,
                    thread: Some(thread),
                }),
                _ => None,
            }
        }

        pub fn handle(&self) -> *mut c_void {
            self.hwnd as *mut c_void
        }
    }

    impl Drop for HiddenWindow {
        fn drop(&mut self) {
            unsafe {
                PostMessageW(self.hwnd as Hwnd, WM_CLOSE, 0, 0);
            }
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }
}

/* macOS only delivers media key events while the main run loop is turned, and
drift's own loop never does that. Call this once per iteration of the main
loop on the main thread; it returns immediately. */
#[cfg(target_os = "macos")]
pub fn pump() {
    use std::ffi::c_void;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(
            mode: *const c_void,
            seconds: f64,
            return_after_source_handled: u8,
        ) -> i32;
    }

    unsafe {
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.0, 0);
    }
}

#[cfg(not(target_os = "macos"))]
#[inline]
pub fn pump() {}

pub use imp::Mpris;
