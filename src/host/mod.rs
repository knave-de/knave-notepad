use crate::ipc;
// Adapted from knave-app at 6d32ccc; see README.md and LICENSE in this directory.
// Event-driven, single-window host for retained Knave UI scenes.
//
// Clipboard requests use the system clipboard; actions also reach the application.
mod clipboard;
mod theme;

use std::{error::Error, sync::Arc};

use knave_renderer::WgpuPainter;
use knave_ui::{DisplayCommand, Transform2D, toolkit::*};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalPosition, LogicalSize},
    event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key as NativeKey, ModifiersState, NamedKey},
    window::{CursorIcon, Window, WindowId},
};

pub use theme::ThemeProvider;

pub type AppResult<T = ()> = Result<T, Box<dyn Error>>;

/// Application state and service integration stay outside the UI scene.
pub trait Application {
    /// Called after the common text engine is available, before the first frame.
    fn ready(&mut self, _text: &mut dyn TextMeasurer) -> AppResult {
        Ok(())
    }
    /// Called when the theme scheme is applied, including before the first frame.
    fn theme_changed(&mut self, _theme: Theme) -> AppResult {
        Ok(())
    }
    /// Called after a provider applies a theme policy, including contrast and chroma.
    /// Existing applications can keep overriding `theme_changed` to react to the scheme.
    fn theme_policy_changed(&mut self, policy: ThemePolicy) -> AppResult {
        self.theme_changed(policy.scheme)
    }
    /// Rebuild theme-dependent cached content using the text engine after a live update.
    fn refresh_theme(&mut self, _text: &mut dyn TextMeasurer) -> AppResult {
        Ok(())
    }
    /// Consume a native key before the toolkit, including its accompanying text.
    fn modifiers_changed(&mut self, _modifiers: ModifiersState) {}
    fn key(&mut self, _key: &NativeKey, _modifiers: ModifiersState) -> AppResult<bool> {
        Ok(false)
    }
    /// Consume committed IME or clipboard text before the toolkit.
    fn text(&mut self, _text: &str) -> AppResult<bool> {
        Ok(false)
    }
    fn input(&mut self, _input: &Input) -> AppResult<bool> {
        Ok(false)
    }
    fn effects(&mut self) -> Vec<Action> {
        Vec::new()
    }
    fn paint(&mut self, _list: &mut knave_ui::DisplayList, _text: &mut dyn TextMeasurer) {}
    fn resize(&mut self, _size: [f32; 2]) -> AppResult {
        Ok(())
    }
    fn ime_state(&self) -> Option<TextInputState> {
        None
    }
    fn close_requested(&mut self) -> AppResult<bool> {
        Ok(true)
    }
    fn shutdown(&mut self) -> AppResult {
        Ok(())
    }
    fn activate_window(&mut self, _id: u64) -> AppResult {
        Ok(())
    }
    fn open_window_files(&mut self, _paths: &[std::path::PathBuf]) -> AppResult {
        Ok(())
    }
    fn window_removed(&mut self, _id: u64) {}
    fn document_generation(&self) -> u64 {
        0
    }
    fn next_checkpoint(&self) -> Option<std::time::Instant> {
        None
    }
    fn checkpoint(&mut self, _now: std::time::Instant) -> AppResult {
        Ok(())
    }
    fn take_exit_request(&mut self) -> bool {
        false
    }
    fn scene(&mut self) -> &mut Scene;
    /// Clipboard effects are handled before delivery; cut also reports the edited value.
    fn action(&mut self, action: Action) -> AppResult;
    /// Recoverable platform clipboard failures do not close the application.
    fn clipboard_error(&mut self, error: &dyn Error) {
        eprintln!("clipboard: {error}");
    }
}

/// Run one window following the desktop theme preference.
pub fn run(
    title: &str,
    size: [f64; 2],
    app: impl Application + 'static,
    server: ipc::Server,
) -> AppResult {
    run_inner(title, size, app, None, true, Some(server))
}

/// Run one window with a caller-supplied theme and optional live file updates.
///
/// On Linux, the provider's watched paths are monitored until the window closes.
/// The provider is responsible for resolving application-owned configuration
/// into a [`ThemeSelection`].
#[allow(dead_code)]
pub fn run_with_theme_provider(
    title: &str,
    size: [f64; 2],
    app: impl Application + 'static,
    provider: Arc<dyn ThemeProvider>,
) -> AppResult {
    run_inner(title, size, app, Some(provider), false, None)
}

