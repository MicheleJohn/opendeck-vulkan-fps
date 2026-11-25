use std::collections::HashMap;
use std::env;
use std::ffi::{CStr, c_char, c_void};
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

type VkResult = i32;
type VkInstance = *mut c_void;
type VkDevice = *mut c_void;
type VkQueue = *mut c_void;
type PfnVkVoidFunction = Option<unsafe extern "system" fn()>;
type PfnGetInstanceProcAddr = unsafe extern "system" fn(VkInstance, *const c_char) -> PfnVkVoidFunction;
type PfnGetDeviceProcAddr = unsafe extern "system" fn(VkDevice, *const c_char) -> PfnVkVoidFunction;
type PfnQueuePresentKHR = unsafe extern "system" fn(VkQueue, *const c_void) -> VkResult;

#[repr(C)]
pub struct VkLayerInstanceCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub function: i32,
    pub u: *mut VkLayerInstanceLink,
}

#[repr(C)]
pub struct VkLayerInstanceLink {
    pub p_next: *mut VkLayerInstanceLink,
    pub pfn_next_get_instance_proc_addr: Option<PfnGetInstanceProcAddr>,
    pub pfn_next_get_physical_device_proc_addr: *const c_void,
}

#[repr(C)]
pub struct VkLayerDeviceCreateInfo {
    pub s_type: i32,
    pub p_next: *const c_void,
    pub function: i32,
    pub u: *mut VkLayerDeviceLink,
}

#[repr(C)]
pub struct VkLayerDeviceLink {
    pub p_next: *mut VkLayerDeviceLink,
    pub pfn_next_get_instance_proc_addr: Option<PfnGetInstanceProcAddr>,
    pub pfn_next_get_device_proc_addr: Option<PfnGetDeviceProcAddr>,
}

#[repr(C)]
struct GenericHeader {
    s_type: i32,
    p_next: *const c_void,
}

static NEXT_GIPA: OnceLock<PfnGetInstanceProcAddr> = OnceLock::new();
static NEXT_GDPA: OnceLock<PfnGetDeviceProcAddr> = OnceLock::new();
static PRESENT_MAP: OnceLock<Mutex<HashMap<usize, PfnQueuePresentKHR>>> = OnceLock::new();
static DEFAULT_PRESENT: OnceLock<PfnQueuePresentKHR> = OnceLock::new();

static FRAME_COUNT: AtomicU64 = AtomicU64::new(0);
static LAST_TICK_MS: AtomicU64 = AtomicU64::new(0);

fn should_enable_layer() -> bool {
    // Se ENABLE_SD_FPS=1 è presente (modalità manuale), attiva subito ignorando DISABLE_SD_FPS
    if env::var("ENABLE_SD_FPS").as_deref() == Ok("1") {
        return true;
    }

    // In modalità auto-detect, rispetta DISABLE_SD_FPS=1 per escludere singoli giochi
    if env::var("DISABLE_SD_FPS").as_deref() == Ok("1") {
        return false;
    }

    let raw_cmd = fs::read("/proc/self/cmdline").unwrap_or_default();
    let cmd = String::from_utf8_lossy(&raw_cmd)
    .replace('\0', " ")
    .to_lowercase();

    const BLACKLIST: &[&str] = &[
        "plasmashell", "kwin", "gnome-shell", "waybar", "opendeck",
        "chrome", "chromium", "firefox", "brave", "vivaldi", "zen-bin",
        "discord", "vesktop", "steamwebhelper", "obs", "mpv", "vlc",
        "konsole", "kitty", "alacritty", "ptyxis",
        "explorer.exe", "services.exe", "winedevice.exe", "plugplay.exe",
        "svchost.exe", "rpcss.exe", "tabtip.exe", "conhost.exe", "crashpad_handler.exe",
    ];
    if BLACKLIST.iter().any(|b| cmd.contains(b)) {
        return false;
    }

    let is_windows_game = cmd.contains(".exe")
    || env::var_os("WINELOADER").is_some()
    || env::var_os("STEAM_COMPAT_DATA_PATH").is_some();

    let is_native_steam_game = env::var("SteamAppId")
    .map(|id| !id.is_empty() && id != "0")
    .unwrap_or(false);

    let is_gamescope = env::var_os("GAMESCOPE_WAYLAND_DISPLAY").is_some();

    is_windows_game || is_native_steam_game || is_gamescope
}

fn present_map() -> &'static Mutex<HashMap<usize, PfnQueuePresentKHR>> {
    PRESENT_MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn dispatch_key(handle: *mut c_void) -> usize {
    if handle.is_null() {
        return 0;
    }
    unsafe { *(handle as *const usize) }
}

fn now_ms() -> u64 {
    SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_millis() as u64)
    .unwrap_or(0)
}

fn write_fps_atomic(fps: u64, ts_sec: u64) {
    let uid = unsafe { libc::geteuid() };
    let pid = std::process::id();
    let tmp_path = format!("/dev/shm/sd_fps_{}.tmp", uid);
    let final_path = format!("/dev/shm/sd_fps_{}", uid);
    if fs::write(&tmp_path, format!("{}\n{}\n{}", fps, ts_sec, pid)).is_ok() {
        let _ = fs::rename(&tmp_path, &final_path);
    }
}

