use std::io::{self, Write as _};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::ExecutableCommand;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

use crate::camera::{HealthStatus, StreamQuality};
use crate::config::{CameraConfig, Config, Go2rtcConfig};
use crate::vendors;

struct CameraStatus {
    name: String,
    host: String,
    camera_type: String,
    rtsp_url: String,
    onvif_port: u16,
    status: HealthStatus,
    refreshing: bool,
}

struct FrameGrabber {
    handle: tokio::task::JoinHandle<()>,
    frame_path: PathBuf,
    frame_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    error_rx: tokio::sync::mpsc::Receiver<String>,
    last_error: Option<String>,
    has_frame: bool,
}

impl FrameGrabber {
    fn start(camera_idx: usize, rtsp_url: &str) -> Self {
        let frame_path = std::env::temp_dir().join(format!("ipcam_tui_{}.jpg", camera_idx));
        let url = rtsp_url.to_string();
        let (frame_tx, frame_rx) = tokio::sync::mpsc::channel(2);
        let (error_tx, error_rx) = tokio::sync::mpsc::channel(4);
        let handle = tokio::spawn(async move {
            match crate::rtsp_grab::grab_frames_continuous(&url, frame_tx).await {
                Ok(()) => {}
                Err(e) => {
                    let msg = format!("{}", e);
                    // Simplify common error messages
                    let msg = if msg.contains("Connection refused") {
                        "Connection refused".to_string()
                    } else if msg.contains("401") || msg.contains("Unauthorized") {
                        "Authentication failed".to_string()
                    } else if msg.contains("timed out") {
                        "Connection timed out".to_string()
                    } else {
                        msg
                    };
                    let _ = error_tx.send(msg).await;
                }
            }
        });
        Self {
            handle,
            frame_path,
            frame_rx,
            error_rx,
            last_error: None,
            has_frame: false,
        }
    }

    fn shutdown(self) {
        self.handle.abort();
        let _ = std::fs::remove_file(&self.frame_path);
    }

    /// Drain channels. Returns the latest JPEG frame if any new frames arrived.
    fn poll(&mut self) -> Option<Vec<u8>> {
        let mut latest = None;
        while let Ok(jpeg) = self.frame_rx.try_recv() {
            if std::fs::write(&self.frame_path, &jpeg).is_ok() {
                self.has_frame = true;
            }
            latest = Some(jpeg);
        }
        while let Ok(err) = self.error_rx.try_recv() {
            self.last_error = Some(err);
        }
        latest
    }

    /// Seed the preview with a cached frame so it shows immediately.
    fn seed_frame(&mut self, jpeg: &[u8]) {
        if std::fs::write(&self.frame_path, jpeg).is_ok() {
            self.has_frame = true;
        }
    }

    fn status_message(&self) -> &str {
        if self.has_frame {
            return "";
        }
        if let Some(ref err) = self.last_error {
            return err;
        }
        "Connecting..."
    }
}

struct App {
    cameras: Vec<CameraStatus>,
    table_state: TableState,
    last_refresh: Instant,
    online_count: usize,
    preview_camera: Option<usize>,
    preview_area: Option<Rect>,
    grabber: Option<FrameGrabber>,
    frame_cache: std::collections::HashMap<usize, Vec<u8>>,
    health_rx: tokio::sync::mpsc::Receiver<Vec<HealthStatus>>,
    health_pending: bool,
    status_message: Option<(String, Instant)>,
    status_tx: tokio::sync::mpsc::Sender<String>,
    status_rx: tokio::sync::mpsc::Receiver<String>,
}

impl App {
    fn new(health_rx: tokio::sync::mpsc::Receiver<Vec<HealthStatus>>) -> Self {
        let (status_tx, status_rx) = tokio::sync::mpsc::channel(4);
        Self {
            cameras: Vec::new(),
            table_state: TableState::default(),
            last_refresh: Instant::now(),
            online_count: 0,
            preview_camera: None,
            preview_area: None,
            grabber: None,
            frame_cache: std::collections::HashMap::new(),
            health_rx,
            health_pending: false,
            status_message: None,
            status_tx,
            status_rx,
        }
    }

