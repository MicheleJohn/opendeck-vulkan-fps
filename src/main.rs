use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tungstenite::{Message, connect, stream::MaybeTlsStream};

static RUNNING: AtomicBool = AtomicBool::new(true);

extern "C" fn handle_shutdown_signal(_sig: libc::c_int) {
    RUNNING.store(false, Ordering::SeqCst);
}

fn install_signal_handlers() {
    unsafe {
        libc::signal(libc::SIGTERM, handle_shutdown_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, handle_shutdown_signal as *const () as libc::sighandler_t);
    }
}

fn layer_json_path() -> Option<PathBuf> {
    let home = env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".local/share/vulkan/implicit_layer.d/VK_LAYER_SD_fps.json"))
}

fn cleanup_on_exit() {
    thread::sleep(Duration::from_millis(300));
    let uid = unsafe { libc::geteuid() };
    let _ = fs::remove_file(format!("/dev/shm/sd_fps_{}", uid));
    let _ = fs::remove_file(format!("/dev/shm/sd_fps_{}.tmp", uid));

    let plugin_missing = env::current_exe()
    .ok()
    .and_then(|p| p.parent().map(|d| !d.join("libvk_layer_sd_fps.so").exists()))
    .unwrap_or(true);

    if plugin_missing {
        if let Some(json_path) = layer_json_path() {
            let _ = fs::remove_file(json_path);
        }
    }
}

fn config_file_path() -> Option<PathBuf> {
    env::current_exe().ok()?.parent().map(|d| d.join("config.json"))
}

fn load_auto_detect_config() -> bool {
    let Some(path) = config_file_path() else { return true };
    fs::read_to_string(path)
    .map(|s| !s.contains("\"ENABLE_AUTODETECT\":false"))
    .unwrap_or(true)
}

fn save_auto_detect_config(enable_autodetect: bool) {
    if let Some(path) = config_file_path() {
        let _ = fs::write(path, format!("{{\"ENABLE_AUTODETECT\":{}}}", enable_autodetect));
    }
}

fn sync_vulkan_layer(enable_autodetect: bool) {
    save_auto_detect_config(enable_autodetect);

    let Ok(home) = env::var("HOME") else { return };
    let Ok(exe_path) = env::current_exe() else { return };
    let Some(plugin_dir) = exe_path.parent() else { return };
    let bundled_so = plugin_dir.join("libvk_layer_sd_fps.so");
    if !bundled_so.exists() {
        return;
    }

    let legacy_so = PathBuf::from(&home).join(".local/lib/libvk_layer_sd_fps.so");
    if legacy_so.exists() {
        let _ = fs::remove_file(legacy_so);
    }

    let Some(target_json) = layer_json_path() else { return };
    if let Some(parent) = target_json.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let env_blocks = if enable_autodetect {
        ",\n    \"disable_environment\": { \"DISABLE_SD_FPS\": \"1\" }"
    } else {
        ",\n    \"enable_environment\": { \"ENABLE_SD_FPS\": \"1\" },\n    \"disable_environment\": { \"DISABLE_SD_FPS_UNUSED\": \"1\" }"
    };

    let json_content = format!(
        "{{\n  \"file_format_version\": \"1.0.0\",\n  \"layer\": {{\n    \"name\": \"VK_LAYER_SD_fps\",\n    \"type\": \"GLOBAL\",\n    \"library_path\": \"{}\",\n    \"api_version\": \"1.3.0\",\n    \"implementation_version\": \"1\",\n    \"description\": \"Lightweight Vulkan FPS counter for OpenDeck\",\n    \"functions\": {{\n      \"vkGetInstanceProcAddr\": \"sd_fps_GetInstanceProcAddr\",\n      \"vkGetDeviceProcAddr\": \"sd_fps_GetDeviceProcAddr\"\n    }}{}\n  }}\n}}",
        bundled_so.display(),
                               env_blocks
    );

    if fs::read_to_string(&target_json)
        .map(|e| e != json_content)
        .unwrap_or(true)
        {
            let _ = fs::write(&target_json, json_content);
        }
}

fn extract_json_str<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let pattern = format!("\"{}\":\"", key);
    let start = json.find(&pattern)? + pattern.len();
    let end = json[start..].find('"')? + start;
    Some(&json[start..end])
}