/// Run without changing the application theme.
#[allow(dead_code)]
pub fn run_unthemed(title: &str, size: [f64; 2], app: impl Application + 'static) -> AppResult {
    run_inner(title, size, app, None, false, None)
}

fn run_inner(
    title: &str,
    size: [f64; 2],
    mut app: impl Application + 'static,
    provider: Option<Arc<dyn ThemeProvider>>,
    follow_desktop: bool,
    server: Option<ipc::Server>,
) -> AppResult {
    if size.iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return Err("window dimensions must be finite and positive".into());
    }
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    if let Some(provider) = &provider {
        match provider.load() {
            Ok(selection) => match theme::apply(app.scene(), &selection) {
                Ok(_) => app.theme_policy_changed(selection.policy)?,
                Err(error) => {
                    eprintln!("theme stylesheet: {error}");
                    theme::apply_fallback(app.scene(), &selection)?;
                    app.theme_policy_changed(selection.policy)?;
                }
            },
            Err(error) => eprintln!("theme provider: {error}"),
        }
    }
    #[cfg(target_os = "linux")]
    let theme_provider = provider;
    #[cfg(target_os = "linux")]
    let watcher = theme_provider
        .as_ref()
        .filter(|provider| !provider.watch_paths().is_empty())
        .and_then(|provider| {
            match theme::Watcher::start(provider.clone(), event_loop.create_proxy()) {
                Ok(watcher) => Some(watcher),
                Err(error) => {
                    eprintln!("theme watcher: {error}");
                    None
                }
            }
        });
    #[cfg(target_os = "linux")]
    let desktop_watcher = if follow_desktop {
        match theme::DesktopWatcher::start({
            let proxy = event_loop.create_proxy();
            move || proxy.send_event(()).is_ok()
        }) {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                eprintln!("desktop theme: {error}");
                None
            }
        }
    } else {
        None
    };
    #[cfg(target_os = "linux")]
    if let Some(watcher) = &desktop_watcher
        && let Some(scheme) = *watcher.scheme.lock().unwrap()
    {
        app.scene().set_theme(scheme)?;
    }
    if follow_desktop {
        let policy = app.scene().theme_policy();
        app.theme_policy_changed(policy)?;
    }
    let incoming = server
        .as_ref()
        .map(|server| {
            let proxy = event_loop.create_proxy();
            server.receive(move || proxy.send_event(()).is_ok())
        })
        .transpose()?;
    let mut host = Host {
        #[cfg(target_os = "linux")]
        desktop_watcher,
        #[cfg(target_os = "linux")]
        watcher,
        #[cfg(target_os = "linux")]
        theme_provider,
        title: title.into(),
        size,
        app,
        gpu: None,
        incoming,
        inactive: Default::default(),
        window: 0,
        next_window: 1,
        modifiers: ModifiersState::empty(),
        pointer: [0.0; 2],
        composing: false,
        app_dirty: true,
        error: None,
        epoch: std::time::Instant::now(),
    };
    let result = event_loop.run_app(&mut host);
    // Remove the IPC endpoint while the application still holds the data lock.
    drop(server);
    result?;
    match host.error {
        Some(error) => Err(error),
        None => host.app.shutdown(),
    }
}