    fn set_status(&mut self, msg: String) {
        self.status_message = Some((msg, Instant::now()));
    }

    fn active_status(&self) -> Option<&str> {
        self.status_message.as_ref().and_then(|(msg, when)| {
            if when.elapsed() < Duration::from_secs(5) {
                Some(msg.as_str())
            } else {
                None
            }
        })
    }

    fn update_counts(&mut self) {
        self.online_count = self.cameras.iter().filter(|c| c.status.online).count();
    }

    fn try_recv_health(&mut self) {
        if let Ok(results) = self.health_rx.try_recv() {
            for (cam, status) in self.cameras.iter_mut().zip(results) {
                cam.status = status;
                cam.refreshing = false;
            }
            self.last_refresh = Instant::now();
            self.update_counts();
            self.health_pending = false;
        }
        // Check for status messages from background tasks (snapshot, etc.)
        while let Ok(msg) = self.status_rx.try_recv() {
            self.set_status(msg);
        }
    }

    fn selected_camera(&self) -> Option<&CameraStatus> {
        self.table_state
            .selected()
            .and_then(|i| self.cameras.get(i))
    }

    fn next(&mut self) {
        if self.cameras.is_empty() {
            return;
        }
        let i = match self.table_state.selected() {
            Some(i) => (i + 1) % self.cameras.len(),
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    fn previous(&mut self) {
        if self.cameras.is_empty() {
            return;
        }
        let i = match self.table_state.selected() {
            Some(0) | None => self.cameras.len() - 1,
            Some(i) => i - 1,
        };
        self.table_state.select(Some(i));
    }

    fn toggle_preview(&mut self) {
        let selected = self.table_state.selected();
        if self.preview_camera == selected {
            self.close_preview();
        } else if let Some(idx) = selected {
            self.close_preview();
            let rtsp_url = self.cameras[idx].rtsp_url.clone();
            self.preview_camera = Some(idx);
            self.grabber = Some(FrameGrabber::start(idx, &rtsp_url));
        }
    }

    fn switch_preview_to_selected(&mut self) {
        let selected = self.table_state.selected();
        if selected == self.preview_camera {
            return;
        }
        if let Some(idx) = selected {
            self.close_preview();
            let rtsp_url = self.cameras[idx].rtsp_url.clone();
            self.preview_camera = Some(idx);
            let mut grabber = FrameGrabber::start(idx, &rtsp_url);
            if let Some(cached) = self.frame_cache.get(&idx) {
                grabber.seed_frame(cached);
            }
            self.grabber = Some(grabber);
        }
    }

    fn close_preview(&mut self) {
        self.preview_camera = None;
        self.preview_area = None;
        if let Some(g) = self.grabber.take() {
            g.shutdown();
        }
    }
}

fn build_camera_status(cam_config: &CameraConfig, go2rtc: Option<&Go2rtcConfig>) -> CameraStatus {
    let rtsp_url = vendors::create_camera(cam_config, go2rtc)
        .map(|c| c.rtsp_url(StreamQuality::Main))
        .unwrap_or_default();
    CameraStatus {
        name: cam_config.name.clone(),
        host: cam_config.host.clone(),
        camera_type: cam_config.camera_type.to_string(),
        rtsp_url,
        onvif_port: cam_config.onvif_port(),
        status: HealthStatus {
            online: false,
            detail: "checking...".to_string(),
            latency: Duration::ZERO,
        },
        refreshing: true,
    }
}

/// Spawn a non-blocking health check for all cameras.
fn spawn_health_check(config: &Config, tx: &tokio::sync::mpsc::Sender<Vec<HealthStatus>>) {
    let cameras: Vec<_> = config
        .cameras
        .iter()
        .map(|c| vendors::create_camera(c, config.go2rtc.as_ref()))
        .collect();
    let tx = tx.clone();
    tokio::spawn(async move {
        let futs: Vec<_> = cameras
            .into_iter()
            .map(|cam| async move {
                match cam {
                    Ok(c) => c.is_reachable().await,
                    Err(e) => HealthStatus {
                        online: false,
                        detail: e.to_string(),
                        latency: Duration::ZERO,
                    },
                }
            })
            .collect();
        let results = futures::future::join_all(futs).await;
        let _ = tx.send(results).await;
    });
}

fn draw(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    terminal.draw(|frame| {
        let area = frame.area();

        // Horizontal split when preview is active
        let (left_area, preview_rect) = if app.preview_camera.is_some() {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);
            (chunks[0], Some(chunks[1]))
        } else {
            (area, None)
        };

        // Left side: header, table, detail, footer
        let has_selection = app.selected_camera().is_some();
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(if has_selection {
                vec![
                    Constraint::Length(1),
                    Constraint::Min(5),
                    Constraint::Length(7),
                    Constraint::Length(1),
                ]
            } else {
                vec![
                    Constraint::Length(1),
                    Constraint::Min(5),
                    Constraint::Length(0),
                    Constraint::Length(1),
                ]
            })
            .split(left_area);

        // Header
        let elapsed = app.last_refresh.elapsed().as_secs();
        let total = app.cameras.len();
        let online = app.online_count;
        let mut header_spans = vec![
            Span::styled(
                " ipcam ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{}/{} online", online, total),
                Style::default().fg(if online == total {
                    Color::Green
                } else {
                    Color::Yellow
                }),
            ),
            Span::styled(
                format!("  {}s ago", elapsed),
                Style::default().fg(Color::DarkGray),
            ),
        ];
        if let Some(msg) = app.active_status() {
            header_spans.push(Span::raw("  "));
            header_spans.push(Span::styled(msg, Style::default().fg(Color::Cyan)));
        }
        let header = Line::from(header_spans);
        frame.render_widget(Paragraph::new(header), main_chunks[0]);

        // Camera table
        let header_cells = ["Name", "Host", "Type", "Status", "Latency", "Detail"]
            .iter()
            .map(|h| {
                Cell::from(*h).style(
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                )
            });
        let header_row = Row::new(header_cells).height(1);

        let rows = app.cameras.iter().map(|cam| {
            let (status_text, status_color) = if cam.status.online {
                ("●", Color::Green)
            } else if cam.refreshing {
                ("●", Color::Yellow)
            } else {
                ("●", Color::Red)
            };
            let latency = if cam.status.latency.as_millis() > 0 {
                format!("{}ms", cam.status.latency.as_millis())
            } else {
                "-".to_string()
            };
            Row::new(vec![
                Cell::from(cam.name.as_str()),
                Cell::from(cam.host.as_str()),
                Cell::from(cam.camera_type.as_str()),
                Cell::from(status_text).style(Style::default().fg(status_color)),
                Cell::from(latency),
                Cell::from(cam.status.detail.as_str()),
            ])
        });

        let table = Table::new(
            rows,
            [
                Constraint::Length(20),
                Constraint::Length(18),
                Constraint::Length(10),
                Constraint::Length(8),
                Constraint::Length(8),
                Constraint::Min(20),
            ],
        )
        .header(header_row)
        .block(Block::default().borders(Borders::ALL).title(" Cameras "))
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

        frame.render_stateful_widget(table, main_chunks[1], &mut app.table_state);

        // Detail panel
        if let Some(cam) = app.selected_camera() {
            let status_label = if cam.status.online {
                "online"
            } else {
                "offline"
            };
            let status_color = if cam.status.online {
                Color::Green
            } else {
                Color::Red
            };
            let mut details = vec![Line::from(vec![
                Span::styled("  Host: ", Style::default().fg(Color::DarkGray)),
                Span::raw(&cam.host),
                Span::raw("    "),
                Span::styled("Type: ", Style::default().fg(Color::DarkGray)),
                Span::raw(&cam.camera_type),
                Span::raw("    "),
                Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
                Span::styled(status_label, Style::default().fg(status_color)),
            ])];
            if cam.status.online {
                details.push(Line::from(vec![
                    Span::styled("  Detail: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&cam.status.detail),
                    Span::raw("    "),
                    Span::styled("Latency: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("{}ms", cam.status.latency.as_millis())),
                ]));
            } else {
                details.push(Line::from(vec![
                    Span::styled("  Detail: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(&cam.status.detail, Style::default().fg(Color::Red)),
                ]));
            }
            details.push(Line::from(vec![
                Span::styled("  RTSP: ", Style::default().fg(Color::DarkGray)),
                Span::raw(crate::redact_url(&cam.rtsp_url)),
            ]));
            details.push(Line::from(vec![
                Span::styled("  ONVIF: ", Style::default().fg(Color::DarkGray)),
                Span::raw(format!("{}:{}", cam.host, cam.onvif_port)),
            ]));
            let detail_block = Paragraph::new(details).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} ", cam.name)),
            );
            frame.render_widget(detail_block, main_chunks[2]);
        }

        // Footer
        let mut footer_spans = vec![
            Span::styled(" q", Style::default().fg(Color::Yellow)),
            Span::raw(" quit  "),
            Span::styled("r", Style::default().fg(Color::Yellow)),
            Span::raw(" refresh  "),
            Span::styled("j/k", Style::default().fg(Color::Yellow)),
            Span::raw(" navigate  "),
            Span::styled("s", Style::default().fg(Color::Yellow)),
            Span::raw(" snapshot  "),
            Span::styled("Enter", Style::default().fg(Color::Yellow)),
        ];
        if app.preview_camera.is_some() {
            footer_spans.push(Span::raw(" stop  "));
            footer_spans.push(Span::styled("Esc", Style::default().fg(Color::Yellow)));
            footer_spans.push(Span::raw(" close preview"));
        } else {
            footer_spans.push(Span::raw(" preview"));
        }
        frame.render_widget(Paragraph::new(Line::from(footer_spans)), main_chunks[3]);

        // Preview pane (border + placeholder text; actual image rendered after draw)
        if let Some(rect) = preview_rect {
            let cam_name = app
                .preview_camera
                .and_then(|i| app.cameras.get(i))
                .map(|c| c.name.as_str())
                .unwrap_or("Preview");

            let has_frame = app.grabber.as_ref().is_some_and(|g| g.has_frame);

            let status_msg = app
                .grabber
                .as_ref()
                .map(|g| g.status_message().to_string())
                .unwrap_or_default();

            let block = Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} - Live ", cam_name));
            let inner = block.inner(rect);
            frame.render_widget(block, rect);

            if !has_frame && !status_msg.is_empty() {
                let is_error = status_msg != "Connecting...";
                let color = if is_error {
                    Color::Red
                } else {
                    Color::DarkGray
                };
                let msg = Paragraph::new(status_msg)
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(color));
                frame.render_widget(msg, inner);
            }

            app.preview_area = Some(inner);
        } else {
            app.preview_area = None;
        }
    })?;

    // Render preview image after ratatui flush (viuer writes directly to stdout)
    if let Some(ref grabber) = app.grabber
        && let Some(area) = app.preview_area
        && area.width > 0
        && area.height > 0
        && grabber.has_frame
    {
        let conf = viuer::Config {
            absolute_offset: true,
            x: area.x,
            y: area.y as i16,
            width: Some(u32::from(area.width)),
            height: Some(u32::from(area.height)),
            restore_cursor: true,
            ..Default::default()
        };
        let _ = viuer::print_from_file(&grabber.frame_path, &conf);
        let _ = io::stdout().flush();
    }

    Ok(())
}

