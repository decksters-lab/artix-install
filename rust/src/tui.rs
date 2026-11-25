use std::io::{self, Stdout};

use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};

use crate::{
    app::{App, PartitionRole, SETTING_COUNT},
    config::InstallConfig,
    discovery::{HardwareSnapshot, format_bytes},
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
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if app.show_storage {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('m') => app.toggle_storage(),
                    KeyCode::Up | KeyCode::Char('k') => app.move_partition_selection(-1),
                    KeyCode::Down | KeyCode::Char('j') => app.move_partition_selection(1),
                    KeyCode::Char('r') => app.assign_partition(PartitionRole::Root),
                    KeyCode::Char('e') => app.assign_partition(PartitionRole::Efi),
                    KeyCode::Char('w') => app.assign_partition(PartitionRole::Swap),
                    KeyCode::Char('p') => app.show_plan = !app.show_plan,
                    _ => {}
                }
                continue;
            }
            if app.show_disks {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Left | KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('d') => {
                        app.toggle_disks()
                    }
                    KeyCode::Up | KeyCode::Char('k') => app.move_disk_selection(-1),
                    KeyCode::Down | KeyCode::Char('j') => app.move_disk_selection(1),
                    KeyCode::Enter => app.select_disk(),
                    KeyCode::Char('m') => {
                        app.toggle_disks();
                        app.toggle_storage();
                    }
                    _ => {}
                }
                continue;
            }
            match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Esc => {
                    if app.section_open {
                        app.section_open = false;
                    } else {
                        break;
                    }
                }
                KeyCode::Char('d') if !app.section_open => app.toggle_disks(),
                KeyCode::Char('m') if !app.section_open => app.toggle_storage(),
                KeyCode::Up | KeyCode::Char('k') if app.section_open => {
                    app.move_choice_cursor(-1);
                }
                KeyCode::Down | KeyCode::Char('j') if app.section_open => {
                    app.move_choice_cursor(1);
                }
                KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
                KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
                KeyCode::Left | KeyCode::Char('h') if app.section_open => {
                    app.section_open = false;
                    app.status = format!(
                        "{}: {}",
                        app.setting_label(app.selected),
                        app.setting_value(app.selected)
                    );
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    if app.section_open {
                        // Right is intentionally inert while an option list is open.
                    } else if app.selected == 1 {
                        app.toggle_disks();
                    } else {
                        app.enter_section();
                    }
                }
                KeyCode::Enter if app.section_open => {
                    app.confirm_section_choice();
                }
                KeyCode::Char(' ') if app.section_open && matches!(app.selected, 6 | 7) => {
                    app.toggle_multi_selection();
                }
                KeyCode::Enter if !app.section_open => {
                    if app.selected == 1 {
                        app.toggle_disks();
                    } else {
                        app.enter_section();
                    }
                }
                KeyCode::Char(' ') if app.section_open && matches!(app.selected, 6 | 7) => {
                    app.toggle_multi_selection();
                }
                KeyCode::Char('p') if !app.section_open => app.show_plan = !app.show_plan,
                KeyCode::Char('?') if !app.section_open => app.show_help = !app.show_help,
                KeyCode::Char('s') if !app.section_open => {
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
    if app.show_storage {
        render_storage(frame, app);
        return;
    }
    if app.show_disks {
        render_disks(frame, app);
        return;
    }
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(12),
            Constraint::Length(4),
        ])
        .split(frame.area());
    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                " ARTIX ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " SETUP",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "   /   Rust prototype 0.1.0",
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(Span::styled(
            "A modular, configuration-driven Artix installer",
            Style::default().fg(Color::Gray),
        )),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );
    frame.render_widget(header, outer[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(27), Constraint::Min(30)])
        .split(outer[1]);
    let items: Vec<ListItem<'_>> = (0..SETTING_COUNT)
        .map(|index| {
            let marker = if index == 1 && app.config.disk.target.is_none() {
                "○"
            } else {
                "●"
            };
            ListItem::new(format!(" {marker} {:<19}", app.setting_label(index)))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::default()
                .title(" Configuration ")
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(app.selected));
    frame.render_stateful_widget(list, body[0], &mut state);

    let detail_title = if app.show_help {
        " Help "
    } else if app.show_plan {
        " Installation plan (preview only) "
    } else {
        " Selected option "
    };
    let block = Block::default()
        .title(detail_title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue));
    let detail = if app.show_help {
        Paragraph::new(help_text())
            .block(block)
            .wrap(Wrap { trim: true })
    } else if app.show_plan {
        Paragraph::new(plan::format_plan(&app.config))
            .block(block)
            .wrap(Wrap { trim: true })
    } else if app.section_open {
        let multi = matches!(app.selected, 6 | 7);
        let options = if multi {
            app.multi_options()
        } else {
            app.single_options()
        };
        let count = options.len();
        let panel = block.inner(body[1]);
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(3),
                Constraint::Length(2),
                Constraint::Min(2),
            ])
            .split(panel);

        let instruction = if multi {
            "↑/↓ move   Space toggle   Enter finish   ← back"
        } else {
            "↑/↓ choose   Enter confirm   ← back"
        };
        frame.render_widget(Paragraph::new(instruction), sections[0]);

        let items: Vec<ListItem<'_>> = options
            .iter()
            .map(|(name, selected)| {
                let marker = if multi {
                    if *selected { "[x]" } else { "[ ]" }
                } else if *selected {
                    "(*)"
                } else {
                    "( )"
                };
                ListItem::new(format!(" {marker} {name}"))
            })
            .collect();
        let list = List::new(items)
            .block(
                Block::default()
                    .title(format!(
                        " {} — {count} choices ",
                        app.setting_label(app.selected)
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::DarkGray)),
            )
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▸ ");
        let mut list_state = ratatui::widgets::ListState::default();
        let cursor = app.multi_cursor.min(count.saturating_sub(1));
        if count > 0 {
            list_state.select(Some(cursor));
        }
        frame.render_stateful_widget(list, sections[1], &mut list_state);

        frame.render_widget(
            Paragraph::new(format!(
                "Choice {} of {count}   Current: {}",
                cursor + 1,
                app.setting_value(app.selected)
            ))
            .wrap(Wrap { trim: true }),
            sections[2],
        );
        frame.render_widget(
            Paragraph::new(app.setting_description(app.selected)).wrap(Wrap { trim: true }),
            sections[3],
        );
        frame.render_widget(block, body[1]);
        return;
    } else {
        let content = format!(
            "{}\n\nCurrent value\n  {}\n\n{}\n\nSafety state\n  {}\n\nHardware summary\n  Firmware: {}\n  Memory: {}\n  Disks found: {}\n\nProfile summary\n  Init: {}\n  Root: {}\n  Desktops: {}\n  Kernels: {}\n  Bootloader: {}",
            app.setting_label(app.selected),
            app.setting_value(app.selected),
            app.setting_description(app.selected),
            if app.config.disk.target.is_none() {
                "No target disk selected; disk actions blocked."
            } else {
                "Target path configured; destructive operations remain disabled."
            },
            app.hardware.firmware,
            app.hardware
                .memory_bytes
                .map_or_else(|| "unknown".into(), format_bytes),
            app.hardware.disks.len(),
            app.config.system.init_system,
            app.config.disk.root_filesystem,
            app.setting_value(6),
            app.setting_value(7),
            app.config.software.bootloader,
        );
        Paragraph::new(content)
            .block(block)
            .wrap(Wrap { trim: true })
    };
    frame.render_widget(detail, body[1]);

    let footer = vec![
        Line::from(vec![
            keycap("↑/↓"),
            Span::raw(if app.section_open {
                " Choose option  "
            } else {
                " Navigate sections  "
            }),
            keycap(if app.section_open {
                "Enter"
            } else {
                "→/Enter"
            }),
            Span::raw(if app.section_open {
                " Confirm / finish  "
            } else {
                " Open section  "
            }),
            keycap("←"),
            Span::raw(" Back  "),
            keycap("D"),
            Span::raw(" Disks  "),
            keycap("M"),
            Span::raw(" Partitions  "),
            keycap("P"),
            Span::raw(" Plan  "),
            keycap("S"),
            Span::raw(" Save  "),
            keycap("?"),
            Span::raw(" Help  "),
            keycap("Q"),
            Span::raw(" Quit"),
        ]),
        Line::from(Span::styled(
            app.status.as_str(),
            Style::default().fg(Color::Yellow),
        )),
    ];
    frame.render_widget(
        Paragraph::new(footer).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        outer[2],
    );
}