fn read_current_fps() -> Option<u64> {
    let uid = unsafe { libc::geteuid() };
    let path = format!("/dev/shm/sd_fps_{}", uid);
    let content = fs::read_to_string(&path).ok()?;
    let mut lines = content.lines();
    let fps = lines.next()?.trim().parse::<u64>().ok()?;
    let ts = lines.next()?.trim().parse::<u64>().ok()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    if fps > 0 && now.saturating_sub(ts) <= 2 {
        Some(fps)
    } else {
        None
    }
}
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut s1, mut s2) = (1u32, 0u32);
    for &b in data {
        s1 = (s1 + b as u32) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    (s2 << 16) | s1
}

fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk.first().copied().unwrap_or(0) as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        out.push(TABLE[b0 >> 2] as char);
        out.push(TABLE[((b0 & 3) << 4) | (b1 >> 4)] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((b1 & 15) << 2) | (b2 >> 6)] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[b2 & 63] as char
        } else {
            '='
        });
    }
    out
}

fn render_gauge_data_uri(fps: Option<u64>) -> String {
    const W: usize = 144;
    const H: usize = 144;
    let (arc_r, arc_g, arc_b, ratio) = match fps {
        None => (100u8, 116u8, 139u8, 0.0f32),
        Some(f) if f <= 19 => (239, 68, 68, (f as f32 / 240.0).clamp(0.0, 1.0)),
        Some(f) if f <= 29 => (249, 115, 22, (f as f32 / 240.0).clamp(0.0, 1.0)),
        Some(f) if f <= 39 => (234, 179, 8, (f as f32 / 240.0).clamp(0.0, 1.0)),
        Some(f) if f <= 59 => (132, 204, 22, (f as f32 / 240.0).clamp(0.0, 1.0)),
        Some(f) => (34, 197, 94, (f as f32 / 240.0).clamp(0.0, 1.0)),
    };

    let needle_deg = (145.0 + ratio * 250.0) % 360.0;
    let needle_rad = needle_deg.to_radians();
    let (nx, ny) = (needle_rad.cos(), needle_rad.sin());
    let (cx, cy) = (72.0f32, 78.0f32);

    let mut raw = Vec::with_capacity(H * (1 + W * 4));
    for y in 0..H {
        raw.push(0);
        for x in 0..W {
            let dx_box = ((x as f32 - 72.0).abs() - 54.0).max(0.0);
            let dy_box = ((y as f32 - 72.0).abs() - 54.0).max(0.0);
            let (mut r, mut g, mut b, mut a) = if dx_box.hypot(dy_box) <= 18.0 {
                (20u8, 24u8, 32u8, 255u8)
            } else {
                (0, 0, 0, 0)
            };

            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = dx.hypot(dy);
            let ang = (dy.atan2(dx).to_degrees() + 360.0) % 360.0;
            let in_arc = ang >= 145.0 || ang <= 35.0;

            if (38.0..=49.0).contains(&dist) && in_arc {
                let progress = ((ang - 145.0).rem_euclid(360.0)) / 250.0;
                if fps.is_some() && progress <= ratio {
                    (r, g, b, a) = (arc_r, arc_g, arc_b, 255);
                } else {
                    (r, g, b, a) = (45, 55, 72, 255);
                }
            }

            let proj = dx * nx + dy * ny;
            let perp = (dx * (-ny) + dy * nx).abs();
            if (0.0..=35.0).contains(&proj) && perp <= 2.6 {
                (r, g, b, a) = (248, 250, 252, 255);
            }
            if dist <= 6.5 {
                (r, g, b, a) = (arc_r, arc_g, arc_b, 255);
            }
            raw.extend_from_slice(&[r, g, b, a]);
        }
    }

    let mut zlib = vec![0x78, 0x01];
    for (i, chunk) in raw.chunks(65535).enumerate() {
        let last = if (i + 1) * 65535 >= raw.len() { 1 } else { 0 };
        let len = chunk.len() as u16;
        zlib.push(last);
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(chunk);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let push_chunk = |out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(tag);
        out.extend_from_slice(data);
        let mut crc_buf = Vec::with_capacity(4 + data.len());
        crc_buf.extend_from_slice(tag);
        crc_buf.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_buf).to_be_bytes());
    };

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(W as u32).to_be_bytes());
    ihdr.extend_from_slice(&(H as u32).to_be_bytes());
    ihdr.push(8);
    ihdr.push(6);
    ihdr.push(0);
    ihdr.push(0);
    ihdr.push(0);
    push_chunk(&mut png, b"IHDR", &ihdr);
    push_chunk(&mut png, b"IDAT", &zlib);
    push_chunk(&mut png, b"IEND", &[]);

    format!("data:image/png;base64,{}", base64_encode(&png))
}

