//! Native screen-change notifications with bounded, latest-per-region delivery.
use crate::backend::Region;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::JoinHandle;

/// A uniquely named rectangle in physical desktop pixels (including negative origins).
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenRegion {
    pub name: String,
    pub region: Region,
}

/// Tightly packed RGB pixels, in top-to-bottom row order.
#[derive(Clone, Debug, PartialEq)]
pub struct RegionFrame {
    pub name: String,
    pub region: Region,
    pub pixels: Vec<u8>,
}

#[derive(Default)]
struct Pending {
    slots: Vec<Option<RegionFrame>>,
    error: Option<String>,
}

impl Pending {
    #[cfg(any(test, all(windows, feature = "gui")))]
    fn publish(&mut self, index: usize, frame: RegionFrame) {
        self.slots[index] = Some(frame);
    }

    fn take(&mut self) -> Result<Vec<RegionFrame>, String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        Ok(self.slots.iter_mut().filter_map(Option::take).collect())
    }
}

pub struct ScreenEvents {
    cancel: Arc<AtomicBool>,
    pending: Arc<Mutex<Pending>>,
    workers: Vec<JoinHandle<()>>,
}

impl ScreenEvents {
    /// Starts one Desktop Duplication worker per selected display on Windows.
    /// Initialization/capture failures are sticky and wake the caller immediately.
    /// Regions must be nonempty and fit entirely within one unrotated display.
    /// Other platforms (and builds without `gui`) have no polling fallback.
    pub fn start(
        regions: Vec<ScreenRegion>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        #[cfg(all(windows, feature = "gui"))]
        {
            native::start(regions, wake)
        }
        #[cfg(not(all(windows, feature = "gui")))]
        {
            let _ = (regions, wake);
            Err("Native screen events require Windows and the gui feature".into())
        }
    }

    /// Drains the latest changed snapshot of each region, or reports capture loss.
    pub fn take_pending(&self) -> Result<Vec<RegionFrame>, String> {
        self.pending
            .lock()
            .map_err(|_| "Screen event buffer poisoned".to_string())?
            .take()
    }

    /// Cancels and joins every worker before clearing all buffered snapshots.
    pub fn stop(&mut self) -> Result<(), String> {
        self.cancel.store(true, Ordering::Release);
        let mut join_error = None;
        for worker in self.workers.drain(..) {
            if worker.join().is_err() {
                join_error = Some("Screen event worker panicked".to_string());
            }
        }
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| "Screen event buffer poisoned".to_string())?;
        pending.slots.iter_mut().for_each(|slot| *slot = None);
        if pending.error.is_none() {
            pending.error = join_error;
        }
        match &pending.error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }
}

impl Drop for ScreenEvents {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            log::error!("Stopping screen events: {error}");
        }
    }
}

#[cfg(any(test, all(windows, feature = "gui")))]
fn changed(previous: &mut Option<Vec<u8>>, pixels: &[u8]) -> bool {
    if previous.as_deref() == Some(pixels) {
        return false;
    }
    *previous = Some(pixels.to_vec());
    true
}

#[cfg(any(test, all(windows, feature = "gui")))]
fn crop_bgra(
    bytes: &[u8],
    pitch: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) -> Result<Vec<u8>, String> {
    let row_end = x.checked_add(width).and_then(|v| v.checked_mul(4));
    let end = y
        .checked_add(height)
        .and_then(|v| v.checked_sub(1))
        .and_then(|v| v.checked_mul(pitch))
        .and_then(|v| v.checked_add(row_end?));
    let length = width.checked_mul(height).and_then(|v| v.checked_mul(3));
    if width == 0
        || height == 0
        || row_end.is_none_or(|end| end > pitch)
        || end.is_none_or(|end| end > bytes.len())
        || length.is_none()
    {
        return Err("Invalid mapped texture bounds".into());
    }
    let mut rgb = Vec::with_capacity(length.unwrap());
    for row in y..y + height {
        let start = row * pitch + x * 4;
        for pixel in bytes[start..start + width * 4].chunks_exact(4) {
            rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        }
    }
    Ok(rgb)
}

