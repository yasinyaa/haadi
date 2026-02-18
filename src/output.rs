use super::*;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use std::io;
use std::time::Duration;

pub(crate) fn relative_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

pub(crate) fn print_human_report(report: &Report) {
    println!("Root: {}", report.root);
    println!("\nSummary:");
    println!(
        "  - Total source files: {}",
        report.summary.total_source_files
    );
    println!(
        "  - Total asset files: {}",
        report.summary.total_asset_files
    );
    println!(
        "  - Reachable source files: {}",
        report.summary.total_reachable_files
    );
    println!("  - Entry files: {}", report.summary.total_entries);
    println!(
        "  - Unresolved local imports: {}",
        report.summary.unresolved_local_imports
    );
    println!(
        "  - High-confidence graph: {}",
        report.summary.high_confidence_graph
    );
    println!(
        "  - Omitted risky findings: {}",
        report.summary.omitted_risky_findings
    );
    println!("  - Unused files: {}", report.summary.unused_files_count);
    println!("  - Unused assets: {}", report.summary.unused_assets_count);
    println!(
        "  - Unused dependencies: {}",
        report.summary.unused_dependencies_count
    );
    println!(
        "  - Unused exports: {}",
        report.summary.unused_exports_count
    );

    if report.entries.is_empty() {
        println!("Entries: (none detected)");
    } else {
        println!("Entries:");
        for entry in &report.entries {
            println!("  - {entry}");
        }
    }

    if !report.warnings.is_empty() {
        println!("\nWarnings:");
        for warning in &report.warnings {
            println!("  - {warning}");
        }
    }

    println!("\nUnused files ({}):", report.unused_files.len());
    for path in &report.unused_files {
        println!("  - {path}");
    }

    println!("\nUnused assets ({}):", report.unused_assets.len());
    for path in &report.unused_assets {
        println!("  - {path}");
    }

    println!(
        "\nUnused dependencies ({}):",
        report.unused_dependencies.len()
    );
    for dep in &report.unused_dependencies {
        println!("  - {dep}");
    }

    let mut grouped: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for item in &report.unused_exports {
        grouped
            .entry(item.file.as_str())
            .or_default()
            .push(item.export.as_str());
    }

    println!("\nUnused exports ({}):", report.unused_exports.len());
    for (file, exports) in grouped {
        println!("  - {file}");
        for export in exports {
            println!("      - {export}");
        }
    }
}

pub(crate) fn print_tui_report(report: &Report) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = run_tui_loop(&mut terminal, report);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_tui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    report: &Report,
) -> Result<()> {
    loop {
        terminal.draw(|frame| draw_dashboard(frame, report))?;

        if event::poll(Duration::from_millis(200))? {
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }

            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                _ => {}
            }
        }
    }

    Ok(())
}

fn draw_dashboard(frame: &mut Frame, report: &Report) {
    let root_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(8),
            Constraint::Min(8),
            Constraint::Min(8),
        ])
        .split(frame.area());

    let title = Paragraph::new(format!("haadi summary | {} | press q to quit", report.root))
        .block(Block::default().borders(Borders::ALL).title("Report"));
    frame.render_widget(title, root_chunks[0]);

    let summary = Paragraph::new(vec![
        Line::from(format!(
            "sources {} | assets {} | reachable {} | entries {}",
            report.summary.total_source_files,
            report.summary.total_asset_files,
            report.summary.total_reachable_files,
            report.summary.total_entries
        )),
        Line::from(format!(
            "unused files {} | unused assets {} | unused deps {} | unused exports {}",
            report.summary.unused_files_count,
            report.summary.unused_assets_count,
            report.summary.unused_dependencies_count,
            report.summary.unused_exports_count
        )),
        Line::from(format!(
            "unresolved locals {} | high-confidence {} | omitted risky {}",
            report.summary.unresolved_local_imports,
            report.summary.high_confidence_graph,
            report.summary.omitted_risky_findings
        )),
    ])
    .block(Block::default().borders(Borders::ALL).title("Summary"))
    .wrap(Wrap { trim: true });
    frame.render_widget(summary, root_chunks[1]);

    let middle = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(root_chunks[2]);

    let warnings_items: Vec<ListItem> = if report.warnings.is_empty() {
        vec![ListItem::new("(none)")]
    } else {
        report
            .warnings
            .iter()
            .take(8)
            .map(|w| ListItem::new(w.as_str()))
            .collect()
    };
    frame.render_widget(
        List::new(warnings_items).block(Block::default().borders(Borders::ALL).title("Warnings")),
        middle[0],
    );

    let entries_items: Vec<ListItem> = if report.entries.is_empty() {
        vec![ListItem::new("(none)")]
    } else {
        report
            .entries
            .iter()
            .take(8)
            .map(|e| ListItem::new(e.as_str()))
            .collect()
    };
    frame.render_widget(
        List::new(entries_items).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Entries (top)"),
        ),
        middle[1],
    );

    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(root_chunks[3]);

    frame.render_widget(
        List::new(top_items(&report.unused_dependencies, 10))
            .block(Block::default().borders(Borders::ALL).title("Unused deps")),
        bottom[0],
    );

    frame.render_widget(
        List::new(top_items(&report.unused_assets, 10)).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Unused assets"),
        ),
        bottom[1],
    );

    let exports: Vec<String> = report
        .unused_exports
        .iter()
        .take(10)
        .map(|e| format!("{} -> {}", e.file, e.export))
        .collect();
    frame.render_widget(
        List::new(top_items(&exports, 10)).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Unused exports"),
        ),
        bottom[2],
    );
}

fn top_items(items: &[String], limit: usize) -> Vec<ListItem<'_>> {
    if items.is_empty() {
        return vec![ListItem::new("(none)")];
    }

    items
        .iter()
        .take(limit)
        .map(|v| ListItem::new(v.as_str()))
        .collect()
}
