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
    pub fn new(window_handle: u64) -> Result<Self, WgcCaptureError> {
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

    pub fn next_frame(
        &mut self,
        timeout: Duration,
    ) -> Result<(Vec<u8>, u32, u32), WgcCaptureError> {
        if unsafe { IsIconic(self.hwnd) }.as_bool() {
            return Err(invalid_target(
                "WGC target became minimized; restore the exact target first",
            ));
        }
        self.resize_pool_if_needed()?;
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(frame) = self.take_latest_available_frame() {
                return self.read_frame(&frame);
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
    ) -> Result<(Vec<u8>, u32, u32), WgcCaptureError> {
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
        unsafe { self.d3d_context.CopyResource(staging, &texture) };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.d3d_context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(|error| capture_error("map WGC staging texture", error))?;
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
        Ok((bgra, width as u32, height as u32))
    }
}

impl Drop for PersistentWgcCapture {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
