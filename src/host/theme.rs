//! Optional, application-supplied theme selection and live file notifications.
use knave_ui::toolkit::{Scene, ThemePolicy};
use std::path::PathBuf;
#[cfg(target_os = "linux")]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
#[cfg(target_os = "linux")]
use winit::event_loop::EventLoopProxy;

/// A resolved theme policy and the application stylesheet to apply to a scene.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemeSelection {
    pub policy: ThemePolicy,
    pub stylesheet: String,
}

/// Supplies theme state and the files whose changes should reload that state.
///
/// This keeps desktop configuration parsing in the application that owns it.
/// Calls to `load` happen on the event-loop thread; `watch_paths` also runs on
/// the theme watcher thread and must be safe to call concurrently.
pub trait ThemeProvider: Send + Sync + 'static {
    fn load(&self) -> Result<ThemeSelection, String>;
    fn watch_paths(&self) -> Vec<PathBuf>;
}

pub fn apply(
    scene: &mut Scene,
    selection: &ThemeSelection,
) -> Result<bool, Box<dyn std::error::Error>> {
    let changed = scene.theme_policy() != selection.policy
        || scene.user_theme_stylesheet() != selection.stylesheet;
    if changed {
        scene.set_theme_policy_stylesheets(selection.policy, "", &selection.stylesheet)?;
    }
    Ok(changed)
}

/// Keep the selected palette if a provider's custom stylesheet is invalid.
pub fn apply_fallback(
    scene: &mut Scene,
    selection: &ThemeSelection,
) -> Result<(), Box<dyn std::error::Error>> {
    scene.set_theme_policy_stylesheets(selection.policy, "", "")?;
    Ok(())
}

