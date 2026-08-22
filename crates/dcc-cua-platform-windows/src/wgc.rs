use std::time::{Duration, Instant};

use thiserror::Error;
use windows::{
    Graphics::{
        Capture::{
            Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem,
            GraphicsCaptureSession,
        },
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
        SizeInt32,
    },
    Win32::{
        Foundation::HWND,
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0},
            Direct3D11::{
                D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
                D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
                D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
                ID3D11Texture2D,
            },
            Dxgi::IDXGIDevice,
        },
        System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
        System::WinRT::{
            Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
            Graphics::Capture::IGraphicsCaptureItemInterop,
        },
        UI::WindowsAndMessaging::IsIconic,
    },
    core::Interface,
};

const PIXEL_FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;
const FRAME_POOL_SIZE: i32 = 2;
const MAX_CAPTURE_PIXELS: usize = 64 * 1024 * 1024;

#[derive(Debug, Error)]
#[error("persistent WGC capture failed: {0}")]
pub struct WgcCaptureError(String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WgcCompositorTimingUnavailable {
    FrameTimestampUnavailable,
    PerformanceCounterUnavailable,
    TimestampAfterPublish,
    LatencyOverflow,
}

impl WgcCompositorTimingUnavailable {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FrameTimestampUnavailable => "frame_timestamp_unavailable",
            Self::PerformanceCounterUnavailable => "performance_counter_unavailable",
            Self::TimestampAfterPublish => "timestamp_after_publish",
            Self::LatencyOverflow => "latency_overflow",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WgcCompositorTiming {
    Available {
        system_relative_time_100ns: i64,
        compositor_to_publish: Duration,
    },
    Unavailable {
        reason: WgcCompositorTimingUnavailable,
    },
}

pub(crate) fn compositor_timing_from_100ns(
    system_relative_time_100ns: Option<i64>,
    publish_time_100ns: Option<i64>,
) -> WgcCompositorTiming {
    let Some(system_relative_time_100ns) = system_relative_time_100ns else {
        return WgcCompositorTiming::Unavailable {
            reason: WgcCompositorTimingUnavailable::FrameTimestampUnavailable,
        };
    };
    let Some(publish_time_100ns) = publish_time_100ns else {
        return WgcCompositorTiming::Unavailable {
            reason: WgcCompositorTimingUnavailable::PerformanceCounterUnavailable,
        };
    };
    let Some(delta_100ns) = publish_time_100ns.checked_sub(system_relative_time_100ns) else {
        return WgcCompositorTiming::Unavailable {
            reason: WgcCompositorTimingUnavailable::LatencyOverflow,
        };
    };
    let Ok(delta_100ns) = u64::try_from(delta_100ns) else {
        return WgcCompositorTiming::Unavailable {
            reason: WgcCompositorTimingUnavailable::TimestampAfterPublish,
        };
    };
    let Some(nanoseconds) = delta_100ns.checked_mul(100) else {
        return WgcCompositorTiming::Unavailable {
            reason: WgcCompositorTimingUnavailable::LatencyOverflow,
        };
    };
    WgcCompositorTiming::Available {
        system_relative_time_100ns,
        compositor_to_publish: Duration::from_nanos(nanoseconds),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Timings measured around one frame after it is available from the WGC pool.
/// `source_wait` is deliberately separate from GPU/CPU readback work.
pub struct WgcFrameMeasurement {
    pub source_wait: Duration,
    pub readback_total: Duration,
    pub gpu_copy_map: Duration,
    pub cpu_copy: Duration,
    compositor_system_relative_time_100ns: Option<i64>,
}

impl WgcFrameMeasurement {
    pub const fn new(
        source_wait: Duration,
        readback_total: Duration,
        gpu_copy_map: Duration,
        cpu_copy: Duration,
        compositor_system_relative_time_100ns: Option<i64>,
    ) -> Self {
        Self {
            source_wait,
            readback_total,
            gpu_copy_map,
            cpu_copy,
            compositor_system_relative_time_100ns,
        }
    }

    pub(crate) fn at_publish_time_100ns(
        self,
        publish_time_100ns: Option<i64>,
    ) -> WgcPublishedFrameMeasurement {
        WgcPublishedFrameMeasurement {
            source_wait: self.source_wait,
            readback_total: self.readback_total,
            gpu_copy_map: self.gpu_copy_map,
            cpu_copy: self.cpu_copy,
            compositor: compositor_timing_from_100ns(
                self.compositor_system_relative_time_100ns,
                publish_time_100ns,
            ),
        }
    }

    pub fn at_publish(self) -> WgcPublishedFrameMeasurement {
        self.at_publish_time_100ns(performance_counter_time_100ns())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WgcPublishedFrameMeasurement {
    pub source_wait: Duration,
    pub readback_total: Duration,
    pub gpu_copy_map: Duration,
    pub cpu_copy: Duration,
    pub compositor: WgcCompositorTiming,
}

#[derive(Debug)]
pub struct PersistentWgcFrame {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub measurement: WgcFrameMeasurement,
}

fn performance_counter_time_100ns() -> Option<i64> {
    let mut counter = 0_i64;
    let mut frequency = 0_i64;
    unsafe {
        QueryPerformanceCounter(&mut counter).ok()?;
        QueryPerformanceFrequency(&mut frequency).ok()?;
    }
    if counter < 0 || frequency <= 0 {
        return None;
    }
    i64::try_from(
        i128::from(counter)
            .checked_mul(10_000_000)?
            .checked_div(i128::from(frequency))?,
    )
    .ok()
}

fn capture_error(context: &str, error: impl std::fmt::Display) -> WgcCaptureError {
    WgcCaptureError(format!("{context}: {error}"))
}

fn invalid_target(message: impl Into<String>) -> WgcCaptureError {
    WgcCaptureError(message.into())
}

/// One long-lived exact-window WGC session.
///
/// The object is constructed, consumed, and dropped on one blocking worker
/// thread. Reusing its D3D11 device and frame pool avoids the per-frame setup
/// paid by the upstream one-shot screenshot API.
pub struct PersistentWgcCapture {
    process_id: u32,
    window_handle: u64,
    hwnd: HWND,
    d3d_device: ID3D11Device,
    d3d_context: ID3D11DeviceContext,
    direct3d_device: IDirect3DDevice,
    item: GraphicsCaptureItem,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    pool_size: SizeInt32,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
}

impl PersistentWgcCapture {
    pub fn new(process_id: u32, window_handle: u64) -> Result<Self, WgcCaptureError> {
        crate::capture_identity::validate_exact_window_owner(process_id, window_handle)
            .map_err(|error| capture_error("validate exact window owner", error))?;
        let raw_handle = usize::try_from(window_handle)
            .map_err(|error| capture_error("convert exact window handle", error))?;
        let hwnd = HWND(raw_handle as *mut _);
        if unsafe { IsIconic(hwnd) }.as_bool() {
            return Err(invalid_target(
                "WGC cannot capture a minimized window; restore the exact target first",
            ));
        }

        let mut d3d_device = None;
        let mut d3d_context = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                windows::Win32::Foundation::HMODULE(std::ptr::null_mut()),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut d3d_device),
                None,
                Some(&mut d3d_context),
            )
        }
        .map_err(|error| capture_error("create D3D11 device", error))?;
        let d3d_device = d3d_device.ok_or_else(|| invalid_target("D3D11 returned no device"))?;
        let d3d_context =
            d3d_context.ok_or_else(|| invalid_target("D3D11 returned no device context"))?;
        let dxgi_device: IDXGIDevice = d3d_device
            .cast()
            .map_err(|error| capture_error("cast D3D11 device to DXGI", error))?;
        let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device) }
            .map_err(|error| capture_error("wrap DXGI device for WinRT", error))?;
        let direct3d_device: IDirect3DDevice = inspectable
            .cast()
            .map_err(|error| capture_error("cast WinRT D3D11 device", error))?;
        let interop_factory: IGraphicsCaptureItemInterop =
            windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                .map_err(|error| capture_error("get WGC item factory", error))?;
        let item: GraphicsCaptureItem = unsafe { interop_factory.CreateForWindow(hwnd) }
            .map_err(|error| capture_error("create WGC item for exact HWND", error))?;
        let pool_size = item
            .Size()
            .map_err(|error| capture_error("read WGC item size", error))?;
        Self::validate_size(pool_size)?;
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &direct3d_device,
            PIXEL_FORMAT,
            FRAME_POOL_SIZE,
            pool_size,
        )
        .map_err(|error| capture_error("create free-threaded WGC frame pool", error))?;
        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|error| capture_error("create WGC capture session", error))?;
        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetIsCursorCaptureEnabled(false);
        session
            .StartCapture()
            .map_err(|error| capture_error("start WGC capture session", error))?;

        Ok(Self {
            process_id,
            window_handle,
            hwnd,
            d3d_device,
            d3d_context,
            direct3d_device,
            item,
            pool,
            session,
            pool_size,
            staging: None,
        })
    }

    /// Compatibility API returning only pixels and dimensions.
    pub fn next_frame(
        &mut self,
        timeout: Duration,
    ) -> Result<(Vec<u8>, u32, u32), WgcCaptureError> {
        self.next_measured_frame(timeout)
            .map(|frame| (frame.bgra, frame.width, frame.height))
    }

    /// Capture one frame with source-wait, readback, and optional compositor timing.
    pub fn next_measured_frame(
        &mut self,
        timeout: Duration,
    ) -> Result<PersistentWgcFrame, WgcCaptureError> {
        self.validate_owner()?;
        if unsafe { IsIconic(self.hwnd) }.as_bool() {
            return Err(invalid_target(
                "WGC target became minimized; restore the exact target first",
            ));
        }
        self.resize_pool_if_needed()?;
        let deadline = Instant::now() + timeout;
        let source_wait_started = Instant::now();
        loop {
            if let Some(frame) = self.take_latest_available_frame() {
                let frame = self.read_frame(&frame, source_wait_started.elapsed())?;
                self.validate_owner()?;
                return Ok(frame);
            }
            if Instant::now() >= deadline {
                return Err(invalid_target(format!(
                    "no WGC frame arrived within {} ms",
                    timeout.as_millis()
                )));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn validate_owner(&self) -> Result<(), WgcCaptureError> {
        crate::capture_identity::validate_exact_window_owner(self.process_id, self.window_handle)
            .map_err(|error| capture_error("validate exact window owner", error))
    }

    fn validate_size(size: SizeInt32) -> Result<(), WgcCaptureError> {
        if size.Width <= 0 || size.Height <= 0 {
            return Err(invalid_target(format!(
                "WGC target has invalid size {}x{}",
                size.Width, size.Height
            )));
        }
        Ok(())
    }

    fn resize_pool_if_needed(&mut self) -> Result<(), WgcCaptureError> {
        let size = self
            .item
            .Size()
            .map_err(|error| capture_error("refresh WGC item size", error))?;
        Self::validate_size(size)?;
        if size != self.pool_size {
            self.pool
                .Recreate(&self.direct3d_device, PIXEL_FORMAT, FRAME_POOL_SIZE, size)
                .map_err(|error| capture_error("resize WGC frame pool", error))?;
            self.pool_size = size;
            self.staging = None;
        }
        Ok(())
    }

    fn take_latest_available_frame(&self) -> Option<Direct3D11CaptureFrame> {
        let mut latest = None;
        while let Ok(frame) = self.pool.TryGetNextFrame() {
            latest = Some(frame);
        }
        latest
    }

    fn read_frame(
        &mut self,
        frame: &Direct3D11CaptureFrame,
        source_wait: Duration,
    ) -> Result<PersistentWgcFrame, WgcCaptureError> {
        let readback_started = Instant::now();
        let compositor_system_relative_time_100ns = frame
            .SystemRelativeTime()
            .ok()
            .map(|timestamp| timestamp.Duration)
            .filter(|timestamp| *timestamp >= 0);
        let content_size = frame
            .ContentSize()
            .map_err(|error| capture_error("read WGC frame content size", error))?;
        Self::validate_size(content_size)?;
        let surface = frame
            .Surface()
            .map_err(|error| capture_error("read WGC frame surface", error))?;
        let access: IDirect3DDxgiInterfaceAccess = surface
            .cast()
            .map_err(|error| capture_error("cast WGC surface to DXGI access", error))?;
        let texture: ID3D11Texture2D = unsafe { access.GetInterface() }
            .map_err(|error| capture_error("read WGC D3D11 texture", error))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut desc) };
        let width = u32::try_from(content_size.Width)
            .map_err(|error| capture_error("convert WGC width", error))?;
        let height = u32::try_from(content_size.Height)
            .map_err(|error| capture_error("convert WGC height", error))?;
        if width > desc.Width || height > desc.Height {
            return Err(invalid_target("WGC content exceeds its backing texture"));
        }
        let recreate_staging =
            self.staging
                .as_ref()
                .is_none_or(|(_, current_width, current_height)| {
                    *current_width != desc.Width || *current_height != desc.Height
                });
        if recreate_staging {
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            let mut staging = None;
            unsafe {
                self.d3d_device
                    .CreateTexture2D(&desc, None, Some(&mut staging))
            }
            .map_err(|error| capture_error("create reusable WGC staging texture", error))?;
            let staging =
                staging.ok_or_else(|| invalid_target("D3D11 returned no staging texture"))?;
            self.staging = Some((staging, desc.Width, desc.Height));
        }
        let staging = &self
            .staging
            .as_ref()
            .expect("staging texture is initialized")
            .0;
        let gpu_copy_map_started = Instant::now();
        unsafe { self.d3d_context.CopyResource(staging, &texture) };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.d3d_context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(|error| capture_error("map WGC staging texture", error))?;
        let gpu_copy_map = gpu_copy_map_started.elapsed();
        let cpu_copy_started = Instant::now();
        let width = width as usize;
        let height = height as usize;
        let pixel_count = width
            .checked_mul(height)
            .filter(|count| *count <= MAX_CAPTURE_PIXELS)
            .ok_or_else(|| invalid_target("WGC frame exceeds the capture pixel limit"))?;
        let row_bytes = width
            .checked_mul(4)
            .ok_or_else(|| invalid_target("WGC row byte count overflowed"))?;
        let stride = mapped.RowPitch as usize;
        if stride < row_bytes {
            unsafe { self.d3d_context.Unmap(staging, 0) };
            return Err(invalid_target("WGC row pitch is smaller than its content"));
        }
        let mut bgra = vec![0_u8; pixel_count * 4];
        let source = mapped.pData as *const u8;
        for row in 0..height {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    source.add(row * stride),
                    bgra.as_mut_ptr().add(row * row_bytes),
                    row_bytes,
                )
            };
        }
        unsafe { self.d3d_context.Unmap(staging, 0) };
        let cpu_copy = cpu_copy_started.elapsed();
        let readback_total = readback_started.elapsed();
        Ok(PersistentWgcFrame {
            bgra,
            width: width as u32,
            height: height as u32,
            measurement: WgcFrameMeasurement::new(
                source_wait,
                readback_total,
                gpu_copy_map,
                cpu_copy,
                compositor_system_relative_time_100ns,
            ),
        })
    }
}

impl Drop for PersistentWgcCapture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