fn render_storage(frame: &mut Frame<'_>, app: &App) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(10),
            Constraint::Length(4),
        ])
        .split(frame.area());
    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                " ARTIX ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " PARTITION MANAGER",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "   /   PLAN ONLY — NO DISK WRITES",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            "Automatic layout preview, or assign existing partitions in Manual / Dual-boot mode.",
            Style::default().fg(Color::Gray),
        )),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );
    frame.render_widget(header, outer[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(42), Constraint::Min(30)])
        .split(outer[1]);
    let partitions = app.target_partitions();
    let items: Vec<ListItem<'_>> = if app.config.disk.target.is_none() {
        vec![ListItem::new(" Choose a target disk first (D)")]
    } else if partitions.is_empty() {
        vec![ListItem::new(
            " No partitions detected on the selected disk",
        )]
    } else {
        partitions
            .iter()
            .map(|part| {
                let role = app.partition_role(&part.path).unwrap_or("");
                let prefix = if role.is_empty() { "[ ]" } else { "[x]" };
                let suffix = if role.is_empty() {
                    String::new()
                } else {
                    format!(" <{role}>")
                };
                let fs = part.filesystem.as_deref().unwrap_or("unknown");
                ListItem::new(format!(
                    " {prefix} {}  {}  {}{}",
                    part.name,
                    format_bytes(part.size_bytes),
                    fs,
                    suffix
                ))
            })
            .collect()
    };
    let title = match app.config.disk.partitioning {
        crate::config::Partitioning::Automatic => " Detected partitions ",
        crate::config::Partitioning::Manual => " Manual assignments ",
        crate::config::Partitioning::DualBoot => " Dual-boot assignments ",
    };
    let list = List::new(items)
        .block(Block::default().title(title).borders(Borders::ALL))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    let mut state = ratatui::widgets::ListState::default();
    if !partitions.is_empty() {
        state.select(Some(app.selected_partition.min(partitions.len() - 1)));
    }
    frame.render_stateful_widget(list, body[0], &mut state);

    let detail_block = Block::default()
        .title(" Layout / role preview ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue));
    let detail = if app.show_plan {
        Paragraph::new(plan::format_plan(&app.config))
            .block(detail_block)
            .wrap(Wrap { trim: true })
    } else {
        let target = app
            .config
            .disk
            .target
            .as_ref()
            .map_or("NOT SELECTED".to_owned(), |p| p.display().to_string());
        let mut lines = vec![
            Line::from(Span::styled(
                "CURRENT STORAGE CONFIGURATION",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(format!("Target: {target}")),
            Line::from(format!("Mode: {}", app.config.disk.partitioning)),
            Line::from(format!("Firmware: {}", app.config.system.firmware)),
            Line::from(format!(
                "Root filesystem: {}",
                app.config.disk.root_filesystem
            )),
            Line::from(format!(
                "LUKS2: {}",
                if app.config.disk.encryption {
                    "requested"
                } else {
                    "off"
                }
            )),
            Line::from(format!(
                "Swap: {} ({} GiB)",
                app.config.disk.swap.mode, app.config.disk.swap.size_gib
            )),
            Line::from(format!("Btrfs layout: {}", app.config.disk.btrfs.layout)),
            Line::from(""),
            Line::from(Span::styled(
                "ASSIGNED PARTITIONS",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(format!(
                "Root: {}",
                app.config
                    .disk
                    .root_partition
                    .as_ref()
                    .map_or("not assigned".into(), |p| p.display().to_string())
            )),
            Line::from(format!(
                "EFI: {}",
                app.config
                    .disk
                    .efi_partition
                    .as_ref()
                    .map_or("not assigned".into(), |p| p.display().to_string())
            )),
            Line::from(format!(
                "Swap: {}",
                app.config
                    .disk
                    .swap_partition
                    .as_ref()
                    .map_or("not assigned".into(), |p| p.display().to_string())
            )),
            Line::from(""),
        ];
        if let Some(part) = partitions.get(app.selected_partition) {
            lines.push(Line::from(Span::styled(
                format!("SELECTED: {}", part.path.display()),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(format!(
                "Filesystem: {}",
                part.filesystem.as_deref().unwrap_or("unknown")
            )));
            lines.push(Line::from(format!(
                "Label: {}",
                part.label.as_deref().unwrap_or("none")
            )));
            lines.push(Line::from(format!(
                "Mounted: {}",
                if part.mounted_at.is_empty() {
                    "no".into()
                } else {
                    part.mounted_at.join(", ")
                }
            )));
            lines.push(Line::from(format!(
                "Active swap: {}   Read-only: {}   Holders: {}",
                part.active_swap, part.read_only, part.has_holders
            )));
            if !part.safety_checks_complete {
                lines.push(Line::from(Span::styled(
                    "Safety metadata incomplete — assignment blocked.",
                    Style::default().fg(Color::Red),
                )));
            }
        } else if partitions.is_empty() {
            lines.push(Line::from(
                "No target disk or partition table is available.",
            ));
        }
        lines.push(Line::from(""));
        match app.config.disk.partitioning {
            crate::config::Partitioning::Automatic => {
                lines.push(Line::from("Automatic mode: layout is generated from firmware, root filesystem, swap and encryption settings."));
                lines.push(Line::from(
                    "The new ESP in an automatic UEFI layout is formatted as FAT32.",
                ));
            }
            crate::config::Partitioning::Manual => {
                lines.push(Line::from(
                    "Manual mode assigns partitions that already exist.",
                ));
                lines.push(Line::from(
                    "Create/resize partitions outside this prototype, then assign them here.",
                ));
                lines.push(Line::from(
                    "R = root   E = EFI   W = swap (when swap mode is Partition)",
                ));
                lines.push(Line::from(format!(
                    "EFI policy: {}. Preserve is safest for existing boot files.",
                    app.config.disk.efi_policy
                )));
            }
            crate::config::Partitioning::DualBoot => {
                lines.push(Line::from(
                    "Dual-boot preserves the existing partition table.",
                ));
                lines.push(Line::from(
                    "Only the selected Artix root partition is planned for formatting.",
                ));
                lines.push(Line::from(
                    "The selected EFI partition is ALWAYS preserved; it is never formatted.",
                ));
                lines.push(Line::from("R = root   E = existing EFI"));
            }
        }
        Paragraph::new(lines)
            .block(detail_block)
            .wrap(Wrap { trim: true })
    };
    frame.render_widget(detail, body[1]);

    let footer = vec![
        Line::from(vec![
            keycap("↑/↓"),
            Span::raw(" Select partition  "),
            keycap("R"),
            Span::raw(" Root  "),
            keycap("E"),
            Span::raw(" EFI  "),
            keycap("W"),
            Span::raw(" Swap  "),
            keycap("P"),
            Span::raw(" Preview  "),
            keycap("M/Esc"),
            Span::raw(" Back"),
        ]),
        Line::from(Span::styled(
            app.status.as_str(),
            Style::default().fg(Color::Yellow),
        )),
    ];
    frame.render_widget(
        Paragraph::new(footer).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        outer[2],
    );
}

fn render_disks(frame: &mut Frame<'_>, app: &App) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(12),
            Constraint::Length(4),
        ])
        .split(frame.area());
    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                " ARTIX ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " SETUP",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "   /   TARGET DISK",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            "Read-only hardware inventory. Disk selection changes the plan only; no disk writes occur.",
            Style::default().fg(Color::Gray),
        )),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );
    frame.render_widget(header, outer[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(27), Constraint::Min(30)])
        .split(outer[1]);

    let settings: Vec<ListItem<'_>> = (0..SETTING_COUNT)
        .map(|index| {
            let marker = if index == 1 && app.config.disk.target.is_none() {
                "○"
            } else {
                "●"
            };
            ListItem::new(format!(" {marker} {:<19}", app.setting_label(index)))
        })
        .collect();
    let settings_list = List::new(settings)
        .block(
            Block::default()
                .title(" Configuration ")
                .borders(Borders::ALL),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    let mut settings_state = ratatui::widgets::ListState::default();
    settings_state.select(Some(app.selected));
    frame.render_stateful_widget(settings_list, body[0], &mut settings_state);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(6), Constraint::Min(6)])
        .split(body[1]);

    let disk_items: Vec<ListItem<'_>> = if app.hardware.disks.is_empty() {
        vec![ListItem::new(" No physical disks detected")]
    } else {
        app.hardware
            .disks
            .iter()
            .map(|disk| {
                let marker = if app.config.disk.target.as_ref() == Some(&disk.path) {
                    "(*)"
                } else if disk.can_be_selected() {
                    "( )"
                } else {
                    "(x)"
                };
                let removable = if disk.removable { "  USB" } else { "" };
                ListItem::new(format!(
                    " {marker} {}  {}{}",
                    disk.name,
                    format_bytes(disk.size_bytes),
                    removable
                ))
            })
            .collect()
    };
    let disk_list = List::new(disk_items)
        .block(
            Block::default()
                .title(" Available disks — ↑/↓ select, Enter choose ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Blue)),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    let mut disk_state = ratatui::widgets::ListState::default();
    if !app.hardware.disks.is_empty() {
        disk_state.select(Some(app.selected_disk));
    }
    frame.render_stateful_widget(disk_list, right[0], &mut disk_state);

    let mut details = vec![Line::from(Span::styled(
        "SELECTED DEVICE DETAILS",
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    if let Some(disk) = app.hardware.disks.get(app.selected_disk) {
        details.push(Line::from(format!(
            "{}  ({})",
            disk.path.display(),
            format_bytes(disk.size_bytes)
        )));
        details.push(Line::from(format!(
            "Model: {}   Vendor: {}",
            disk.model.as_deref().unwrap_or("unknown"),
            disk.vendor.as_deref().unwrap_or("unknown")
        )));
        details.push(Line::from(format!(
            "Removable: {}   Read-only: {}   Active swap: {}",
            disk.removable, disk.read_only, disk.active_swap
        )));
        details.push(Line::from(format!(
            "Partitions: {}   Holders: {}",
            disk.partitions.len(),
            disk.has_holders
        )));
        if let Some(reason) = disk.selection_block_reason() {
            details.push(Line::from(Span::styled(
                format!("NOT SELECTABLE: {reason}"),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )));
        } else {
            details.push(Line::from(Span::styled(
                "Selectable for the plan only.",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )));
        }
    } else {
        details.push(Line::from("No physical block devices were found."));
    }
    if !app.hardware.warnings.is_empty() {
        details.push(Line::from(Span::styled(
            format!("Discovery warnings: {}", app.hardware.warnings.join("; ")),
            Style::default().fg(Color::Yellow),
        )));
    }
    frame.render_widget(
        Paragraph::new(details)
            .block(
                Block::default()
                    .title(" Device details ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Blue)),
            )
            .wrap(Wrap { trim: true }),
        right[1],
    );

    let footer = vec![
        Line::from(vec![
            keycap("↑/↓"),
            Span::raw(" Select disk  "),
            keycap("Enter"),
            Span::raw(" Use in plan  "),
            keycap("←/Esc"),
            Span::raw(" Back to configuration  "),
            keycap("Q"),
            Span::raw(" Quit"),
        ]),
        Line::from(Span::styled(
            app.status.as_str(),
            Style::default().fg(Color::Yellow),
        )),
    ];
    frame.render_widget(
        Paragraph::new(footer).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        outer[2],
    );
}

fn keycap(value: &'static str) -> Span<'static> {
    Span::styled(
        format!(" {value} "),
        Style::default().fg(Color::Black).bg(Color::Gray),
    )
}

fn help_text() -> String {
    [
        "ARTIX SETUP — KEYBOARD HELP",
        "",
        "Up / Down    Move through settings; inside a section, move through choices",
        "Right / Enter Open a settings section",
        "Left / Esc   Back out of an open section",
        "Space        Toggle a desktop or kernel checkbox",
        "Enter        Confirm and leave a section",
        "D            Open read-only hardware and disk inventory",
        "M            Open partition manager and assign existing partitions",
        "P            Toggle installation plan preview",
        "S            Save profile as ./artix-setup.toml",
        "?            Toggle this help screen",
        "Q / Esc      Quit from the main settings list",
        "",
        "DISK INVENTORY",
        "The D screen reads sysfs, procfs and udev metadata only.",
        "It blocks selection of mounted, read-only or held devices.",
        "Selecting a disk only stores its path in the plan; it cannot write to it.",
        "",
        "IMPORTANT",
        "This version is a UI and configuration prototype only.",
        "It does not execute commands or write to disks.",
        "Optional packages require live repository checks before installation.",
    ]
    .join("\n")
}
