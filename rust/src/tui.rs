use std::io::{self, Stdout};

use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame, Terminal,
};

use crate::{
    app::{App, SETTING_COUNT},
    config::InstallConfig,
    discovery::{format_bytes, HardwareSnapshot},
    plan,
};

type AppTerminal = Terminal<CrosstermBackend<Stdout>>;

pub fn run(config: InstallConfig, hardware: HardwareSnapshot) -> Result<()> {
    enable_raw_mode().context("enable raw terminal mode")?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(error).context("enter alternate screen");
    }
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            return Err(error).context("initialize terminal renderer");
        }
    };
    let mut app = App::new(config, hardware);
    let result = run_loop(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;
    result
}

fn restore_terminal(terminal: &mut AppTerminal) -> Result<()> {
    disable_raw_mode().context("disable raw terminal mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen).context("leave alternate screen")?;
    terminal.show_cursor().context("show terminal cursor")?;
    Ok(())
}

fn run_loop(terminal: &mut AppTerminal, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|frame| render(frame, app))?;
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press { continue; }
            if app.show_disks {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('d') => app.toggle_disks(),
                    KeyCode::Up | KeyCode::Char('k') => app.move_disk_selection(-1),
                    KeyCode::Down | KeyCode::Char('j') => app.move_disk_selection(1),
                    KeyCode::Enter => app.select_disk(),
                    _ => {}
                }
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('d') => app.toggle_disks(),
                KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
                KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
                KeyCode::Left | KeyCode::Char('h') => app.adjust(-1),
                KeyCode::Right | KeyCode::Char('l') => app.adjust(1),
                KeyCode::Enter | KeyCode::Char(' ') => {
                    if matches!(app.selected, 6 | 7) { app.toggle_multi_selection(); }
                    else if key.code == KeyCode::Enter { app.adjust(1); }
                }
                KeyCode::Char('p') => app.show_plan = !app.show_plan,
                KeyCode::Char('?') => app.show_help = !app.show_help,
                KeyCode::Char('s') => {
                    if let Err(error) = app.save_config() {
                        app.status = format!("Could not save profile: {error:#}");
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn render(frame: &mut Frame<'_>, app: &App) {
    if app.show_disks {
        render_disks(frame, app);
        return;
    }
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(12), Constraint::Length(4)])
        .split(frame.area());
    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(" ARTIX ", Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled(" SETUP", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("   /   Rust prototype 0.1.0", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(Span::styled("A modular, configuration-driven Artix installer", Style::default().fg(Color::Gray))),
    ]).block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Cyan)));
    frame.render_widget(header, outer[0]);

    let body = Layout::default().direction(Direction::Horizontal)
        .constraints([Constraint::Length(27), Constraint::Min(30)]).split(outer[1]);
    let items: Vec<ListItem<'_>> = (0..SETTING_COUNT).map(|index| {
        let marker = if index == 1 && app.config.disk.target.is_none() { "○" } else { "●" };
        ListItem::new(format!(" {marker} {:<19}", app.setting_label(index)))
    }).collect();
    let list = List::new(items)
        .block(Block::default().title(" Configuration ").borders(Borders::ALL))
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD))
        .highlight_symbol("▸ ");
    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(app.selected));
    frame.render_stateful_widget(list, body[0], &mut state);

    let detail_title = if app.show_help { " Help " } else if app.show_plan { " Installation plan (preview only) " } else { " Selected option " };
    let block = Block::default().title(detail_title).borders(Borders::ALL).border_style(Style::default().fg(Color::Blue));
    let detail = if app.show_help {
        Paragraph::new(help_text()).block(block).wrap(Wrap { trim: true })
    } else if app.show_plan {
        Paragraph::new(plan::format_plan(&app.config)).block(block).wrap(Wrap { trim: true })
    } else if matches!(app.selected, 6 | 7) {
        let options = app.multi_options();
        let mut lines = vec![
            Line::from(Span::styled(app.setting_label(app.selected), Style::default().add_modifier(Modifier::BOLD))),
            Line::from("Left / Right moves cursor; Space or Enter toggles."),
            Line::from(""),
        ];
        for (index, (name, selected)) in options.iter().enumerate() {
            let mark = if *selected { "[x]" } else { "[ ]" };
            let cursor = if index == app.multi_cursor { "▸" } else { " " };
            let style = if index == app.multi_cursor { Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Gray) };
            lines.push(Line::from(vec![Span::styled(format!("{cursor} {mark} "), style), Span::raw(name.clone())]));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(format!("Selected: {}", app.setting_value(app.selected)), Style::default().fg(Color::Yellow))));
        lines.push(Line::from(""));
        lines.push(Line::from(app.setting_description(app.selected)));
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true })
    } else {
        let content = format!(
            "{}\n\nCurrent value\n  {}\n\n{}\n\nSafety state\n  {}\n\nHardware summary\n  Firmware: {}\n  Memory: {}\n  Disks found: {}\n\nProfile summary\n  Init: {}\n  Root: {}\n  Desktops: {}\n  Kernels: {}\n  Bootloader: {}",
            app.setting_label(app.selected), app.setting_value(app.selected), app.setting_description(app.selected),
            if app.config.disk.target.is_none() { "No target disk selected; disk actions blocked." } else { "Target path configured; destructive operations remain disabled." },
            app.hardware.firmware,
            app.hardware.memory_bytes.map_or_else(|| "unknown".into(), format_bytes),
            app.hardware.disks.len(),
            app.config.system.init_system, app.config.disk.root_filesystem, app.setting_value(6), app.setting_value(7),
            app.config.software.bootloader,
        );
        Paragraph::new(content).block(block).wrap(Wrap { trim: true })
    };
    frame.render_widget(detail, body[1]);

    let footer = vec![
        Line::from(vec![
            keycap("↑/↓"), Span::raw(" Navigate  "),
            keycap("←/→"), Span::raw(" Change / cursor  "),
            keycap("D"), Span::raw(" Disks  "),
            keycap("P"), Span::raw(" Plan  "),
            keycap("S"), Span::raw(" Save  "),
            keycap("?"), Span::raw(" Help  "),
            keycap("Q"), Span::raw(" Quit"),
        ]),
        Line::from(Span::styled(app.status.as_str(), Style::default().fg(Color::Yellow))),
    ];
    frame.render_widget(Paragraph::new(footer).block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray))), outer[2]);
}