struct Gpu {
    clipboard: clipboard::Clipboard,
    // Drop the surface before its window.
    surface: wgpu::Surface<'static>,
    window: Arc<Window>,
    instance: wgpu::Instance,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    supports_surface_view_formats: bool,
    painter: WgpuPainter,
    occluded: bool,
}
impl Gpu {
    fn new(window: Arc<Window>) -> AppResult<Self> {
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone())?;
        let adapter =
            pollster::block_on(knave_renderer::select_adapter(&instance, Some(&surface)))?;
        let supports_surface_view_formats = adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::SURFACE_VIEW_FORMATS);
        let info = adapter.get_info();
        eprintln!(
            "knave-app: GPU adapter {} ({:?}, {})",
            info.name, info.backend, info.driver
        );
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface has no supported configuration")?;
        config.format = surface
            .get_capabilities(&adapter)
            .formats
            .into_iter()
            .find(|f| f.is_srgb())
            .ok_or("surface has no sRGB format")?;
        if supports_surface_view_formats {
            config.view_formats = vec![
                WgpuPainter::paired_text_format(config.format)
                    .ok_or("sRGB surface has no compatible Unorm text view format")?,
            ];
        }
        let painter = WgpuPainter::new(&device, &queue, config.format)?;
        if cfg!(debug_assertions) && std::env::var_os("KNAVE_NOTEPAD_CAPTURE_REQUEST").is_some() {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
        }
        surface.configure(&device, &config);
        let clipboard = clipboard::Clipboard::new(&window)?;
        Ok(Self {
            clipboard,
            surface,
            window,
            instance,
            device,
            queue,
            config,
            supports_surface_view_formats,
            painter,
            occluded: false,
        })
    }
    fn resize(&mut self) {
        let size = self.window.inner_size();
        if size.width > 0 && size.height > 0 {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&self.device, &self.config);
        }
    }
    fn layout(&mut self, scene: &mut Scene) {
        let size = self
            .window
            .inner_size()
            .to_logical::<f32>(self.window.scale_factor());
        scene.layout([size.width, size.height], &mut self.painter);
    }
    /// Debug-only opt-in frame readback, used to inspect the actual presented application.
    fn capture(&self, texture: &wgpu::Texture, path: &str) -> AppResult {
        let width = self.config.width;
        let height = self.config.height;
        let pitch = (width * 4).div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Notes visual QA"),
            size: (pitch * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(pitch),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        rx.recv()??;
        let mapped = slice.get_mapped_range()?;
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for row in mapped.chunks(pitch as usize) {
            pixels.extend_from_slice(&row[..width as usize * 4]);
        }
        if matches!(
            self.config.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        ) {
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
        }
        let mut png = png::Encoder::new(std::fs::File::create(path)?, width, height);
        png.set_color(png::ColorType::Rgba);
        png.set_depth(png::BitDepth::Eight);
        png.write_header()?.write_image_data(&pixels)?;
        Ok(())
    }
    fn draw(&mut self, app: &mut impl Application) -> AppResult {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Ok(());
        }
        let mut reconfigure = false;
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                reconfigure = true;
                frame
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.resize();
                self.window.request_redraw();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self.instance.create_surface(self.window.clone())?;
                self.resize();
                self.window.request_redraw();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("surface acquisition validation failed".into());
            }
        };
        let scale = self.window.scale_factor() as f32;
        let logical = size.to_logical::<f32>(scale as f64);
        let scene = app.scene();
        let mut list = scene
            .display_list([logical.width, logical.height], &mut self.painter)
            .clone();
        let overlay_start = scene.overlay_start();
        let overlay_len = list.commands.len() - overlay_start;
        app.paint(&mut list, &mut self.painter);
        list.commands[overlay_start..].rotate_left(overlay_len);
        scale_list(&mut list, scale);
        let view = frame.texture.create_view(&Default::default());
        let text_view = if list.clear_color.alpha == u8::MAX && self.supports_surface_view_formats {
            self.config.view_formats.first().map(|format| {
                frame.texture.create_view(&wgpu::TextureViewDescriptor {
                    format: Some(*format),
                    ..Default::default()
                })
            })
        } else {
            None
        };
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let [r, g, b, a] = list.clear_color.to_linear_rgba();
        {
            let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("knave-app-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: r.into(),
                            g: g.into(),
                            b: b.into(),
                            a: a.into(),
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }
        if let Some(text_view) = &text_view {
            self.painter.encode_targets(
                &self.device,
                &self.queue,
                &mut encoder,
                &view,
                text_view,
                (size.width, size.height),
                &list,
            )?;
        } else {
            self.painter.encode(
                &self.device,
                &self.queue,
                &mut encoder,
                &view,
                (size.width, size.height),
                &list,
            )?;
        }
        self.queue.submit([encoder.finish()]);
        if cfg!(debug_assertions)
            && let Some(request) = std::env::var_os("KNAVE_NOTEPAD_CAPTURE_REQUEST")
            && let Ok(path) = std::fs::read_to_string(&request)
        {
            let _ = std::fs::remove_file(&request);
            if let Err(error) = self.capture(&frame.texture, path.trim()) {
                eprintln!("capture: {error}");
            }
        }
        self.window.pre_present_notify();
        self.queue.present(frame);
        if reconfigure {
            self.resize();
        }
        Ok(())
    }
}

fn scale_list(list: &mut knave_ui::DisplayList, scale: f32) {
    let scaling = Transform2D::scale(scale, scale);
    for command in &mut list.commands {
        if matches!(
            command,
            DisplayCommand::BeginOpacity { .. } | DisplayCommand::EndOpacity
        ) {
            continue;
        }
        let (transform, clip) = match command {
            DisplayCommand::Shape {
                transform, clip, ..
            }
            | DisplayCommand::Image {
                transform, clip, ..
            }
            | DisplayCommand::Text {
                transform, clip, ..
            }
            | DisplayCommand::RichText {
                transform, clip, ..
            } => (transform, clip),
            DisplayCommand::BeginOpacity { .. } | DisplayCommand::EndOpacity => unreachable!(),
        };
        *transform = scaling.compose(*transform);
        *clip = clip.map(|rect| scaling.transform_rect_bounds(rect));
    }
}