#[cfg(all(windows, feature = "gui"))]
mod native {
    use super::*;
    use std::{collections::HashSet, panic::AssertUnwindSafe};
    use windows::{
        core::Interface,
        Win32::{
            Graphics::{
                Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL},
                Direct3D11::*,
                Dxgi::{Common::*, *},
            },
            UI::HiDpi::{
                SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            },
        },
    };

    #[derive(Clone)]
    struct Display {
        adapter: u32,
        output: u32,
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        rotated: bool,
    }

    struct PhysicalCoordinates(DPI_AWARENESS_CONTEXT);
    impl PhysicalCoordinates {
        fn enter() -> Result<Self, String> {
            let previous =
                unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
            if previous.0.is_null() {
                return Err("Cannot enable physical-pixel coordinates on capture thread".into());
            }
            Ok(Self(previous))
        }
    }
    impl Drop for PhysicalCoordinates {
        fn drop(&mut self) {
            if unsafe { SetThreadDpiAwarenessContext(self.0) }.0.is_null() {
                log::error!("Restoring capture thread DPI awareness failed");
            }
        }
    }

    fn displays() -> Result<Vec<Display>, String> {
        let _coordinates = PhysicalCoordinates::enter()?;
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(|e| e.to_string())?;
            let mut displays = Vec::new();
            let mut adapter_index = 0;
            loop {
                let adapter = match factory.EnumAdapters1(adapter_index) {
                    Ok(adapter) => adapter,
                    Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                    Err(error) => return Err(error.to_string()),
                };
                let mut output_index = 0;
                loop {
                    let output = match adapter.EnumOutputs(output_index) {
                        Ok(output) => output,
                        Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                        Err(error) => return Err(error.to_string()),
                    };
                    let desc = output.GetDesc().map_err(|e| e.to_string())?;
                    if desc.AttachedToDesktop.as_bool() {
                        displays.push(Display {
                            adapter: adapter_index,
                            output: output_index,
                            left: desc.DesktopCoordinates.left,
                            top: desc.DesktopCoordinates.top,
                            right: desc.DesktopCoordinates.right,
                            bottom: desc.DesktopCoordinates.bottom,
                            rotated: desc.Rotation != DXGI_MODE_ROTATION_IDENTITY
                                && desc.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED,
                        });
                    }
                    output_index += 1;
                }
                adapter_index += 1;
            }
            Ok(displays)
        }
    }

    pub(super) fn start(
        regions: Vec<ScreenRegion>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<ScreenEvents, String> {
        // Discovery COM interfaces are created and dropped on this thread; only
        // plain display coordinates and enumeration indices leave it.
        let displays = std::thread::Builder::new()
            .name("screen-display-discovery".into())
            .spawn(displays)
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|_| "Display discovery panicked".to_string())??;
        let mut groups: Vec<Vec<(usize, ScreenRegion)>> = vec![Vec::new(); displays.len()];
        let mut names = HashSet::new();
        for (index, region) in regions.iter().enumerate() {
            if !names.insert(&region.name) {
                return Err(format!("Duplicate screen region name: {}", region.name));
            }
            let r = region.region;
            let display_index = displays
                .iter()
                .position(|d| {
                    r.width > 0
                        && r.height > 0
                        && r.x >= d.left
                        && r.y >= d.top
                        && i64::from(r.x) + i64::from(r.width) <= i64::from(d.right)
                        && i64::from(r.y) + i64::from(r.height) <= i64::from(d.bottom)
                })
                .ok_or_else(|| {
                    format!(
                        "Region '{}' must fit wholly in one attached display",
                        region.name
                    )
                })?;
            if displays[display_index].rotated {
                return Err(format!(
                    "Region '{}' uses an unsupported rotated display",
                    region.name
                ));
            }
            groups[display_index].push((index, region.clone()));
        }
        let mut events = ScreenEvents {
            cancel: Arc::new(AtomicBool::new(false)),
            pending: Arc::new(Mutex::new(Pending {
                slots: vec![None; regions.len()],
                error: None,
            })),
            workers: Vec::new(),
        };
        for (display, regions) in displays.into_iter().zip(groups) {
            if regions.is_empty() {
                continue;
            }
            let cancel = events.cancel.clone();
            let pending = events.pending.clone();
            let wake = wake.clone();
            let worker = std::thread::Builder::new()
                .name(format!(
                    "screen-events-{}-{}",
                    display.adapter, display.output
                ))
                .spawn(move || {
                    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        capture(display, regions, &cancel, &pending, &wake)
                    }))
                    .unwrap_or_else(|_| Err("Screen event worker panicked".into()));
                    if let Err(error) = result {
                        cancel.store(true, Ordering::Release);
                        match pending.lock() {
                            Ok(mut pending) => {
                                if pending.error.is_none() {
                                    pending.error = Some(error);
                                }
                            }
                            Err(_) => log::error!("Screen event buffer poisoned: {error}"),
                        }
                        wake();
                    }
                })
                .map_err(|e| format!("Starting screen event worker: {e}"))?;
            events.workers.push(worker);
        }
        Ok(events)
    }

    struct Frame<'a>(&'a IDXGIOutputDuplication, bool);
    impl Frame<'_> {
        fn release(mut self) -> Result<(), String> {
            self.1 = false;
            unsafe { self.0.ReleaseFrame().map_err(|e| e.to_string()) }
        }
    }
    impl Drop for Frame<'_> {
        fn drop(&mut self) {
            if self.1 {
                if let Err(error) = unsafe { self.0.ReleaseFrame() } {
                    log::error!("Releasing desktop duplication frame: {error}");
                }
            }
        }
    }

    struct Mapping<'a>(&'a ID3D11DeviceContext, &'a ID3D11Texture2D);
    impl Drop for Mapping<'_> {
        fn drop(&mut self) {
            unsafe { self.0.Unmap(self.1, 0) }
        }
    }

    fn capture(
        display: Display,
        regions: Vec<(usize, ScreenRegion)>,
        cancel: &AtomicBool,
        pending: &Mutex<Pending>,
        wake: &Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), String> {
        let _coordinates = PhysicalCoordinates::enter()?;
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(|e| e.to_string())?;
            let adapter = factory
                .EnumAdapters1(display.adapter)
                .map_err(|e| e.to_string())?;
            let output: IDXGIOutput1 = adapter
                .EnumOutputs(display.output)
                .and_then(|output| output.cast())
                .map_err(|e| e.to_string())?;
            let desc = output.GetDesc().map_err(|e| e.to_string())?;
            let rect = desc.DesktopCoordinates;
            if !desc.AttachedToDesktop.as_bool()
                || rect.left != display.left
                || rect.top != display.top
                || rect.right != display.right
                || rect.bottom != display.bottom
                || (desc.Rotation != DXGI_MODE_ROTATION_IDENTITY
                    && desc.Rotation != DXGI_MODE_ROTATION_UNSPECIFIED)
            {
                return Err("Display layout changed during screen event initialization".into());
            }
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None::<&[D3D_FEATURE_LEVEL]>,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(|e| format!("Creating capture device: {e}"))?;
            let device = device.ok_or("No D3D11 device returned")?;
            let context = context.ok_or("No D3D11 context returned")?;
            let duplication = output
                .DuplicateOutput(&device)
                .map_err(|e| format!("Duplicating display output: {e}"))?;
            let mut previous = vec![None; regions.len()];
            let mut staging: Option<ID3D11Texture2D> = None;
            while !cancel.load(Ordering::Acquire) {
                let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
                let mut resource = None;
                match duplication.AcquireNextFrame(50, &mut info, &mut resource) {
                    Ok(()) => {}
                    Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => continue,
                    Err(error) => return Err(format!("Desktop duplication capture lost: {error}")),
                }
                let frame = Frame(&duplication, true);
                if cancel.load(Ordering::Acquire) {
                    frame.release()?;
                    break;
                }
                let texture: ID3D11Texture2D = resource
                    .ok_or("Desktop duplication returned no texture")?
                    .cast()
                    .map_err(|e| e.to_string())?;
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                texture.GetDesc(&mut desc);
                if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM
                    || i64::from(desc.Width) != i64::from(display.right) - i64::from(display.left)
                    || i64::from(desc.Height) != i64::from(display.bottom) - i64::from(display.top)
                {
                    return Err(format!(
                        "Desktop duplication format or display dimensions changed: texture {}x{} {:?}, display {}x{}",
                        desc.Width, desc.Height, desc.Format,
                        i64::from(display.right) - i64::from(display.left),
                        i64::from(display.bottom) - i64::from(display.top),
                    ));
                }
                if staging.is_none() {
                    desc.Usage = D3D11_USAGE_STAGING;
                    desc.BindFlags = 0;
                    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                    desc.MiscFlags = 0;
                    device
                        .CreateTexture2D(&desc, None, Some(&mut staging))
                        .map_err(|e| e.to_string())?;
                }
                let staging = staging.as_ref().ok_or("No staging texture returned")?;
                context.CopyResource(staging, &texture);
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                context
                    .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                    .map_err(|e| format!("Mapping captured desktop: {e}"))?;
                let mapping = Mapping(&context, staging);
                let length = (desc.Height as usize)
                    .checked_sub(1)
                    .and_then(|rows| rows.checked_mul(mapped.RowPitch as usize))
                    .and_then(|offset| offset.checked_add((desc.Width as usize).checked_mul(4)?))
                    .filter(|length| *length <= isize::MAX as usize)
                    .ok_or("Mapped desktop texture is too large")?;
                if mapped.pData.is_null() || (mapped.RowPitch as u64) < u64::from(desc.Width) * 4 {
                    return Err("Invalid desktop texture mapping".into());
                }
                let bytes = std::slice::from_raw_parts(mapped.pData.cast::<u8>(), length);
                let mut updates = Vec::new();
                for (local_index, (index, region)) in regions.iter().enumerate() {
                    let r = region.region;
                    let pixels = crop_bgra(
                        bytes,
                        mapped.RowPitch as usize,
                        (i64::from(r.x) - i64::from(display.left)) as usize,
                        (i64::from(r.y) - i64::from(display.top)) as usize,
                        r.width as usize,
                        r.height as usize,
                    )?;
                    if changed(&mut previous[local_index], &pixels) {
                        updates.push((
                            *index,
                            RegionFrame {
                                name: region.name.clone(),
                                region: r,
                                pixels,
                            },
                        ));
                    }
                }
                drop(mapping);
                frame.release()?;
                if !updates.is_empty() {
                    let mut pending = pending
                        .lock()
                        .map_err(|_| "Screen event buffer poisoned".to_string())?;
                    for (index, frame) in updates {
                        pending.publish(index, frame);
                    }
                    drop(pending);
                    wake();
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_padded_rows_and_converts_bgra_to_rgb() {
        let bytes = [
            1, 2, 3, 255, 4, 5, 6, 255, 99, 99, 99, 99, 7, 8, 9, 255, 10, 11, 12, 255, 99, 99, 99,
            99,
        ];
        assert_eq!(
            crop_bgra(&bytes, 12, 1, 0, 1, 2).unwrap(),
            [6, 5, 4, 12, 11, 10]
        );
        assert!(crop_bgra(&bytes, 12, 3, 0, 1, 1).is_err());
        assert!(crop_bgra(&bytes[..18], 12, 1, 0, 1, 2).is_err());
        assert!(crop_bgra(&bytes, 12, usize::MAX, 0, 1, 1).is_err());
    }

    #[test]
    fn first_and_exact_changes_only() {
        let mut previous = None;
        assert!(changed(&mut previous, &[1, 2, 3]));
        assert!(!changed(&mut previous, &[1, 2, 3]));
        assert!(changed(&mut previous, &[1, 2, 4]));
        assert!(changed(&mut previous, &[1, 2, 3]));
    }

    #[test]
    fn coalesces_latest_per_region_and_keeps_errors_sticky() {
        let mut pending = Pending {
            slots: vec![None; 2],
            error: None,
        };
        let frame = |value| RegionFrame {
            name: "region".into(),
            region: Region::default(),
            pixels: vec![value],
        };
        pending.publish(0, frame(1));
        pending.publish(1, frame(2));
        pending.publish(0, frame(3));
        assert_eq!(pending.take().unwrap(), vec![frame(3), frame(2)]);
        assert!(pending.take().unwrap().is_empty());
        pending.error = Some("capture lost".into());
        assert_eq!(pending.take().unwrap_err(), "capture lost");
        assert_eq!(pending.take().unwrap_err(), "capture lost");
    }

    #[cfg(not(all(windows, feature = "gui")))]
    #[test]
    fn unsupported_without_native_backend() {
        assert!(ScreenEvents::start(vec![], Arc::new(|| {})).is_err());
    }

    #[cfg(all(windows, feature = "gui"))]
    #[test]
    #[ignore = "requires an interactive Windows desktop; briefly shows a tiny non-activating solid-color window"]
    fn native_start_stop_smoke() {
        use windows::{
            core::PCWSTR,
            Win32::{
                Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
                Graphics::Gdi::*,
                System::LibraryLoader::GetModuleHandleW,
                UI::{HiDpi::*, WindowsAndMessaging::*},
            },
        };

        struct TestWindow {
            hwnd: HWND,
            instance: HINSTANCE,
            class: Vec<u16>,
            registered: bool,
            brushes: [HBRUSH; 2],
            previous_dpi: DPI_AWARENESS_CONTEXT,
        }

        impl Drop for TestWindow {
            fn drop(&mut self) {
                unsafe {
                    if !self.hwnd.0.is_null() {
                        if let Err(error) = DestroyWindow(self.hwnd) {
                            log::error!("Destroying native smoke window: {error}");
                        }
                    }
                    if self.registered {
                        if let Err(error) =
                            UnregisterClassW(PCWSTR(self.class.as_ptr()), self.instance)
                        {
                            log::error!("Unregistering native smoke window: {error}");
                        }
                    }
                    for brush in self.brushes {
                        if !brush.0.is_null() && !DeleteObject(brush).as_bool() {
                            log::error!("Deleting native smoke window brush failed");
                        }
                    }
                    if !self.previous_dpi.0.is_null()
                        && SetThreadDpiAwarenessContext(self.previous_dpi).0.is_null()
                    {
                        log::error!("Restoring native smoke thread DPI awareness failed");
                    }
                }
            }
        }

        unsafe extern "system" fn window_proc(
            hwnd: HWND,
            message: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            unsafe {
                if message == WM_ERASEBKGND {
                    let brush = HBRUSH(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut _);
                    let mut rect = RECT::default();
                    if !brush.0.is_null() && GetClientRect(hwnd, &mut rect).is_ok() {
                        return LRESULT(
                            (FillRect(HDC(wparam.0 as *mut _), &rect, brush) != 0) as isize,
                        );
                    }
                    return LRESULT(0);
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
        }

        fn pump_messages() {
            unsafe {
                let mut message = MSG::default();
                for _ in 0..32 {
                    if !PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                        break;
                    }
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }

        fn repaint(window: &TestWindow, color: usize) {
            unsafe {
                let brush = window.brushes[color].0 as isize;
                SetWindowLongPtrW(window.hwnd, GWLP_USERDATA, brush);
                assert_eq!(
                    GetWindowLongPtrW(window.hwnd, GWLP_USERDATA),
                    brush,
                    "Setting test color failed",
                );
                assert!(RedrawWindow(
                    window.hwnd,
                    None,
                    None,
                    RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW,
                )
                .as_bool());
            }
        }

        fn require_color(
            events: &ScreenEvents,
            wakes: &std::sync::atomic::AtomicUsize,
            color: [u8; 3],
            previous_wakes: usize,
        ) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            let mut matched = false;
            while std::time::Instant::now() < deadline {
                pump_messages();
                // Never format pixel buffers, even on an assertion failure.
                for frame in events.take_pending().expect("Native capture failed") {
                    if frame.pixels.len() == 4 * 4 * 3
                        && frame.pixels.chunks_exact(3).all(|pixel| pixel == color)
                    {
                        matched = true;
                    }
                }
                if matched && wakes.load(Ordering::Acquire) > previous_wakes {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("Timed out waiting for controlled native window color");
        }

        let colors = [[37u8, 83, 149], [181, 59, 103]];
        let mut window = TestWindow {
            hwnd: HWND::default(),
            instance: HINSTANCE::default(),
            class: format!(
                "ScreenAutomationNativeSmoke_{}_{:?}\0",
                std::process::id(),
                std::thread::current().id()
            )
            .encode_utf16()
            .collect(),
            registered: false,
            brushes: [HBRUSH::default(); 2],
            previous_dpi: DPI_AWARENESS_CONTEXT::default(),
        };
        unsafe {
            window.previous_dpi =
                SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            assert!(
                !window.previous_dpi.0.is_null(),
                "Setting test DPI awareness failed"
            );
            window.instance = GetModuleHandleW(None).unwrap().into();
            for (index, color) in colors.iter().enumerate() {
                window.brushes[index] = CreateSolidBrush(COLORREF(
                    u32::from(color[0]) | u32::from(color[1]) << 8 | u32::from(color[2]) << 16,
                ));
                assert!(
                    !window.brushes[index].0.is_null(),
                    "Creating test brush failed"
                );
            }
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: window.instance,
                lpszClassName: PCWSTR(window.class.as_ptr()),
                ..Default::default()
            };
            assert_ne!(RegisterClassW(&class), 0, "Registering test window failed");
            window.registered = true;
            window.hwnd = CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                PCWSTR(window.class.as_ptr()),
                PCWSTR::null(),
                WS_POPUP,
                64,
                64,
                32,
                32,
                None,
                None,
                window.instance,
                None,
            )
            .expect("Creating non-activating native test window failed");
            let _ = ShowWindow(window.hwnd, SW_SHOWNOACTIVATE);
        }
        let mut client = windows::Win32::Foundation::POINT { x: 8, y: 8 };
        assert!(unsafe { ClientToScreen(window.hwnd, &mut client) }.as_bool());
        for _ in 0..3 {
            repaint(&window, 0);
            let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let signal = wakes.clone();
            let mut events = ScreenEvents::start(
                vec![ScreenRegion {
                    name: "smoke".into(),
                    region: Region {
                        x: client.x,
                        y: client.y,
                        width: 4,
                        height: 4,
                    },
                }],
                Arc::new(move || {
                    signal.fetch_add(1, Ordering::Release);
                }),
            )
            .unwrap();
            require_color(&events, &wakes, colors[0], 0);
            let first_wakes = wakes.load(Ordering::Acquire);
            repaint(&window, 1);
            require_color(&events, &wakes, colors[1], first_wakes);
            events.stop().unwrap();
            assert!(events.take_pending().unwrap().is_empty());
        }
    }
}