fn main() {
    install_signal_handlers();
    let mut enable_autodetect = load_auto_detect_config();
    sync_vulkan_layer(enable_autodetect);

    let args: Vec<String> = env::args().collect();
    let mut port = String::new();
    let mut uuid = String::new();
    let mut reg_event = String::new();

    let mut i = 1;
    while i + 1 < args.len() {
        match args[i].trim_start_matches('-') {
            "port" => port = args[i + 1].clone(),
            "pluginUUID" => uuid = args[i + 1].clone(),
            "registerEvent" => reg_event = args[i + 1].clone(),
            _ => {}
        }
        i += 2;
    }

    let url = format!("ws://127.0.0.1:{}", port);
    let (mut socket, _) = connect(&url).expect("Failed to connect to OpenDeck WebSocket");

    let reg_msg = format!("{{\"event\":\"{}\",\"uuid\":\"{}\"}}", reg_event, uuid);
    let _ = socket.send(Message::Text(reg_msg.into()));

    if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
        let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
    }

    let mut contexts: HashSet<String> = HashSet::new();
    let mut last_sent = SystemTime::now() - Duration::from_secs(2);
    let mut last_rendered_fps: Option<Option<u64>> = None;
    let mut cached_uri = render_gauge_data_uri(None);

    while RUNNING.load(Ordering::SeqCst) {
        if let Ok(Message::Text(txt)) = socket.read() {
            let s = txt.as_str();
            if let Some(ev) = extract_json_str(s, "event") {
                if ev == "willAppear" {
                    if let Some(ctx) = extract_json_str(s, "context") {
                        contexts.insert(ctx.to_string());
                        last_rendered_fps = None;
                        let _ = socket.send(Message::Text(
                            format!("{{\"event\":\"getSettings\",\"context\":\"{}\"}}", ctx).into(),
                        ));
                    }
                } else if ev == "willDisappear" {
                    if let Some(ctx) = extract_json_str(s, "context") {
                        contexts.remove(ctx);
                    }
                } else if (ev == "didReceiveSettings" || ev == "sendToPlugin")
                    && (s.contains("\"ENABLE_AUTODETECT\":false") || s.contains("\"ENABLE_AUTODETECT\":true"))
                    {
                        enable_autodetect = !s.contains("\"ENABLE_AUTODETECT\":false");
                        sync_vulkan_layer(enable_autodetect);
                    }
            }
        }

        if last_sent.elapsed().unwrap_or_default() >= Duration::from_secs(1) {
            last_sent = SystemTime::now();
            let current_fps = read_current_fps();
            let title = match current_fps {
                Some(f) => format!("{}\\nFPS", f),
                None => "--\\nFPS".to_string(),
            };

            let image_changed = last_rendered_fps != Some(current_fps);
            if image_changed {
                cached_uri = render_gauge_data_uri(current_fps);
                last_rendered_fps = Some(current_fps);
            }

            for ctx in &contexts {
                let title_msg = format!(
                    "{{\"event\":\"setTitle\",\"context\":\"{}\",\"payload\":{{\"title\":\"{}\",\"target\":0}}}}",
                    ctx, title
                );
                if socket.send(Message::Text(title_msg.into())).is_err() {
                    cleanup_on_exit();
                    return;
                }
                if image_changed {
                    let img_msg = format!(
                        "{{\"event\":\"setImage\",\"context\":\"{}\",\"payload\":{{\"image\":\"{}\",\"target\":0}}}}",
                        ctx, cached_uri
                    );
                    if socket.send(Message::Text(img_msg.into())).is_err() {
                        cleanup_on_exit();
                        return;
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(50));
    }

    cleanup_on_exit();
}