#[cfg(target_os = "linux")]
pub struct Watcher {
    _watcher: knave_ui::toolkit::theme_file::FileWatcher,
    pub pending: Arc<AtomicBool>,
}
#[cfg(target_os = "linux")]
impl Watcher {
    pub fn start(
        provider: Arc<dyn ThemeProvider>,
        proxy: EventLoopProxy<()>,
    ) -> Result<Self, String> {
        let pending = Arc::new(AtomicBool::new(false));
        let signal = pending.clone();
        let paths_provider = provider.clone();
        let watcher = knave_ui::toolkit::theme_file::FileWatcher::start(
            move || paths_provider.watch_paths(),
            move || {
                if !signal.swap(true, Ordering::AcqRel) && proxy.send_event(()).is_err() {
                    signal.store(false, Ordering::Release);
                }
            },
        )?;
        Ok(Self {
            _watcher: watcher,
            pending,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use knave_ui::toolkit::{Chroma, Contrast, Element, Theme, Widget};

    #[test]
    fn provider_reload_changes_policy_without_changing_scheme() {
        let mut scene = Scene::new(Element::new(0, Widget::Panel)).unwrap();
        let selection = ThemeSelection {
            policy: ThemePolicy::new(Theme::Dark, Contrast::High, Chroma::Monochrome),
            stylesheet: "panel { padding: 12px; }".into(),
        };
        assert!(apply(&mut scene, &selection).unwrap());
        assert_eq!(scene.theme_policy(), selection.policy);
        assert!(!apply(&mut scene, &selection).unwrap());
    }

    #[test]
    fn rejected_provider_stylesheet_preserves_previous_policy_and_css() {
        let mut scene = Scene::new(Element::new(0, Widget::Panel)).unwrap();
        let selection = ThemeSelection {
            policy: ThemePolicy::new(Theme::Light, Contrast::High, Chroma::FullColor),
            stylesheet: "panel { padding: 12px; }".into(),
        };
        apply(&mut scene, &selection).unwrap();
        let invalid = ThemeSelection {
            policy: ThemePolicy::default(),
            stylesheet: "panel { unknown-property: 12px; }".into(),
        };
        assert!(apply(&mut scene, &invalid).is_err());
        assert_eq!(scene.theme_policy(), selection.policy);
        assert_eq!(scene.user_theme_stylesheet(), selection.stylesheet);
    }
}

/// Desktop portal subscription. Closing its private connection unblocks and joins
/// the listener, so application shutdown never leaves a theme worker behind.
#[cfg(target_os = "linux")]
pub struct DesktopWatcher {
    connection: zbus::blocking::Connection,
    thread: Option<std::thread::JoinHandle<()>>,
    pub scheme: Arc<std::sync::Mutex<Option<knave_ui::toolkit::Theme>>>,
}
#[cfg(target_os = "linux")]
impl DesktopWatcher {
    pub fn start(
        wake: impl Fn() -> bool + Send + 'static,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::subscribe(zbus::blocking::Connection::session()?, wake)
    }

    fn subscribe(
        connection: zbus::blocking::Connection,
        wake: impl Fn() -> bool + Send + 'static,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let settings = zbus::blocking::Proxy::new(
            &connection,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Settings",
        )?;
        // Subscribe before reading to avoid losing a change during startup.
        let signals = settings.receive_signal("SettingChanged")?;
        let value: zbus::zvariant::OwnedValue =
            settings.call("ReadOne", &("org.freedesktop.appearance", "color-scheme"))?;
        let scheme = Arc::new(std::sync::Mutex::new(portal_scheme(value)));
        let shared = scheme.clone();
        let thread = std::thread::Builder::new()
            .name("desktop-theme".into())
            .spawn(move || {
                for message in signals {
                    let Ok((namespace, key, value)) =
                        message
                            .body()
                            .deserialize::<(String, String, zbus::zvariant::OwnedValue)>()
                    else {
                        continue;
                    };
                    if namespace == "org.freedesktop.appearance" && key == "color-scheme" {
                        *shared.lock().unwrap() = portal_scheme(value);
                        if !wake() {
                            break;
                        }
                    }
                }
            })?;
        Ok(Self {
            connection,
            thread: Some(thread),
            scheme,
        })
    }
}
#[cfg(target_os = "linux")]
impl Drop for DesktopWatcher {
    fn drop(&mut self) {
        let _ = self.connection.clone().close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
#[cfg(target_os = "linux")]
fn portal_scheme(value: zbus::zvariant::OwnedValue) -> Option<knave_ui::toolkit::Theme> {
    match u32::try_from(value).ok()? {
        1 => Some(knave_ui::toolkit::Theme::Dark),
        2 => Some(knave_ui::toolkit::Theme::Light),
        _ => None,
    }
}

#[cfg(all(test, target_os = "linux"))]
mod desktop_tests {
    use super::*;
    #[test]
    fn portal_preferences_are_decoded_without_inventing_a_preference() {
        use knave_ui::toolkit::Theme;
        assert_eq!(portal_scheme(1u32.into()), Some(Theme::Dark));
        assert_eq!(portal_scheme(2u32.into()), Some(Theme::Light));
        assert_eq!(portal_scheme(0u32.into()), None);
        assert_eq!(portal_scheme(99u32.into()), None);
        assert_eq!(portal_scheme(true.into()), None);
    }
    struct Settings;
    #[zbus::interface(name = "org.freedesktop.portal.Settings")]
    impl Settings {
        fn read_one(&self, _namespace: &str, _key: &str) -> zbus::zvariant::OwnedValue {
            2u32.into()
        }
    }

    #[test]
    fn portal_subscription_reads_startup_and_delivers_live_updates() {
        use std::io::BufRead;
        use std::process::{Command, Stdio};
        use std::time::Duration;
        struct Bus(std::process::Child);
        impl Drop for Bus {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut bus = Bus(Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("test requires dbus-daemon"));
        let mut address = String::new();
        std::io::BufReader::new(bus.0.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let server = zbus::blocking::connection::Builder::address(address.trim())
            .unwrap()
            .name("org.freedesktop.portal.Desktop")
            .unwrap()
            .serve_at("/org/freedesktop/portal/desktop", Settings)
            .unwrap()
            .build()
            .unwrap();
        let client = zbus::blocking::connection::Builder::address(address.trim())
            .unwrap()
            .build()
            .unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let watcher = DesktopWatcher::subscribe(client, move || sender.send(()).is_ok()).unwrap();
        assert_eq!(
            *watcher.scheme.lock().unwrap(),
            Some(knave_ui::toolkit::Theme::Light)
        );
        for (value, expected) in [
            (1u32, Some(knave_ui::toolkit::Theme::Dark)),
            (2, Some(knave_ui::toolkit::Theme::Light)),
            (0, None),
        ] {
            server
                .emit_signal(
                    None::<&str>,
                    "/org/freedesktop/portal/desktop",
                    "org.freedesktop.portal.Settings",
                    "SettingChanged",
                    &(
                        "org.freedesktop.appearance",
                        "color-scheme",
                        zbus::zvariant::Value::from(value),
                    ),
                )
                .unwrap();
            receiver.recv_timeout(Duration::from_secs(3)).unwrap();
            assert_eq!(*watcher.scheme.lock().unwrap(), expected);
        }
        // An idle subscription must shut down without waiting for another signal.
        drop(watcher);
    }
    #[test]
    #[ignore = "requires a running desktop settings portal"]
    fn live_portal_startup_and_shutdown() {
        let watcher = DesktopWatcher::start(|| true).unwrap();
        assert!(watcher.scheme.lock().unwrap().is_some());
        drop(watcher);
    }
}
