//! Captura de tela e de janela pelo Windows Graphics Capture, sem o seletor do navegador e sem
//! a borda amarela. Os quadros vão para o WebView por memória compartilhada (`bridge`) e lá viram
//! uma trilha de vídeo comum, então preview, gravação e transmissão seguem o mesmo caminho das
//! placas de captura.

mod bridge;
pub mod sources;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::core::{Interface, Result as WinResult};
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

pub use bridge::Bridge;
use bridge::FrameInfo;

#[derive(serde::Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStarted {
    pub width: u32,
    pub height: u32,
}

struct Running {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// Uma captura por vez (é a fonte de vídeo do app).
pub struct ScreenCapture {
    bridge: Arc<Bridge>,
    running: Mutex<Option<Running>>,
    fps: Arc<AtomicU32>,
}

impl ScreenCapture {
    pub fn new(bridge: Bridge) -> Self {
        Self { bridge: Arc::new(bridge), running: Mutex::new(None), fps: Arc::new(AtomicU32::new(60)) }
    }

    /// `id`: `screen:\\.\DISPLAY2` ou `window:<hwnd>` (ver `sources`).
    pub fn start(&self, id: &str, fps: u32, cursor: bool) -> Result<CaptureStarted, String> {
        self.stop();
        self.fps.store(fps.max(1), Ordering::Relaxed);
        Target::parse(id).ok_or_else(|| crate::error::code("capture_gone"))?;
        let id = id.to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let (ready, started) = mpsc::channel();
        let thread = {
            let (bridge, fps, stop) = (self.bridge.clone(), self.fps.clone(), stop.clone());
            std::thread::Builder::new()
                .name("capturax-screen-capture".into())
                .spawn(move || run(&id, cursor, bridge, fps, stop, ready))
                .map_err(|e| e.to_string())?
        };
        let result = started.recv_timeout(Duration::from_secs(5)).unwrap_or_else(|_| Err(crate::error::code("capture_timeout")));
        *self.running.lock().unwrap() = Some(Running { stop, thread: Some(thread) });
        if result.is_err() {
            self.stop();
        }
        result
    }

    /// Ponte com o WebView (também usada pelo áudio do P2P).
    pub fn bridge(&self) -> Arc<Bridge> {
        self.bridge.clone()
    }

    /// Limite de quadros por segundo (vale na hora).
    pub fn set_fps(&self, fps: u32) {
        self.fps.store(fps.max(1), Ordering::Relaxed);
    }

    pub fn stop(&self) {
        let Some(mut running) = self.running.lock().unwrap().take() else { return };
        running.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = running.thread.take() {
            let _ = thread.join();
        }
        self.bridge.release();
    }
}

enum Target {
    Monitor(HMONITOR),
    Window(HWND),
}

impl Target {
    fn parse(id: &str) -> Option<Self> {
        if let Some(device) = id.strip_prefix("screen:") {
            return sources::monitor_handles()
                .into_iter()
                .find(|&monitor| sources::monitor_device(monitor).is_some_and(|(name, _)| name.eq_ignore_ascii_case(device)))
                .map(Target::Monitor);
        }
        let hwnd = id.strip_prefix("window:")?.parse::<isize>().ok()?;
        Some(Target::Window(HWND(hwnd as _)))
    }

    fn item(&self) -> WinResult<GraphicsCaptureItem> {
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        unsafe {
            match *self {
                Target::Monitor(monitor) => interop.CreateForMonitor(monitor),
                Target::Window(hwnd) => interop.CreateForWindow(hwnd),
            }
        }
    }
}

fn create_device() -> WinResult<(ID3D11Device, ID3D11DeviceContext, IDirect3DDevice)> {
    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, Default::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))?;
        let device = device.unwrap();
        let dxgi: IDXGIDevice = device.cast()?;
        let winrt: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;
        Ok((device, context.unwrap(), winrt))
    }
}

fn staging_texture(device: &ID3D11Device, width: u32, height: u32) -> WinResult<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut texture = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture))? };
    Ok(texture.unwrap())
}