struct WindowSlot {
    token: u64,
    gpu: Gpu,
    modifiers: ModifiersState,
    pointer: [f32; 2],
    composing: bool,
    app_dirty: bool,
}

struct Host<A> {
    #[cfg(target_os = "linux")]
    desktop_watcher: Option<theme::DesktopWatcher>,
    #[cfg(target_os = "linux")]
    watcher: Option<theme::Watcher>,
    #[cfg(target_os = "linux")]
    theme_provider: Option<Arc<dyn ThemeProvider>>,
    epoch: std::time::Instant,
    title: String,
    size: [f64; 2],
    app: A,
    gpu: Option<Gpu>,
    incoming: Option<std::sync::mpsc::Receiver<Vec<std::path::PathBuf>>>,
    inactive: std::collections::HashMap<WindowId, WindowSlot>,
    window: u64,
    next_window: u64,
    modifiers: ModifiersState,
    pointer: [f32; 2],
    composing: bool,
    app_dirty: bool,
    error: Option<Box<dyn Error>>,
}
impl<A: Application> Host<A> {
    fn park_window(&mut self) {
        if let Some(gpu) = self.gpu.take() {
            self.inactive.insert(
                gpu.window.id(),
                WindowSlot {
                    token: self.window,
                    gpu,
                    modifiers: self.modifiers,
                    pointer: self.pointer,
                    composing: self.composing,
                    app_dirty: self.app_dirty,
                },
            );
        }
    }

    fn select_window(&mut self, id: WindowId) -> AppResult<bool> {
        if self.gpu.as_ref().is_some_and(|gpu| gpu.window.id() == id) {
            return Ok(true);
        }
        let Some(slot) = self.inactive.remove(&id) else {
            return Ok(false);
        };
        self.park_window();
        self.app.activate_window(slot.token)?;
        self.window = slot.token;
        self.gpu = Some(slot.gpu);
        self.modifiers = slot.modifiers;
        self.pointer = slot.pointer;
        self.composing = slot.composing;
        self.app_dirty = slot.app_dirty;
        Ok(true)
    }