fn restore_terminal() {
    let _ = terminal::disable_raw_mode();
    let _ = io::stdout().execute(LeaveAlternateScreen);
}

pub async fn run_tui(config: &Config, interval: u64) -> Result<()> {
    if config.cameras.is_empty() {
        anyhow::bail!("no cameras configured");
    }

    terminal::enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;

    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_panic(info);
    }));

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let (health_tx, health_rx) = tokio::sync::mpsc::channel(1);
    let mut app = App::new(health_rx);
    let refresh_interval = Duration::from_secs(interval);

    app.cameras = config
        .cameras
        .iter()
        .map(|c| build_camera_status(c, config.go2rtc.as_ref()))
        .collect();
    if !app.cameras.is_empty() {
        app.table_state.select(Some(0));
    }

    draw(&mut terminal, &mut app)?;

    // Initial health check (non-blocking)
    spawn_health_check(config, &health_tx);
    app.health_pending = true;

    loop {
        // Check for background health check results
        app.try_recv_health();
        if let Some(ref mut g) = app.grabber
            && let Some(jpeg) = g.poll()
            && let Some(idx) = app.preview_camera
        {
            app.frame_cache.insert(idx, jpeg);
        }

        draw(&mut terminal, &mut app)?;

        // Shorter poll when preview is active for smoother image updates
        let base_timeout = if app.preview_camera.is_some() {
            Duration::from_millis(200)
        } else {
            Duration::from_secs(1)
        };
        let poll_timeout =
            base_timeout.min(refresh_interval.saturating_sub(app.last_refresh.elapsed()));

        if event::poll(poll_timeout)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            let prev_preview = app.preview_camera;
            match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Esc => {
                    if app.preview_camera.is_some() {
                        app.close_preview();
                    } else {
                        break;
                    }
                }
                KeyCode::Enter => app.toggle_preview(),
                KeyCode::Char('r') => {
                    for cam in &mut app.cameras {
                        cam.refreshing = true;
                    }
                    spawn_health_check(config, &health_tx);
                    app.health_pending = true;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    app.next();
                    if app.preview_camera.is_some() {
                        app.switch_preview_to_selected();
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    app.previous();
                    if app.preview_camera.is_some() {
                        app.switch_preview_to_selected();
                    }
                }
                KeyCode::Char('s') => {
                    if let Some(idx) = app.table_state.selected() {
                        let cam = &app.cameras[idx];
                        if !cam.status.online {
                            app.set_status(format!("'{}' is offline", cam.name));
                        } else {
                            let name = cam.name.clone();
                            let cam_config = config.cameras.get(idx).cloned();
                            let go2rtc = config.go2rtc.clone();
                            let tx = app.status_tx.clone();
                            tokio::spawn(async move {
                                if let Some(cfg) = cam_config {
                                    match vendors::create_camera(&cfg, go2rtc.as_ref()) {
                                        Ok(c) => match c.snapshot().await {
                                            Ok(snap) => {
                                                let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
                                                let filename = format!(
                                                    "{}_{}.{}",
                                                    name,
                                                    ts,
                                                    snap.format.extension()
                                                );
                                                match std::fs::write(&filename, &snap.data) {
                                                    Ok(()) => {
                                                        let _ = tx
                                                            .send(format!("Saved {}", filename))
                                                            .await;
                                                    }
                                                    Err(e) => {
                                                        let _ = tx
                                                            .send(format!("Write failed: {}", e))
                                                            .await;
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                let _ = tx
                                                    .send(format!("Snapshot failed: {}", e))
                                                    .await;
                                            }
                                        },
                                        Err(e) => {
                                            let _ = tx.send(format!("Camera error: {}", e)).await;
                                        }
                                    }
                                }
                            });
                            app.set_status(format!("Saving snapshot from '{}'...", cam.name));
                        }
                    }
                }
                _ => {}
            }
            // Force full redraw when preview changed (closed or switched camera)
            if prev_preview != app.preview_camera {
                terminal.clear()?;
            }
        }

        // Auto-refresh health checks (non-blocking)
        if !app.health_pending && app.last_refresh.elapsed() >= refresh_interval {
            for cam in &mut app.cameras {
                cam.refreshing = true;
            }
            spawn_health_check(config, &health_tx);
            app.health_pending = true;
        }
    }

    app.close_preview();
    restore_terminal();
    Ok(())
}