unsafe extern "system" fn hooked_queue_present_khr(queue: VkQueue, p_present_info: *const c_void) -> VkResult {
    FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
    let now = now_ms();
    let last = LAST_TICK_MS.load(Ordering::Relaxed);

    if last == 0 {
        LAST_TICK_MS.store(now, Ordering::Relaxed);
    } else if now.saturating_sub(last) >= 1000
        && LAST_TICK_MS
        .compare_exchange(last, now, Ordering::SeqCst, Ordering::Relaxed)
        .is_ok()
        {
            let count = FRAME_COUNT.swap(0, Ordering::Relaxed);
            let elapsed = now - last;
            let fps = ((count as f64) * 1000.0 / (elapsed as f64)).round() as u64;
            write_fps_atomic(fps, now / 1000);
        }

        let key = dispatch_key(queue);
        let next_fn = present_map()
        .lock()
        .ok()
        .and_then(|m| m.get(&key).copied())
        .or_else(|| DEFAULT_PRESENT.get().copied());

        match next_fn {
            Some(next) => unsafe { next(queue, p_present_info) },
            None => 0,
        }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn sd_fps_CreateInstance(
    p_create_info: *const c_void,
    p_allocator: *const c_void,
    p_instance: *mut VkInstance,
) -> VkResult {
    if !should_enable_layer() {
        return -3;
    }

    let mut header = unsafe { (*(p_create_info as *const GenericHeader)).p_next as *mut VkLayerInstanceCreateInfo };
    let mut next_gipa = None;

    while !header.is_null() {
        unsafe {
            if (*header).s_type == 47 && (*header).function == 0 {
                let link = (*header).u;
                if !link.is_null() {
                    next_gipa = (*link).pfn_next_get_instance_proc_addr;
                    (*header).u = (*link).p_next;
                }
                break;
            }
            header = (*header).p_next as *mut VkLayerInstanceCreateInfo;
        }
    }

    let Some(gipa) = next_gipa else { return -3 };
    let _ = NEXT_GIPA.set(gipa);

    unsafe {
        if let Some(raw_create) = gipa(std::ptr::null_mut(), c"vkCreateInstance".as_ptr()) {
            let create_fn: unsafe extern "system" fn(*const c_void, *const c_void, *mut VkInstance) -> VkResult =
            std::mem::transmute(raw_create);
            create_fn(p_create_info, p_allocator, p_instance)
        } else {
            -3
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn sd_fps_CreateDevice(
    physical_device: VkInstance,
    p_create_info: *const c_void,
    p_allocator: *const c_void,
    p_device: *mut VkDevice,
) -> VkResult {
    let mut header = unsafe { (*(p_create_info as *const GenericHeader)).p_next as *mut VkLayerDeviceCreateInfo };
    let mut next_gipa = None;
    let mut next_gdpa = None;

    while !header.is_null() {
        unsafe {
            if (*header).s_type == 48 && (*header).function == 0 {
                let link = (*header).u;
                if !link.is_null() {
                    next_gipa = (*link).pfn_next_get_instance_proc_addr;
                    next_gdpa = (*link).pfn_next_get_device_proc_addr;
                    (*header).u = (*link).p_next;
                }
                break;
            }
            header = (*header).p_next as *mut VkLayerDeviceCreateInfo;
        }
    }

    let (Some(gipa), Some(gdpa)) = (next_gipa, next_gdpa) else { return -3 };
    let _ = NEXT_GDPA.set(gdpa);

    unsafe {
        let res = if let Some(raw_create) = gipa(physical_device, c"vkCreateDevice".as_ptr()) {
            let create_fn: unsafe extern "system" fn(VkInstance, *const c_void, *const c_void, *mut VkDevice) -> VkResult =
            std::mem::transmute(raw_create);
            create_fn(physical_device, p_create_info, p_allocator, p_device)
        } else {
            return -3;
        };

        if res == 0 && !p_device.is_null() && !(*p_device).is_null()
            && let Some(raw_present) = gdpa(*p_device, c"vkQueuePresentKHR".as_ptr())
            {
                let present_fn: PfnQueuePresentKHR = std::mem::transmute(raw_present);
                let _ = DEFAULT_PRESENT.set(present_fn);
                if let Ok(mut map) = present_map().lock() {
                    map.insert(dispatch_key(*p_device), present_fn);
                }
            }
            res
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn sd_fps_GetDeviceProcAddr(device: VkDevice, p_name: *const c_char) -> PfnVkVoidFunction {
    if p_name.is_null() {
        return None;
    }
    unsafe {
        match CStr::from_ptr(p_name).to_bytes() {
            b"vkGetDeviceProcAddr" => Some(std::mem::transmute(sd_fps_GetDeviceProcAddr as *const ())),
            b"vkQueuePresentKHR" => Some(std::mem::transmute(hooked_queue_present_khr as *const ())),
            _ => NEXT_GDPA.get().and_then(|f| f(device, p_name)),
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn sd_fps_GetInstanceProcAddr(instance: VkInstance, p_name: *const c_char) -> PfnVkVoidFunction {
    if p_name.is_null() {
        return None;
    }
    unsafe {
        match CStr::from_ptr(p_name).to_bytes() {
            b"vkGetInstanceProcAddr" => Some(std::mem::transmute(sd_fps_GetInstanceProcAddr as *const ())),
            b"vkCreateInstance" => Some(std::mem::transmute(sd_fps_CreateInstance as *const ())),
            b"vkCreateDevice" => Some(std::mem::transmute(sd_fps_CreateDevice as *const ())),
            b"vkGetDeviceProcAddr" => Some(std::mem::transmute(sd_fps_GetDeviceProcAddr as *const ())),
            _ => NEXT_GIPA.get().and_then(|f| f(instance, p_name)),
        }
    }
}