    fn remove_window(&mut self, event_loop: &ActiveEventLoop) -> AppResult {
        let closed = self.window;
        self.gpu = None;
        if let Some(id) = self.inactive.keys().next().copied() {
            self.select_window(id)?;
            self.app.window_removed(closed);
            self.app_dirty = true;
            self.sync(true);
        } else {
            event_loop.exit();
        }
        Ok(())
    }

    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> AppResult {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_name("knave-notepad", "knave-notepad")
                    .with_title(&self.title)
                    .with_inner_size(LogicalSize::new(self.size[0], self.size[1]))
                    .with_min_inner_size(LogicalSize::new(640.0, 480.0)),
            )?,
        );
        let mut gpu = Gpu::new(window)?;
        self.app.ready(&mut gpu.painter)?;
        self.app
            .resize([self.size[0] as f32, self.size[1] as f32])?;
        gpu.window.request_redraw();
        self.gpu = Some(gpu);
        self.sync(true);
        Ok(())
    }

    fn accept_windows(&mut self, event_loop: &ActiveEventLoop) -> AppResult {
        while let Some(paths) = self
            .incoming
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok())
        {
            self.park_window();
            let id = self.next_window;
            self.next_window += 1;
            self.app.activate_window(id)?;
            self.window = id;
            self.modifiers = ModifiersState::empty();
            self.composing = false;
            self.pointer = [0.0; 2];
            self.app_dirty = true;
            self.app.open_window_files(&paths)?;
            self.create_window(event_loop)?;
        }
        Ok(())
    }

    fn window_ids(&self) -> Vec<WindowId> {
        self.gpu
            .iter()
            .map(|gpu| gpu.window.id())
            .chain(self.inactive.keys().copied())
            .collect()
    }

    fn request_window_redraws(&self) {
        for gpu in self
            .gpu
            .iter()
            .chain(self.inactive.values().map(|slot| &slot.gpu))
        {
            gpu.window.request_redraw();
        }
    }

    #[cfg(target_os = "linux")]
    fn watcher_selection(
        &mut self,
    ) -> Result<Option<(knave_ui::toolkit::ThemePolicy, bool)>, Box<dyn Error>> {
        let Some(provider) = &self.theme_provider else {
            return Ok(None);
        };
        let selection = provider.load().map_err(std::io::Error::other)?;
        let policy = selection.policy;
        let changed = theme::apply(self.app.scene(), &selection)?;
        Ok(Some((policy, changed)))
    }

    fn input(&mut self, input: Input) -> AppResult {
        if let Some(gpu) = &mut self.gpu {
            gpu.layout(self.app.scene());
        }
        if self.app.input(&input)? {
            self.app_dirty = true;
            self.flush_effects()?;
            self.sync(true);
            return Ok(());
        }
        let committed = match &input {
            Input::Text(text) | Input::Paste(text) | Input::PasteTo { text, .. } => Some(text),
            _ => None,
        };
        if let Some(text) = committed
            && self.app.text(text)?
        {
            self.app_dirty = true;
            self.flush_effects()?;
            self.sync(true);
            return Ok(());
        }
        let Some(gpu) = &mut self.gpu else {
            return Ok(());
        };
        // Hit testing must see the latest layout, including resize and action mutations.
        gpu.layout(self.app.scene());
        let result = self.app.scene().event(input);
        if let Some(action) = result.action {
            let clipboard_result = match &action {
                Action::Copy(text) | Action::Cut { copied: text, .. } => self
                    .gpu
                    .as_mut()
                    .expect("active window")
                    .clipboard
                    .store(text.clone())
                    .map(|()| None),
                Action::RequestPaste(target) => self
                    .gpu
                    .as_mut()
                    .expect("active window")
                    .clipboard
                    .load()
                    .map(|text| {
                        Some(Input::PasteTo {
                            target: *target,
                            text,
                        })
                    }),
                _ => Ok(None),
            };
            match clipboard_result {
                Ok(Some(input)) => self.input(input)?,
                Ok(None) => {}
                Err(error) => self.app.clipboard_error(error.as_ref()),
            }
            self.app.action(action)?;
        }
        self.sync(true);
        Ok(())
    }
    fn flush_effects(&mut self) -> AppResult {
        for effect in self.app.effects() {
            let Some(gpu) = &mut self.gpu else {
                continue;
            };
            match effect {
                Action::Copy(text) | Action::Cut { copied: text, .. } => {
                    if let Err(error) = gpu.clipboard.store(text) {
                        self.app.clipboard_error(error.as_ref());
                    }
                }
                Action::RequestPaste(id) => match gpu.clipboard.load() {
                    Ok(text) => self.input(Input::PasteTo { target: id, text })?,
                    Err(error) => self.app.clipboard_error(error.as_ref()),
                },
                _ => {}
            }
        }
        Ok(())
    }
    fn sync(&mut self, redraw: bool) {
        let Some(gpu) = &mut self.gpu else {
            return;
        };
        let size = gpu
            .window
            .inner_size()
            .to_logical::<f32>(gpu.window.scale_factor());
        if let Err(error) = self.app.resize([size.width, size.height]) {
            eprintln!("resize: {error}");
        }
        gpu.layout(self.app.scene());
        let scene = self.app.scene();
        let needs_frame = scene.needs_frame();
        let cursor = scene.cursor();
        gpu.window.set_cursor_visible(cursor != CursorShape::Hidden);
        gpu.window.set_cursor(cursor_icon(cursor));
        let state = scene.text_input_state().or_else(|| self.app.ime_state());
        gpu.window.set_ime_allowed(state.is_some());
        if let Some(state) = state {
            gpu.window
                .set_ime_purpose(if state.purpose == TextPurpose::Password {
                    winit::window::ImePurpose::Password
                } else {
                    winit::window::ImePurpose::Normal
                });
            let r = state.cursor_rectangle;
            gpu.window.set_ime_cursor_area(
                LogicalPosition::new(r.x, r.y),
                LogicalSize::new(r.width, r.height),
            );
        }
        let size = gpu.window.inner_size();
        if redraw
            && !gpu.occluded
            && size.width > 0
            && size.height > 0
            && (needs_frame || self.app_dirty)
        {
            gpu.window.request_redraw();
        }
    }
    fn event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) -> AppResult {
        for action in self.app.scene().advance(self.epoch.elapsed()) {
            self.app.action(action)?;
        }
        match event {
            WindowEvent::CloseRequested => {
                if self.app.close_requested()? {
                    self.remove_window(event_loop)?;
                } else {
                    self.app_dirty = true;
                    self.sync(true);
                }
            }
            WindowEvent::Destroyed => self.remove_window(event_loop)?,
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize();
                    gpu.window.request_redraw();
                }
                self.sync(true);
            }
            WindowEvent::RedrawRequested => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.draw(&mut self.app)?;
                    self.app_dirty = false;
                }
                self.sync(false);
            }
            WindowEvent::Occluded(occluded) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.occluded = occluded;
                    if !occluded {
                        gpu.window.request_redraw();
                    }
                }
            }
            WindowEvent::Focused(focused) => {
                if !focused {
                    self.modifiers = ModifiersState::empty();
                    self.composing = false;
                }
                self.input(if focused {
                    Input::FocusGained
                } else {
                    Input::FocusLost
                })?;
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                self.app.modifiers_changed(self.modifiers);
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self.gpu.as_ref().map_or(1.0, |g| g.window.scale_factor());
                let p = position.to_logical::<f32>(scale);
                self.pointer = [p.x, p.y];
                self.input(Input::PointerMove(self.pointer))?;
            }
            WindowEvent::CursorLeft { .. } => self.input(Input::PointerLeave)?,
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => self.input(Input::ContextMenu(self.pointer))?,
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                self.input(if state.is_pressed() {
                    Input::PointerDown(self.pointer)
                } else {
                    Input::PointerUp(self.pointer)
                })?;
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scale = self.gpu.as_ref().map_or(1.0, |g| g.window.scale_factor());
                let delta = scroll_delta(delta, scale);
                self.input(Input::Scroll {
                    position: self.pointer,
                    delta,
                })?;
            }
            WindowEvent::Ime(ime) => match ime {
                Ime::Enabled => {}
                Ime::Disabled => {
                    self.composing = false;
                    self.input(Input::Preedit(String::new()))?;
                }
                Ime::Preedit(text, _) => {
                    self.composing = !text.is_empty();
                    self.input(Input::Preedit(text))?;
                }
                Ime::Commit(text) => {
                    self.composing = false;
                    self.input(Input::Text(text))?;
                }
            },
            WindowEvent::KeyboardInput {
                event,
                is_synthetic: false,
                ..
            } => {
                if event.state.is_pressed()
                    && !self.composing
                    && self.app.key(&event.logical_key, self.modifiers)?
                {
                    self.app_dirty = true;
                    self.flush_effects()?;
                    self.sync(true);
                    return Ok(());
                }
                self.input(Input::Key {
                    key: translate_key(&event.logical_key),
                    pressed: event.state.is_pressed(),
                    repeat: event.repeat,
                    modifiers: KeyModifiers {
                        shift: self.modifiers.shift_key(),
                        control: self.modifiers.control_key(),
                        alt: self.modifiers.alt_key(),
                        logo: self.modifiers.super_key(),
                    },
                })?;
                if event.state == ElementState::Pressed
                    && !self.composing
                    && !self.modifiers.control_key()
                    && !self.modifiers.super_key()
                    && let Some(text) = event.text
                {
                    let text: String = text.chars().filter(|c| !c.is_control()).collect();
                    if !text.is_empty() {
                        self.input(Input::Text(text))?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: Box<dyn Error>) {
        self.error = Some(error);
        event_loop.exit();
    }
}
impl<A: Application> ApplicationHandler for Host<A> {
    fn user_event(&mut self, event_loop: &ActiveEventLoop, _: ()) {
        if let Err(error) = self.accept_windows(event_loop) {
            self.fail(event_loop, error);
            return;
        }
        #[cfg(target_os = "linux")]
        for id in self.window_ids() {
            if let Err(error) = self.select_window(id) {
                self.fail(event_loop, error);
                return;
            }
            self.refresh_window_theme(event_loop);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = std::time::Instant::now();
        if self
            .app
            .next_checkpoint()
            .is_some_and(|deadline| now >= deadline)
        {
            if let Err(error) = self.app.checkpoint(now) {
                eprintln!("recovery: {error}");
            }
            self.app_dirty = true;
        }
        let mut deadline = self.app.next_checkpoint();
        let generation = self.app.document_generation();
        for id in self.window_ids() {
            let result = (|| {
                if !self.select_window(id)? {
                    return Ok(());
                }
                for action in self.app.scene().advance(self.epoch.elapsed()) {
                    self.app.action(action)?;
                }
                if self.app.take_exit_request() {
                    self.remove_window(event_loop)?;
                    return Ok(());
                }
                self.sync(true);
                if let Some(wakeup) = self.app.scene().next_wakeup() {
                    deadline = deadline.into_iter().chain(Some(now + wakeup)).min();
                }
                Ok(())
            })();
            if let Err(error) = result {
                self.fail(event_loop, error);
                return;
            }
        }
        if generation != self.app.document_generation() {
            self.request_window_redraws();
        }
        event_loop.set_control_flow(deadline.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_none()
            && let Err(error) = self.create_window(event_loop)
        {
            self.fail(event_loop, error);
        }
    }

    fn suspended(&mut self, _: &ActiveEventLoop) {
        self.app.scene().event(Input::FocusLost);
        self.gpu = None;
        self.inactive.clear();
        self.composing = false;
        self.modifiers = ModifiersState::empty();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let generation = self.app.document_generation();
        let result = (|| {
            if self.select_window(id)? {
                self.event(event_loop, event)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.fail(event_loop, error);
        } else if generation != self.app.document_generation() {
            self.request_window_redraws();
        }
    }
}

#[cfg(target_os = "linux")]
impl<A: Application> Host<A> {
    fn refresh_window_theme(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(watcher) = &self.desktop_watcher {
            let scheme = *watcher.scheme.lock().unwrap();
            if let Some(scheme) = scheme {
                let mut policy = self.app.scene().theme_policy();
                if policy.scheme != scheme {
                    policy.scheme = scheme;
                    let result = self
                        .app
                        .scene()
                        .set_theme(scheme)
                        .map_err(|error| Box::new(error) as Box<dyn Error>)
                        .and_then(|()| self.app.theme_policy_changed(policy))
                        .and_then(|()| {
                            if let Some(gpu) = &mut self.gpu {
                                self.app.refresh_theme(&mut gpu.painter)
                            } else {
                                Ok(())
                            }
                        });
                    if let Err(error) = result {
                        self.fail(event_loop, error);
                        return;
                    }
                    self.sync(true);
                }
            }
        }
        if let Some(watcher) = &self.watcher {
            watcher
                .pending
                .store(false, std::sync::atomic::Ordering::Release);
            match self.watcher_selection() {
                Ok(Some((policy, true))) => {
                    let result = self.app.theme_policy_changed(policy).and_then(|()| {
                        if let Some(gpu) = &mut self.gpu {
                            self.app.refresh_theme(&mut gpu.painter)
                        } else {
                            Ok(())
                        }
                    });
                    if let Err(error) = result {
                        self.fail(event_loop, error);
                        return;
                    }
                }
                Ok(Some((_, false))) | Ok(None) => {}
                Err(error) => eprintln!("theme: {error}"),
            }
            self.sync(true);
        }
    }
}
// Winit reports content motion; Scene expects movement of the scroll offset.
// Natural scrolling has already been applied by the platform. Do not invert it again.
fn scroll_delta(delta: MouseScrollDelta, scale: f64) -> [f32; 2] {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => [-x * 40.0, -y * 40.0],
        MouseScrollDelta::PixelDelta(position) => {
            let logical = position.to_logical::<f32>(scale);
            [-logical.x, -logical.y]
        }
    }
}

fn translate_key(key: &NativeKey) -> Key {
    match key {
        NativeKey::Named(named) => match named {
            NamedKey::Tab => Key::Tab,
            NamedKey::Enter => Key::Enter,
            NamedKey::Space => Key::Space,
            NamedKey::Escape => Key::Escape,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Delete => Key::Delete,
            NamedKey::ContextMenu => Key::ContextMenu,
            NamedKey::F10 => Key::F10,
            _ => Key::Other,
        },
        NativeKey::Character(text) => match text.to_lowercase().as_str() {
            " " => Key::Space,
            "a" => Key::A,
            "c" => Key::C,
            "x" => Key::X,
            "v" => Key::V,
            "q" => Key::Q,
            "k" => Key::K,
            _ => Key::Other,
        },
        _ => Key::Other,
    }
}
fn cursor_icon(cursor: CursorShape) -> CursorIcon {
    match cursor {
        CursorShape::Default | CursorShape::Hidden => CursorIcon::Default,
        CursorShape::Pointer => CursorIcon::Pointer,
        CursorShape::Text => CursorIcon::Text,
        CursorShape::EwResize => CursorIcon::EwResize,
        CursorShape::NsResize => CursorIcon::NsResize,
        CursorShape::Grab => CursorIcon::Grab,
        CursorShape::Grabbing => CursorIcon::Grabbing,
        CursorShape::NotAllowed => CursorIcon::NotAllowed,
        CursorShape::Move => CursorIcon::Move,
        CursorShape::NwseResize => CursorIcon::NwseResize,
        CursorShape::NeswResize => CursorIcon::NeswResize,
        CursorShape::Crosshair => CursorIcon::Crosshair,
        CursorShape::Wait => CursorIcon::Wait,
        CursorShape::Progress => CursorIcon::Progress,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use knave_ui::{Color, DisplayList, Rect, TextStyle};

    #[test]
    fn fractional_dpi_scales_transform_and_output_clip_together() {
        let mut list = DisplayList {
            revision: 1,
            clear_color: Color::BACKGROUND,
            commands: vec![DisplayCommand::Text {
                bounds: Rect::new(0.0, 0.0, 100.0, 20.0),
                text: "HiDPI".into(),
                style: TextStyle::default(),
                transform: Transform2D::translation(12.0, 8.0),
                clip: Some(Rect::new(10.0, 5.0, 80.0, 20.0)),
            }],
        };
        scale_list(&mut list, 1.5);
        let DisplayCommand::Text {
            transform,
            clip,
            bounds,
            ..
        } = &list.commands[0]
        else {
            panic!()
        };
        assert_eq!(
            *transform,
            Transform2D::scale(1.5, 1.5).compose(Transform2D::translation(12.0, 8.0))
        );
        assert_eq!(*clip, Some(Rect::new(15.0, 7.5, 120.0, 30.0)));
        assert_eq!(*bounds, Rect::new(0.0, 0.0, 100.0, 20.0));
    }

    #[test]
    fn scroll_conversion_preserves_platform_direction_and_logical_distance() {
        use winit::dpi::PhysicalPosition;
        // Conventional wheel down and its platform-inverted counterpart.
        assert_eq!(
            scroll_delta(MouseScrollDelta::LineDelta(0.0, -1.0), 2.0),
            [0.0, 40.0]
        );
        assert_eq!(
            scroll_delta(MouseScrollDelta::LineDelta(0.0, 1.0), 2.0),
            [0.0, -40.0]
        );
        // Smooth input is physical pixels in winit, logical offsets in Scene.
        assert_eq!(
            scroll_delta(
                MouseScrollDelta::PixelDelta(PhysicalPosition::new(-30.0, -60.0)),
                1.5
            ),
            [20.0, 40.0]
        );
        assert_eq!(
            scroll_delta(
                MouseScrollDelta::PixelDelta(PhysicalPosition::new(30.0, 60.0)),
                1.5
            ),
            [-20.0, -40.0]
        );
    }

    #[test]
    fn platform_wheel_down_advances_content_and_reverse_restores_it() {
        struct NoText;
        impl TextMeasurer for NoText {
            fn measure(&mut self, _: &str, _: &TextStyle, _: f32) -> TextLayout {
                panic!("no text in scene")
            }
            fn measure_spans(
                &mut self,
                _: &[knave_ui::TextSpan],
                _: &TextStyle,
                _: f32,
            ) -> TextLayout {
                panic!("no text in scene")
            }
        }
        let mut scene = Scene::new(
            Element::new(0, Widget::Scroll)
                .layout(Layout {
                    height: Length::Fill,
                    ..Default::default()
                })
                .children(vec![Element::new(1, Widget::Panel).layout(Layout {
                    height: Length::Px(300.0),
                    ..Default::default()
                })]),
        )
        .unwrap();
        scene.layout([200.0, 100.0], &mut NoText);
        let initial = scene.bounds(ElementId(1)).unwrap().y;
        scene.event(Input::Scroll {
            position: [20.0, 20.0],
            delta: scroll_delta(MouseScrollDelta::LineDelta(0.0, -1.0), 1.0),
        });
        scene.layout([200.0, 100.0], &mut NoText);
        assert_eq!(scene.bounds(ElementId(1)).unwrap().y, initial - 40.0);
        scene.event(Input::Scroll {
            position: [20.0, 20.0],
            delta: scroll_delta(MouseScrollDelta::LineDelta(0.0, 1.0), 1.0),
        });
        scene.layout([200.0, 100.0], &mut NoText);
        assert_eq!(scene.bounds(ElementId(1)).unwrap().y, initial);
    }

    #[test]
    fn logical_keys_preserve_shifted_clipboard_shortcuts_and_space() {
        assert_eq!(translate_key(&NativeKey::Character("C".into())), Key::C);
        assert_eq!(translate_key(&NativeKey::Character(" ".into())), Key::Space);
        assert_eq!(
            translate_key(&NativeKey::Named(NamedKey::ArrowLeft)),
            Key::Left
        );
        assert_eq!(translate_key(&NativeKey::Character("é".into())), Key::Other);
    }
}