fn render_disks(frame: &mut Frame<'_>, app: &App) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(10), Constraint::Length(4)])
        .split(frame.area());
    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(" ARTIX ", Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled(" HARDWARE", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::styled("   /   READ-ONLY INVENTORY", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(Span::styled("No partitioning or formatting is available in this version.", Style::default().fg(Color::Gray))),
    ]).block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Cyan)));
    frame.render_widget(header, outer[0]);

    let body = Layout::default().direction(Direction::Horizontal)
        .constraints([Constraint::Length(38), Constraint::Min(30)]).split(outer[1]);
    let items: Vec<ListItem<'_>> = if app.hardware.disks.is_empty() {
        vec![ListItem::new(" No physical disks detected")]
    } else {
        app.hardware.disks.iter().map(|disk| {
            let marker = if disk.can_be_selected() { "○" } else { "×" };
            let removable = if disk.removable { " USB" } else { "" };
            ListItem::new(format!(" {marker} {}  {}{}", disk.name, format_bytes(disk.size_bytes), removable))
        }).collect()
    };
    let list = List::new(items)
        .block(Block::default().title(" Block devices ").borders(Borders::ALL))
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD))
        .highlight_symbol("▸ ");
    let mut state = ratatui::widgets::ListState::default();
    if !app.hardware.disks.is_empty() { state.select(Some(app.selected_disk)); }
    frame.render_stateful_widget(list, body[0], &mut state);

    let detail_block = Block::default().title(" Device details ").borders(Borders::ALL).border_style(Style::default().fg(Color::Blue));
    let mut lines = vec![
        Line::from(Span::styled("LIVE SYSTEM", Style::default().add_modifier(Modifier::BOLD))),
        Line::from(format!("Firmware: {}", app.hardware.firmware)),
        Line::from(format!("Memory: {}", app.hardware.memory_bytes.map_or_else(|| "unknown".into(), format_bytes))),
        Line::from(format!("CPU: {}", app.hardware.cpu_model.as_deref().or(app.hardware.cpu_vendor.as_deref()).unwrap_or("unknown"))),
        Line::from(format!("GPUs: {}", if app.hardware.gpus.is_empty() { "none detected".into() } else { app.hardware.gpus.iter().map(|g| g.vendor.as_str()).collect::<Vec<_>>().join(", ") })),
        Line::from(format!("Network: {}", if app.hardware.network_interfaces.is_empty() { "none detected".into() } else { app.hardware.network_interfaces.iter().map(|n| n.name.as_str()).collect::<Vec<_>>().join(", ") })),
        Line::from(""),
    ];
    if let Some(disk) = app.hardware.disks.get(app.selected_disk) {
        lines.push(Line::from(Span::styled(format!("{}  ({})", disk.path.display(), format_bytes(disk.size_bytes)), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))));
        lines.push(Line::from(format!("Model: {}", disk.model.as_deref().unwrap_or("unknown"))));
        lines.push(Line::from(format!("Vendor: {}", disk.vendor.as_deref().unwrap_or("unknown"))));
        lines.push(Line::from(format!("Serial / WWID: {}", disk.serial.as_deref().or(disk.wwid.as_deref()).unwrap_or("unknown"))));
        lines.push(Line::from(format!("Removable: {}   Read-only: {}", disk.removable, disk.read_only)));
        lines.push(Line::from(format!("Active swap: {}   Holders: {}", disk.active_swap, disk.has_holders)));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(format!("Partitions ({})", disk.partitions.len()), Style::default().add_modifier(Modifier::BOLD))));
        if disk.partitions.is_empty() {
            lines.push(Line::from("  No partitions detected"));
        }
        for partition in &disk.partitions {
            lines.push(Line::from(format!("  {}  {}", partition.path.display(), format_bytes(partition.size_bytes))));
            lines.push(Line::from(format!("    FS: {}   Label: {}", partition.filesystem.as_deref().unwrap_or("unknown"), partition.label.as_deref().unwrap_or("none"))));
            lines.push(Line::from(format!("    Mounted: {}   Active swap: {}   Holders: {}", if partition.mounted_at.is_empty() { "no".into() } else { partition.mounted_at.join(", ") }, partition.active_swap, partition.has_holders)));
        }
        lines.push(Line::from(""));
        if let Some(reason) = disk.selection_block_reason() {
            lines.push(Line::from(Span::styled(format!("NOT SELECTABLE: {reason}"), Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))));
        } else {
            lines.push(Line::from(Span::styled("Selectable for plan only.", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))));
        }
    } else {
        lines.push(Line::from("No physical block devices were found."));
    }
    if !app.hardware.warnings.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Discovery warnings:", Style::default().fg(Color::Yellow))));
        for warning in &app.hardware.warnings {
            lines.push(Line::from(format!("  {warning}")));
        }
    }
    frame.render_widget(Paragraph::new(lines).block(detail_block).wrap(Wrap { trim: true }), body[1]);

    let footer = vec![
        Line::from(vec![
            keycap("↑/↓"), Span::raw(" Select disk  "),
            keycap("Enter"), Span::raw(" Use in plan  "),
            keycap("D/Esc"), Span::raw(" Back  "),
            keycap("Q"), Span::raw(" Quit"),
        ]),
        Line::from(Span::styled(app.status.as_str(), Style::default().fg(Color::Yellow))),
    ];
    frame.render_widget(Paragraph::new(footer).block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray))), outer[2]);
}

fn keycap(value: &'static str) -> Span<'static> {
    Span::styled(format!(" {value} "), Style::default().fg(Color::Black).bg(Color::Gray))
}

fn help_text() -> String {
    [
        "ARTIX SETUP — KEYBOARD HELP", "",
        "Up / Down    Move through settings",
        "Left / Right Change a value, or move a multi-select cursor",
        "Space / Enter Toggle a desktop or kernel choice",
        "D            Open read-only hardware and disk inventory",
        "P            Toggle installation plan preview",
        "S            Save profile as ./artix-setup.toml",
        "?            Toggle this help screen",
        "Q / Esc      Quit", "",
        "DISK INVENTORY",
        "The D screen reads sysfs, procfs and udev metadata only.",
        "It blocks selection of mounted, read-only or held devices.",
        "Selecting a disk only stores its path in the plan; it cannot write to it.",
        "", "IMPORTANT",
        "This version is a UI and configuration prototype only.",
        "It does not execute commands or write to disks.",
        "Optional packages require live repository checks before installation.",
    ].join("\n")
}
