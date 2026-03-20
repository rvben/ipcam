# camera-cli

A command-line tool for managing IP cameras (Tapo, Reolink) via RTSP and vendor APIs.

## Features

- Capture snapshots from individual cameras or all cameras in parallel
- Assemble multi-camera snapshots into a single tiled grid image
- Record video clips and build timelapses using ffmpeg
- Print or pipe RTSP stream URLs (main and sub streams)
- Pan/tilt/zoom control via ONVIF (Tapo) or Reolink API
- Poll motion detection events and watch for status changes
- Health-check all cameras and run a hook command on status change
- End-to-end camera test (network reachability, RTSP URL, snapshot)
- Discover cameras on the local network via ONVIF WS-Discovery
- Interactive `init` wizard that auto-discovers and configures cameras
- Frigate NVR integration: list events and fetch latest snapshots
- go2rtc restream proxy support for cameras behind a proxy
- JSON output for every command (`--json`)
- Shell completions for bash, zsh, and fish

## Installation

### Prerequisites

- Rust toolchain (install via [rustup](https://rustup.rs))
- [ffmpeg](https://ffmpeg.org/download.html) — required for snapshots, recording, and timelapse

### Build and install

```bash
cargo install --path .
```

## Quick Start

Run the interactive setup wizard to discover cameras on your network and generate a config file:

```bash
camera-cli init
```

Or use `--auto` to skip prompts and generate a config from detected cameras:

```bash
camera-cli init --auto
```

Once configured, a few common commands:

```bash
# List configured cameras
camera-cli list

# Capture a snapshot
camera-cli snapshot front-door

# Check all cameras are reachable
camera-cli status

# Run an end-to-end test on all cameras
camera-cli test
```

## Configuration

The config file is TOML and lives at:

- **macOS:** `~/Library/Application Support/camera-cli/config.toml`
- **Linux:** `~/.config/camera-cli/config.toml`

Run `camera-cli config` to print the exact path on your system.

### Example config

```toml
[go2rtc]
host = "192.168.1.10"
port = 8554  # default

[frigate]
host = "192.168.1.11"
port = 5001  # default

[[cameras]]
name = "front-door"
type = "reolink"
host = "192.168.1.101"
username = "admin"
password = "your-password"

[[cameras]]
name = "backyard"
type = "tapo"
host = "192.168.1.102"
username = "admin"
password = "your-password"
# Optional: override RTSP port (default: 554)
rtsp_port = 554
# Optional: override ONVIF port (default: 2020 for Tapo, 8000 for Reolink)
onvif_port = 2020
# Optional: use a go2rtc restream instead of direct RTSP
go2rtc_stream = "backyard"
# Optional: Frigate camera name if it differs from the config name
frigate_name = "backyard_cam"
```

### Config fields

| Field | Required | Description |
|---|---|---|
| `name` | yes | Unique identifier used in all commands |
| `type` | yes | `tapo` or `reolink` |
| `host` | yes | IP address of the camera |
| `username` | no | Camera username (default: `admin`) |
| `password` | no | Camera password |
| `rtsp_port` | no | RTSP port (default: 554) |
| `onvif_port` | no | ONVIF port (default: 2020 for Tapo, 8000 for Reolink) |
| `go2rtc_stream` | no | go2rtc stream name when using a restream proxy |
| `frigate_name` | no | Frigate camera name (default: config `name` with `-` replaced by `_`) |

## Commands

| Command | Description |
|---|---|
| `init` | Interactive setup wizard; discovers cameras via ONVIF and writes config |
| `list` | List all configured cameras |
| `info <camera>` | Show camera model and firmware (Reolink) or basic info (Tapo) |
| `snapshot <camera>` | Capture a JPEG snapshot |
| `snapshot --all` | Capture snapshots from all cameras in parallel |
| `snapshot --grid` | Capture from all cameras and tile into a single image |
| `snapshot --every <interval>` | Capture snapshots on a repeating interval |
| `snapshot-all` | Alias for `snapshot --all` |
| `stream <camera>` | Print the RTSP URL, or pipe to a file with `--output` |
| `record <camera>` | Record a video clip (default 30 s) |
| `timelapse <camera>` | Capture frames at an interval and encode to MP4 |
| `status [camera]` | Check which cameras are online and report latency |
| `test [camera]` | End-to-end test: network, RTSP URL, snapshot |
| `watch` | Poll camera health continuously; run a hook on status changes |
| `events <camera>` | Show motion detection status; use `--watch` to poll continuously |
| `ptz <camera> <action>` | Pan/tilt/zoom control: `left`, `right`, `up`, `down`, `stop`, `preset` |
| `discover` | Scan the network for ONVIF cameras |
| `frigate events` | List recent Frigate NVR events |
| `frigate snapshot <camera>` | Fetch the latest snapshot from Frigate |
| `config` | Show the config file path and whether it exists |
| `completions <shell>` | Print a shell completion script |

All commands accept `--json` for machine-readable output and `--config <path>` to override the config file location.

## Examples

### Snapshot

```bash
# Single camera, default filename (<camera>_<timestamp>.jpg)
camera-cli snapshot front-door

# Single camera, custom output path
camera-cli snapshot front-door --output /tmp/front.jpg

# All cameras saved to a directory
camera-cli snapshot --all --output-dir /tmp/cams/

# Tiled grid of all cameras
camera-cli snapshot --grid --output-dir /tmp/

# Capture every 5 minutes, save timestamped files
camera-cli snapshot front-door --every 5m --output-dir /tmp/front-door/
```

### Status and test

```bash
# Check all cameras
camera-cli status

# Check a single camera
camera-cli status front-door

# Full end-to-end test (network + RTSP + snapshot)
camera-cli test

# Test a single camera, JSON output
camera-cli test front-door --json
```

### Watch

```bash
# Poll every 30 s, print status changes to stdout
camera-cli watch

# Poll every minute, run a script on any status change
camera-cli watch --interval 1m --exec 'notify-send "$CAMERA_NAME is $CAMERA_STATUS"'
```

The `--exec` command receives these environment variables:

| Variable | Value |
|---|---|
| `CAMERA_NAME` | Camera name from config |
| `CAMERA_HOST` | Camera IP address |
| `CAMERA_STATUS` | `online` or `offline` |
| `CAMERA_DETAIL` | Human-readable detail (model or error message) |

### Timelapse

```bash
# 1-hour capture with a snapshot every 30 s, encoded to timelapse.mp4
camera-cli timelapse front-door --interval 30s --duration 1h --output front-door.mp4

# Keep individual frames as well
camera-cli timelapse front-door --interval 1m --duration 4h \
  --output timelapse.mp4 --output-dir ./frames/
```

### Shell completions

```bash
# zsh
camera-cli completions zsh > ~/.zfunc/_camera-cli

# bash
camera-cli completions bash > /etc/bash_completion.d/camera-cli

# fish
camera-cli completions fish > ~/.config/fish/completions/camera-cli.fish
```

### PTZ control

```bash
# Pan left at speed 5 (default)
camera-cli ptz backyard left

# Tilt up at speed 8
camera-cli ptz backyard up --speed 8

# Go to preset position 1
camera-cli ptz backyard preset 1

# Stop movement
camera-cli ptz backyard stop
```

## Supported Cameras

### Tapo (TP-Link)

- Snapshots captured via ffmpeg over RTSP (sub-stream by default)
- PTZ control via ONVIF SOAP over HTTP (port 2020 by default)
- RTSP streams: `stream1` (main), `stream2` (sub)
- Motion detection: not supported (Tapo does not expose an API for this)
- Health check: TCP connect to the RTSP port

### Reolink

- Snapshots fetched via the Reolink HTTP/JSON API (`/cgi-bin/api.cgi?cmd=Snap`)
- PTZ control via the Reolink `PtzCtrl` API
- Motion detection via `GetMdState` API
- RTSP streams: `h264Preview_01_main` (main), `h264Preview_01_sub` (sub)
- Health check: `GetDevInfo` API call (also returns model name)
- Device info (model, firmware) available via `GetDevInfo`

## Integration

### go2rtc

If your cameras are behind a [go2rtc](https://github.com/AlexxIT/go2rtc) restream proxy, add a `[go2rtc]` section to your config and set `go2rtc_stream` on each camera. camera-cli will use the proxy RTSP URL (`rtsp://<host>:<port>/<stream>`) instead of connecting to the camera directly.

Sub-stream URLs are constructed by appending `_sub` to the stream name (e.g. `backyard_sub`).

### Frigate NVR

Add a `[frigate]` section to your config pointing at your [Frigate](https://frigate.video) instance. Then use the `frigate` subcommand:

```bash
# List the 20 most recent events across all cameras
camera-cli frigate events --limit 20

# Filter by camera
camera-cli frigate events --camera front_door

# Save the latest Frigate snapshot for a camera
camera-cli frigate snapshot front_door --output /tmp/latest.jpg
```

Frigate camera names use underscores (e.g. `front_door`). Set `frigate_name` in the camera config if the name differs from your camera-cli name.

## License

MIT