fn run(id: &str, cursor: bool, bridge: Arc<Bridge>, fps: Arc<AtomicU32>, stop: Arc<AtomicBool>, ready: mpsc::Sender<Result<CaptureStarted, String>>) {
    let _com = crate::audio::ComGuard::new();
    let setup = || -> WinResult<_> {
        // Os handles não podem mudar de thread: resolve o id aqui.
        let item = Target::parse(id).ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_INVALIDARG))?.item()?;
        let (device, context, winrt_device) = create_device()?;
        let size = item.Size()?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&winrt_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size)?;
        let session: GraphicsCaptureSession = pool.CreateCaptureSession(&item)?;
        let _ = session.SetIsCursorCaptureEnabled(cursor);
        // Windows 11: sem a borda amarela em volta do que está sendo capturado.
        let _ = session.SetIsBorderRequired(false);
        Ok((item, device, context, winrt_device, pool, session, size))
    };
    let (item, device, context, winrt_device, pool, session, size) = match setup() {
        Ok(parts) => parts,
        Err(error) => {
            let _ = ready.send(Err(crate::error::with_detail("capture_failed", error.message())));
            return;
        }
    };

    let (signal, frames) = mpsc::channel::<()>();
    let arrived = {
        let signal = signal.clone();
        pool.FrameArrived(&TypedEventHandler::new(move |_, _| {
            let _ = signal.send(());
            Ok(())
        }))
    };
    // Janela fechada: a captura termina e o WebView avisa o usuário.
    let closed = {
        let stop = stop.clone();
        let bridge = bridge.clone();
        item.Closed(&TypedEventHandler::new(move |_, _| {
            stop.store(true, Ordering::Relaxed);
            bridge.post(r#"{"capturax":"ended"}"#.into());
            Ok(())
        }))
    };
    let applied_fps = fps.load(Ordering::Relaxed);
    // Windows 11 24H2+: o próprio Windows limita a taxa (menos trabalho); antes disso, só o filtro abaixo.
    let _ = session.SetMinUpdateInterval(TimeSpan { Duration: 10_000_000 / applied_fps.max(1) as i64 });
    if let Err(error) = session.StartCapture() {
        let _ = ready.send(Err(crate::error::with_detail("capture_failed", error.message())));
        return;
    }
    let _ = ready.send(Ok(CaptureStarted { width: size.Width as u32 & !1, height: size.Height as u32 & !1 }));

    let mut pool_size = size;
    let mut staging: Option<(ID3D11Texture2D, u32, u32)> = None;
    let mut last_ticks: Option<i64> = None;
    let mut applied_fps = applied_fps;

    while !stop.load(Ordering::Relaxed) {
        if frames.recv_timeout(Duration::from_millis(100)).is_err() {
            continue;
        }
        let limit = fps.load(Ordering::Relaxed).max(1);
        if limit != applied_fps {
            applied_fps = limit;
            let _ = session.SetMinUpdateInterval(TimeSpan { Duration: 10_000_000 / limit as i64 });
        }
        // Pega o mais recente; os mais velhos são descartados.
        let mut latest = None;
        while let Ok(frame) = pool.TryGetNextFrame() {
            latest = Some(frame);
        }
        let Some(frame) = latest else { continue };

        let result = (|| -> WinResult<()> {
            let ticks = frame.SystemRelativeTime()?.Duration;
            // Limite de FPS: 10% de folga para não descartar quadros que chegam um pouco antes.
            if last_ticks.is_some_and(|last| (ticks - last) < 9_000_000 / limit as i64) {
                return Ok(());
            }
            let content = frame.ContentSize()?;
            if content.Width != pool_size.Width || content.Height != pool_size.Height {
                // Janela mudou de tamanho: o próximo quadro já vem no tamanho novo.
                pool_size = SizeInt32 { Width: content.Width.max(2), Height: content.Height.max(2) };
                pool.Recreate(&winrt_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, pool_size)?;
            }
            let texture: ID3D11Texture2D = unsafe { frame.Surface()?.cast::<IDirect3DDxgiInterfaceAccess>()?.GetInterface()? };
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            unsafe { texture.GetDesc(&mut desc) };
            // Tamanho par (os encoders de vídeo exigem).
            let width = (content.Width.max(2) as u32).min(desc.Width) & !1;
            let height = (content.Height.max(2) as u32).min(desc.Height) & !1;
            if staging.as_ref().is_none_or(|(_, w, h)| *w != width || *h != height) {
                staging = Some((staging_texture(&device, width, height)?, width, height));
            }
            let target = &staging.as_ref().unwrap().0;
            let region = D3D11_BOX { left: 0, top: 0, front: 0, right: width, bottom: height, back: 1 };
            unsafe { context.CopySubresourceRegion(target, 0, 0, 0, 0, &texture, 0, Some(&region)) };

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            unsafe { context.Map(target, 0, D3D11_MAP_READ, 0, Some(&mut mapped))? };
            let row = width as usize * 4;
            let pitch = mapped.RowPitch as usize;
            let source = mapped.pData as *const u8;
            let sent = bridge.send(FrameInfo { width, height, timestamp_us: ticks / 10 }, |destination| {
                for y in 0..height as usize {
                    unsafe { std::ptr::copy_nonoverlapping(source.add(y * pitch), destination.as_mut_ptr().add(y * row), row) };
                }
            });
            unsafe { context.Unmap(target, 0) };
            match sent {
                Ok(true) => last_ticks = Some(ticks),
                Ok(false) => {}
                Err(error) => eprintln!("Screen capture: {error}"),
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("Screen capture frame failed: {error}");
        }
    }

    if let Ok(token) = arrived {
        let _ = pool.RemoveFrameArrived(token);
    }
    if let Ok(token) = closed {
        let _ = item.RemoveClosed(token);
    }
    let _ = session.Close();
    let _ = pool.Close();
}
