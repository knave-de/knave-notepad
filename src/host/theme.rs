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
