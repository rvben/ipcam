use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Terminal;

use crate::camera::{HealthStatus, StreamQuality};
use crate::config::{CameraConfig, Config};
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

struct App {
    cameras: Vec<CameraStatus>,
    table_state: TableState,
    last_refresh: Instant,
    online_count: usize,
}

impl App {
    fn new() -> Self {
        Self {
            cameras: Vec::new(),
            table_state: TableState::default(),
            last_refresh: Instant::now(),
            online_count: 0,
        }
    }

    fn update_counts(&mut self) {
        self.online_count = self.cameras.iter().filter(|c| c.status.online).count();
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
}

fn build_camera_status(cam_config: &CameraConfig, go2rtc: Option<&crate::config::Go2rtcConfig>) -> CameraStatus {
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

async fn poll_cameras(config: &Config, cameras: &mut [CameraStatus]) {
    let futs: Vec<_> = config
        .cameras
        .iter()
        .map(|cam_config| {
            let cam = vendors::create_camera(cam_config, config.go2rtc.as_ref());
            async move {
                match cam {
                    Ok(c) => c.is_reachable().await,
                    Err(e) => HealthStatus {
                        online: false,
                        detail: e.to_string(),
                        latency: Duration::ZERO,
                    },
                }
            }
        })
        .collect();

    let results = futures::future::join_all(futs).await;
    for (cam, status) in cameras.iter_mut().zip(results) {
        cam.status = status;
        cam.refreshing = false;
    }
}

fn draw(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    terminal.draw(|frame| {
        let has_selection = app.selected_camera().is_some();
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(if has_selection {
                vec![
                    Constraint::Length(1),
                    Constraint::Min(5),
                    Constraint::Length(6),
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
            .split(frame.area());

        // Header (compact, no border)
        let elapsed = app.last_refresh.elapsed().as_secs();
        let total = app.cameras.len();
        let online = app.online_count;
        let header = Line::from(vec![
            Span::styled(" ipcam ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{}/{} online", online, total),
                Style::default().fg(if online == total { Color::Green } else { Color::Yellow }),
            ),
            Span::styled(
                format!("  {}s ago", elapsed),
                Style::default().fg(Color::DarkGray),
            ),
        ]);
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
            let (status_text, status_color) = if cam.refreshing {
                ("...", Color::DarkGray)
            } else if cam.status.online {
                ("●", Color::Green)
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

        // Detail panel (only if a camera is selected)
        if let Some(cam) = app.selected_camera() {
            let status_label = if cam.status.online { "online" } else { "offline" };
            let status_color = if cam.status.online { Color::Green } else { Color::Red };
            let details = vec![
                Line::from(vec![
                    Span::styled("  Host: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&cam.host),
                    Span::raw("    "),
                    Span::styled("Type: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&cam.camera_type),
                    Span::raw("    "),
                    Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(status_label, Style::default().fg(status_color)),
                ]),
                Line::from(vec![
                    Span::styled("  RTSP: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&cam.rtsp_url),
                ]),
                Line::from(vec![
                    Span::styled("  ONVIF: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("{}:{}", cam.host, cam.onvif_port)),
                    Span::raw("    "),
                    Span::styled("Latency: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(format!("{}ms", cam.status.latency.as_millis())),
                ]),
            ];
            let detail_block = Paragraph::new(details)
                .block(Block::default().borders(Borders::ALL).title(format!(" {} ", cam.name)));
            frame.render_widget(detail_block, main_chunks[2]);
        }

        // Footer (compact, no border)
        let footer = Line::from(vec![
            Span::styled(" q", Style::default().fg(Color::Yellow)),
            Span::raw(" quit  "),
            Span::styled("r", Style::default().fg(Color::Yellow)),
            Span::raw(" refresh  "),
            Span::styled("j/k", Style::default().fg(Color::Yellow)),
            Span::raw(" navigate"),
        ]);
        frame.render_widget(Paragraph::new(footer), main_chunks[3]);
    })?;
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

    // Setup terminal
    terminal::enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;

    // Ensure terminal is restored on panic
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_panic(info);
    }));

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let refresh_interval = Duration::from_secs(interval);

    // Build initial camera list with "checking..." status
    app.cameras = config
        .cameras
        .iter()
        .map(|c| build_camera_status(c, config.go2rtc.as_ref()))
        .collect();
    if !app.cameras.is_empty() {
        app.table_state.select(Some(0));
    }

    // Draw immediately so user sees something while first poll runs
    draw(&mut terminal, &mut app)?;

    // Initial poll
    poll_cameras(config, &mut app.cameras).await;
    app.last_refresh = Instant::now();
    app.update_counts();

    loop {
        draw(&mut terminal, &mut app)?;

        // Use a short poll timeout so the elapsed counter updates frequently
        let poll_timeout = Duration::from_secs(1)
            .min(refresh_interval.saturating_sub(app.last_refresh.elapsed()));

        if event::poll(poll_timeout)? {
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('r') => {
                        for cam in &mut app.cameras {
                            cam.refreshing = true;
                        }
                        draw(&mut terminal, &mut app)?;
                        poll_cameras(config, &mut app.cameras).await;
                        app.last_refresh = Instant::now();
                        app.update_counts();
                    }
                    KeyCode::Down | KeyCode::Char('j') => app.next(),
                    KeyCode::Up | KeyCode::Char('k') => app.previous(),
                    _ => {}
                }
            }
        } else if app.last_refresh.elapsed() >= refresh_interval {
            poll_cameras(config, &mut app.cameras).await;
            app.last_refresh = Instant::now();
            app.update_counts();
        }
    }

    restore_terminal();
    Ok(())
}
