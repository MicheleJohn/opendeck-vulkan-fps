# Vulkan FPS for OpenDeck

A lightweight, native Rust plugin for [OpenDeck](https://github.com) that displays real-time game FPS directly on your Stream Deck keys on Linux — with **zero on-screen HUD**, **near-zero CPU/GPU overhead**, and full compatibility across **AMD, NVIDIA, and Intel** GPUs.

## How It Works

The plugin consists of two lightweight native Rust components:
1. **Vulkan Implicit Layer (`libvk_layer_sd_fps.so`):** Registered via `~/.local/share/vulkan/implicit_layer.d/VK_LAYER_SD_fps.json` on first launch and loaded directly from OpenDeck's `.sdPlugin` directory. It hooks `vkQueuePresentKHR` as a transparent pass-through, increments an atomic frame counter (`~1 ns` per frame), and writes the current FPS atomically to `/dev/shm/sd_fps_<UID>` (RAM) once per second.
2. **OpenDeck Daemon (`sd_fps_plugin`):** Reads the FPS value from shared memory once per second and updates both the Stream Deck button title and a dynamic gauge icon (`0–240+ FPS` across 5 color bands) via OpenDeck's local WebSocket API.

Works out-of-the-box with native **Vulkan** games as well as **DirectX 9/10/11/12** Windows games running through **Proton, Wine, Lutris, Heroic, Bottles, and Gamescope** (via DXVK / VKD3D-Proton).

## Usage

1. Install **Vulkan FPS** from the OpenDeck Marketplace (or copy `io.github.michelejohn.fps.sdPlugin` into `~/.config/opendeck/plugins/`).
2. Drag **FPS Counter** onto any key on your Stream Deck.
3. Choose your preferred detection mode in the action settings panel:

### Auto-detect mode (`ENABLE_AUTODETECT` checked — Default)
- Automatically activates on Steam, Proton, Wine, Lutris, Heroic, and Gamescope games while ignoring browsers and desktop compositors.
- To exclude a specific game, set:
  ```bash
  DISABLE_SD_FPS=1
  ```

### Manual mode (`ENABLE_AUTODETECT` unchecked)
- Disables automatic game detection so the layer only loads on titles where you explicitly enable it:
  - **Steam Launch Options:**
    ```bash
    ENABLE_SD_FPS=1 %command%
    ```
  - **Lutris / Heroic Environment Variables:**
    ```text
    ENABLE_SD_FPS=1
    ```

## Uninstalling

Uninstalling **Vulkan FPS** from OpenDeck removes the `.sdPlugin` directory (immediately disabling the Vulkan layer) and automatically cleans up `~/.local/share/vulkan/implicit_layer.d/VK_LAYER_SD_fps.json` and `/dev/shm/sd_fps_<UID>`.

## Building from Source

```bash
./build.sh --install
```
Then restart OpenDeck to load the updated binaries.
