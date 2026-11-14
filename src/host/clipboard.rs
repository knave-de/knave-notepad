use crate::AppResult;
#[cfg(all(unix, not(target_os = "macos")))]
use winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use winit::window::Window;

pub(crate) enum Clipboard {
    #[cfg(all(unix, not(target_os = "macos")))]
    Wayland(smithay_clipboard::Clipboard),
    Native(Option<arboard::Clipboard>),
}
impl Clipboard {
    pub(crate) fn new(window: &Window) -> AppResult<Self> {
        #[cfg(all(unix, not(target_os = "macos")))]
        if let RawDisplayHandle::Wayland(display) = window.display_handle()?.as_raw() {
            // Gpu owns the window and drops this clipboard first. The borrowed
            // display stays alive until the clipboard worker has joined.
            return Ok(Self::Wayland(unsafe {
                smithay_clipboard::Clipboard::new(display.display.as_ptr())
            }));
        }
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        let _ = window;
        Ok(Self::Native(None))
    }
    fn native(slot: &mut Option<arboard::Clipboard>) -> AppResult<&mut arboard::Clipboard> {
        if slot.is_none() {
            *slot = Some(arboard::Clipboard::new()?);
        }
        Ok(slot.as_mut().expect("clipboard initialized"))
    }
    pub(crate) fn store(&mut self, text: String) -> AppResult {
        match self {
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::Wayland(clipboard) => clipboard.store(text),
            Self::Native(slot) => Self::native(slot)?.set_text(text)?,
        }
        Ok(())
    }
    pub(crate) fn load(&mut self) -> AppResult<String> {
        Ok(match self {
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::Wayland(clipboard) => clipboard.load()?,
            Self::Native(slot) => Self::native(slot)?.get_text()?,
        })
    }
}
