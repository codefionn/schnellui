//! The window's renderer. With a GPU backend the window opens on the CPU renderer
//! (softbuffer) while a worker thread starts the GPU, then switches to the wgpu
//! surface on the first frame after the GPU is ready. `Backend::Cpu` stays on the
//! CPU, and so does a window whose GPU start fails.

use std::sync::mpsc;
use std::sync::Arc;

use winit::window::Window;

use schnellui::render_soft::SoftSurface;
use schnellui::render_wgpu::{Backend, RendererError, SurfaceBoot, SurfaceDevice, SurfaceRenderer};
use schnellui::scene::{Color, Scene};
use schnellui::text::GlyphAtlas;

enum Active {
    Gpu(Box<SurfaceRenderer>),
    Cpu(Box<SoftSurface>),
}

/// A GPU start-up running on a worker thread.
struct GpuBoot {
    result: mpsc::Receiver<Result<SurfaceDevice, RendererError>>,
    backend: Backend,
}

pub(super) struct WindowRenderer {
    active: Active,
    boot: Option<GpuBoot>,
    scale: f32,
    clear: Color,
}

impl WindowRenderer {
    /// Picks the renderer for `backend` after the `SCHNELLUI_BACKEND` override.
    /// A GPU backend starts in the background behind the CPU renderer; `wake` runs
    /// on the worker thread once the GPU is ready or has failed, and should lead to
    /// a redraw, which makes the switch.
    pub(super) fn new(
        window: Arc<Window>,
        width: u32,
        height: u32,
        backend: Backend,
        wake: impl FnOnce() + Send + 'static,
    ) -> Result<WindowRenderer, RendererError> {
        let backend = backend.with_env_override();
        if backend == Backend::Cpu {
            let soft = SoftSurface::new(window, width, height)?;
            return Ok(WindowRenderer::with(Active::Cpu(Box::new(soft)), None));
        }
        let soft = match SoftSurface::new(window.clone(), width, height) {
            Ok(soft) => soft,
            Err(e) => {
                eprintln!("windowed: no CPU presenter ({e}); waiting for the GPU");
                return Self::gpu_now(window, width, height, backend);
            }
        };
        // macOS creates the Metal layer on the main thread, and winit on Windows
        // hands out the window's handle only on the event loop's thread, so there
        // the surface starts here and only the adapter and device on the worker.
        // Elsewhere the instance and the surface start on the worker too, which is
        // most of the wait.
        #[cfg(any(target_os = "macos", windows))]
        let start = {
            let staged = match SurfaceBoot::new(window.clone(), backend) {
                Ok(staged) => staged,
                Err(e) => return Self::cpu_after(soft, backend, e),
            };
            move || staged.prepare()
        };
        #[cfg(not(any(target_os = "macos", windows)))]
        let start = {
            let window = window.clone();
            move || SurfaceBoot::new(window, backend).and_then(SurfaceBoot::prepare)
        };
        let (send, result) = mpsc::sync_channel(1);
        let spawned = std::thread::Builder::new()
            .name("schnellui-gpu-boot".into())
            .spawn(move || {
                let _ = send.send(start());
                wake();
            });
        match spawned {
            Ok(_) => Ok(WindowRenderer::with(
                Active::Cpu(Box::new(soft)),
                Some(GpuBoot { result, backend }),
            )),
            Err(e) => {
                eprintln!("windowed: could not start the GPU thread ({e})");
                drop(soft);
                Self::gpu_now(window, width, height, backend)
            }
        }
    }

    fn with(active: Active, boot: Option<GpuBoot>) -> WindowRenderer {
        WindowRenderer {
            active,
            boot,
            scale: 1.0,
            clear: Color::default(),
        }
    }

