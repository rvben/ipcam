# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [0.0.7](https://github.com/rvben/ipcam/compare/v0.0.6...v0.0.7) - 2026-06-11

### Added

- **cli**: rename format flag to --output/-o with --format alias ([a603de8](https://github.com/rvben/ipcam/commit/a603de8f5dde28b05f284e029d4839dcd74ee128))
- clispec v0.2 compliance ([9038dac](https://github.com/rvben/ipcam/commit/9038dac1a6408cd7188ed3c54d487af5250ac8fc))
- native RTSP frame grabbing with retina + openh264, drop ffmpeg dependency ([9b7f787](https://github.com/rvben/ipcam/commit/9b7f787c0c42c111798281abfcfe2e650eea40ea))
- **tui**: add snapshot, open stream, and status feedback ([9b8d6cd](https://github.com/rvben/ipcam/commit/9b8d6cd8dad2a0c83d5944f750d4b4fb27c42564))
- **config**: separate ONVIF credentials from RTSP credentials ([92139fc](https://github.com/rvben/ipcam/commit/92139fc9bfc316d9f0e7317c8c628f995970a123))
- **snapshot**: ONVIF GetSnapshotUri with ffmpeg fallback, config check, model scopes ([cbac068](https://github.com/rvben/ipcam/commit/cbac0683edc73843df8854c2238893c8220d952c))
- **cli**: add colored table output for list, status, discover, and watch ([a46aa96](https://github.com/rvben/ipcam/commit/a46aa9680eba75e5ff638cf3d9048f8cd66e1e48))

### Fixed

- emit structured error envelope on clap parse failures ([a957e3c](https://github.com/rvben/ipcam/commit/a957e3cbcbf5b559befbd6ccbfa24aea15380aad))
- **tui**: replace '...' refreshing status with yellow indicator ([504b4d9](https://github.com/rvben/ipcam/commit/504b4d90a41a77e7007e6ade619976610cce3608))
- **tui**: RTSP auth, connection timeouts, preview UX improvements ([7b6364f](https://github.com/rvben/ipcam/commit/7b6364f99412d6a629819e4ff399d0831de9493c))
- **tui**: show meaningful errors in live preview instead of hanging ([07e8222](https://github.com/rvben/ipcam/commit/07e82221f475a252c56ab9930936453dfd645ad3))
- **status**: show camera type when ONVIF model query fails ([5ace38b](https://github.com/rvben/ipcam/commit/5ace38bde2a6b87a307438c1eab65a0b36482c32))