    /// Starts the GPU on this thread, falling back to the CPU under `Auto`.
    fn gpu_now(
        window: Arc<Window>,
        width: u32,
        height: u32,
        backend: Backend,
    ) -> Result<WindowRenderer, RendererError> {
        match SurfaceRenderer::new(window.clone(), width, height, backend) {
            Ok(r) => Ok(WindowRenderer::with(Active::Gpu(Box::new(r)), None)),
            Err(e) if backend == Backend::Auto => {
                eprintln!("windowed: no usable GPU ({e}); drawing on the CPU");
                let soft = SoftSurface::new(window, width, height)?;
                Ok(WindowRenderer::with(Active::Cpu(Box::new(soft)), None))
            }
            Err(e) => Err(e),
        }
    }

    #[cfg(any(target_os = "macos", windows))]
    fn cpu_after(
        soft: SoftSurface,
        backend: Backend,
        e: RendererError,
    ) -> Result<WindowRenderer, RendererError> {
        gpu_failed(backend, &e);
        Ok(WindowRenderer::with(Active::Cpu(Box::new(soft)), None))
    }

    pub(super) fn size(&self) -> (u32, u32) {
        match &self.active {
            Active::Gpu(r) => r.size(),
            Active::Cpu(r) => r.size(),
        }
    }

    pub(super) fn resize(&mut self, width: u32, height: u32) {
        match &mut self.active {
            Active::Gpu(r) => r.resize(width, height),
            Active::Cpu(r) => r.resize(width, height),
        }
    }

    pub(super) fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
        match &mut self.active {
            Active::Gpu(r) => r.set_scale(scale),
            Active::Cpu(r) => r.set_scale(scale),
        }
    }

    pub(super) fn set_clear_color(&mut self, color: Color) {
        self.clear = color;
        match &mut self.active {
            Active::Gpu(r) => r.set_clear_color(color),
            Active::Cpu(r) => r.set_clear_color(color),
        }
    }

    /// Drops the GPU atlas textures; the CPU renderer reads the atlases directly,
    /// and a GPU that is still starting uploads them whole on its first frame.
    pub(super) fn invalidate_atlases(&mut self) {
        if let Active::Gpu(r) = &mut self.active {
            r.invalidate_atlases();
        }
    }

    /// See [`SurfaceRenderer::reconcile_remount_atlases`]; nothing to reconcile on
    /// the CPU.
    pub(super) fn reconcile_remount_atlases(&mut self, glyphs: &mut GlyphAtlas) {
        if let Active::Gpu(r) = &mut self.active {
            r.reconcile_remount_atlases(glyphs);
        }
    }

    /// Draws a frame. The first frame after the GPU is ready draws on the GPU; the
    /// CPU presenter is dropped only after that frame is presented, so the window
    /// never shows an empty surface in between.
    pub(super) fn render(&mut self, scene: &Scene, atlas: &mut GlyphAtlas) {
        let retired = self.poll_boot();
        match &mut self.active {
            Active::Gpu(r) => r.render(scene, atlas),
            Active::Cpu(r) => r.render(scene, atlas),
        }
        drop(retired);
    }

    /// Takes the GPU over from the CPU when the worker has finished, and returns
    /// the presenter it replaced.
    fn poll_boot(&mut self) -> Option<Active> {
        let boot = self.boot.as_ref()?;
        let result = match boot.result.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err(RendererError::NoDevice(
                "the GPU start-up thread ended without a result".into(),
            )),
        };
        let backend = boot.backend;
        self.boot = None;
        match result {
            Ok(device) => {
                let (width, height) = self.size();
                let mut gpu = device.attach(width, height);
                gpu.set_scale(self.scale);
                gpu.set_clear_color(self.clear);
                Some(std::mem::replace(
                    &mut self.active,
                    Active::Gpu(Box::new(gpu)),
                ))
            }
            Err(e) => {
                gpu_failed(backend, &e);
                None
            }
        }
    }
}

fn gpu_failed(backend: Backend, e: &RendererError) {
    if backend == Backend::Auto {
        eprintln!("windowed: no usable GPU ({e}); drawing on the CPU");
    } else {
        eprintln!("windowed: the {backend:?} GPU backend failed ({e}); drawing on the CPU");
    }
}
