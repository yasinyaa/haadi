use super::*;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use regex::RegexBuilder;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiPage {
    Summary,
    Delete,
}

#[derive(Debug, Clone)]
struct DeleteCandidate {
    rel_path: String,
    kind: &'static str,
    state: CandidateState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateState {
    Active,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeleteFilter {
    All,
    Files,
    Assets,
}

impl DeleteFilter {
    fn next(self) -> Self {
        match self {
            DeleteFilter::All => DeleteFilter::Files,
            DeleteFilter::Files => DeleteFilter::Assets,
            DeleteFilter::Assets => DeleteFilter::All,
        }
    }

    fn label(self) -> &'static str {
        match self {
            DeleteFilter::All => "all",
            DeleteFilter::Files => "files",
            DeleteFilter::Assets => "assets",
        }
    }
}

#[derive(Debug)]
struct DeleteState {
    items: Vec<DeleteCandidate>,
    selected: BTreeSet<usize>,
    cursor: usize,
    confirm_delete: bool,
    confirm_empty_trash: bool,
    confirm_restore_previous: bool,
    confirm_restore_all: bool,
    filter: DeleteFilter,
    search_query: String,
    search_input: String,
    editing_search: bool,
    message: String,
    root: PathBuf,
    trash_root: PathBuf,
    undo_stack: Vec<Vec<DeletedEntry>>,
}

#[derive(Debug, Clone)]
struct DeletedEntry {
    candidate: DeleteCandidate,
    original_abs: PathBuf,
    trash_abs: PathBuf,
}

#[derive(Debug, Serialize)]
struct DeleteLogRecord {
    action: &'static str,
    batch_id: String,
    kind: String,
    rel_path: String,
    original_abs: String,
    trash_abs: String,
    ts_unix_ms: u128,
}

#[derive(Debug)]
struct TuiState {
    page: TuiPage,
    delete: DeleteState,
}

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
    println!("  - Used assets: {}", report.summary.used_assets_count);
    println!("  - Unused assets: {}", report.summary.unused_assets_count);
    println!(
        "  - Asset usage coverage: {:.1}%",
        report.summary.asset_usage_coverage_pct
    );
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

    println!("\nUsed assets ({}):", report.used_assets.len());
    for path in &report.used_assets {
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

    let mut state = TuiState {
        page: TuiPage::Summary,
        delete: DeleteState {
            items: build_delete_candidates(report),
            selected: BTreeSet::new(),
            cursor: 0,
            confirm_delete: false,
            confirm_empty_trash: false,
            confirm_restore_previous: false,
            confirm_restore_all: false,
            filter: DeleteFilter::All,
            search_query: String::new(),
            search_input: String::new(),
            editing_search: false,
            message: "Select unused files/assets, then press x and confirm with y.".to_string(),
            root: PathBuf::from(&report.root),
            trash_root: PathBuf::from(&report.root).join(".haadi_trash"),
            undo_stack: Vec::new(),
        },
    };
    let _ = hydrate_deleted_candidates_from_trash(&mut state.delete);

    let result = run_tui_loop(&mut terminal, report, &mut state);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_tui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    report: &Report,
    state: &mut TuiState,
) -> Result<()> {
    loop {
        terminal.draw(|frame| draw_page(frame, report, state))?;

        if event::poll(Duration::from_millis(200))? {
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }

            match state.page {
                TuiPage::Summary => {
                    if handle_summary_key(key.code, state) {
                        break;
                    }
                }
                TuiPage::Delete => {
                    if handle_delete_key(key.code, state)? {
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

fn handle_summary_key(code: KeyCode, state: &mut TuiState) -> bool {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => true,
        KeyCode::Char('d') => {
            state.page = TuiPage::Delete;
            false
        }
        _ => false,
    }
}

fn handle_delete_key(code: KeyCode, state: &mut TuiState) -> Result<bool> {
    if state.delete.editing_search {
        match code {
            KeyCode::Enter => {
                state.delete.search_query = state.delete.search_input.clone();
                state.delete.editing_search = false;
                state.delete.message = format!(
                    "Search applied: '{}'.",
                    if state.delete.search_query.is_empty() {
                        "(none)"
                    } else {
                        state.delete.search_query.as_str()
                    }
                );
                clamp_delete_cursor(&mut state.delete);
            }
            KeyCode::Esc => {
                state.delete.editing_search = false;
                state.delete.search_input.clear();
                state.delete.message = "Search edit canceled.".to_string();
            }
            KeyCode::Backspace => {
                state.delete.search_input.pop();
            }
            KeyCode::Char(c) => {
                state.delete.search_input.push(c);
            }
            _ => {}
        }
        return Ok(false);
    }

    if state.delete.confirm_delete {
        match code {
            KeyCode::Char('y') => {
                apply_selected_deletions(&mut state.delete)?;
                state.delete.confirm_delete = false;
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                state.delete.confirm_delete = false;
                state.delete.message = "Deletion canceled.".to_string();
            }
            _ => {}
        }
        return Ok(false);
    }

    if state.delete.confirm_empty_trash {
        match code {
            KeyCode::Char('y') => {
                empty_trash(&mut state.delete)?;
                state.delete.confirm_empty_trash = false;
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                state.delete.confirm_empty_trash = false;
                state.delete.message = "Empty trash canceled.".to_string();
            }
            _ => {}
        }
        return Ok(false);
    }

    if state.delete.confirm_restore_previous {
        match code {
            KeyCode::Char('y') => {
                restore_previous_session(&mut state.delete)?;
                state.delete.confirm_restore_previous = false;
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                state.delete.confirm_restore_previous = false;
                state.delete.message = "Restore previous session canceled.".to_string();
            }
            _ => {}
        }
        return Ok(false);
    }

    if state.delete.confirm_restore_all {
        match code {
            KeyCode::Char('y') => {
                restore_all_sessions(&mut state.delete)?;
                state.delete.confirm_restore_all = false;
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                state.delete.confirm_restore_all = false;
                state.delete.message = "Restore all sessions canceled.".to_string();
            }
            _ => {}
        }
        return Ok(false);
    }

    match code {
        KeyCode::Char('q') => Ok(true),
        KeyCode::Char('b') | KeyCode::Esc => {
            state.page = TuiPage::Summary;
            Ok(false)
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let filtered = filtered_indices(&state.delete);
            if !filtered.is_empty() && state.delete.cursor > 0 {
                state.delete.cursor = state.delete.cursor.saturating_sub(1);
            }
            Ok(false)
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let filtered = filtered_indices(&state.delete);
            if state.delete.cursor + 1 < filtered.len() {
                state.delete.cursor += 1;
            }
            Ok(false)
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            toggle_selected(&mut state.delete);
            Ok(false)
        }
        KeyCode::Char('a') => {
            let filtered = filtered_indices(&state.delete);
            state.delete.selected = filtered.into_iter().collect();
            state.delete.message = format!("Selected {} items.", state.delete.selected.len());
            Ok(false)
        }
        KeyCode::Char('c') => {
            state.delete.selected.clear();
            state.delete.message = "Selection cleared.".to_string();
            Ok(false)
        }
        KeyCode::Char('x') => {
            if state.delete.selected.is_empty() {
                state.delete.message = "No items selected for deletion.".to_string();
            } else {
                state.delete.confirm_delete = true;
                state.delete.message = format!(
                    "Confirm delete {} selected files? Press y to confirm, n to cancel.",
                    state.delete.selected.len()
                );
            }
            Ok(false)
        }
        KeyCode::Char('z') => {
            state.delete.confirm_empty_trash = true;
            state.delete.message =
                "Empty trash and clear undo history? Press y to confirm, n to cancel.".to_string();
            Ok(false)
        }
        KeyCode::Char('r') => {
            state.delete.confirm_restore_previous = true;
            state.delete.message =
                "Restore most recent previous trash session? Press y to confirm, n to cancel."
                    .to_string();
            Ok(false)
        }
        KeyCode::Char('R') => {
            state.delete.confirm_restore_all = true;
            state.delete.message =
                "Restore ALL trash sessions? Press y to confirm, n to cancel.".to_string();
            Ok(false)
        }
        KeyCode::Char('u') => {
            undo_last_deletion(&mut state.delete)?;
            Ok(false)
        }
        KeyCode::Char('i') => {
            restore_specific_file_from_trash(&mut state.delete)?;
            Ok(false)
        }
        KeyCode::Char('o') => {
            restore_folder_from_trash(&mut state.delete)?;
            Ok(false)
        }
        KeyCode::Char('f') => {
            state.delete.filter = state.delete.filter.next();
            clamp_delete_cursor(&mut state.delete);
            state.delete.message = format!("Filter: {}", state.delete.filter.label());
            Ok(false)
        }
        KeyCode::Char('g') => {
            reset_filter_and_search(&mut state.delete);
            Ok(false)
        }
        KeyCode::Char('/') => {
            state.delete.editing_search = true;
            state.delete.search_input = state.delete.search_query.clone();
            state.delete.message = "Search mode: type and press Enter to apply.".to_string();
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn reset_filter_and_search(state: &mut DeleteState) {
    state.filter = DeleteFilter::All;
    state.search_query.clear();
    state.search_input.clear();
    state.editing_search = false;
    clamp_delete_cursor(state);
    state.message = "Reset filter and search.".to_string();
}

fn toggle_selected(state: &mut DeleteState) {
    let filtered = filtered_indices(state);
    if filtered.is_empty() {
        return;
    }
    let idx = filtered[state.cursor];

    if state.selected.contains(&idx) {
        state.selected.remove(&idx);
    } else {
        state.selected.insert(idx);
    }

    state.message = format!("Selected {} items.", state.selected.len());
}

fn apply_selected_deletions(state: &mut DeleteState) -> Result<()> {
    if state.selected.is_empty() {
        state.message = "No items selected for deletion.".to_string();
        return Ok(());
    }

    let root = fs::canonicalize(&state.root).unwrap_or_else(|_| state.root.clone());
    let mut deleted_indices = Vec::new();
    let mut deleted_entries = Vec::new();
    let mut failed = 0usize;
    let batch_id = generate_batch_id();

    for idx in state.selected.iter().copied() {
        let Some(item) = state.items.get(idx) else {
            continue;
        };
        if item.state == CandidateState::Deleted {
            failed += 1;
            continue;
        }

        let joined = root.join(&item.rel_path);
        let absolute = fs::canonicalize(&joined).unwrap_or(joined.clone());
        if !absolute.starts_with(&root) {
            failed += 1;
            continue;
        }
        if !absolute.is_file() {
            failed += 1;
            continue;
        }

        match move_to_trash(&root, &state.trash_root, item, &absolute, &batch_id) {
            Ok(entry) => {
                deleted_indices.push(idx);
                deleted_entries.push(entry);
            }
            Err(_) => failed += 1,
        }
    }

    deleted_indices.sort_unstable();
    deleted_indices.dedup();
    for idx in deleted_indices.iter().copied() {
        if let Some(item) = state.items.get_mut(idx) {
            item.state = CandidateState::Deleted;
        }
    }

    state.selected.clear();
    clamp_delete_cursor(state);

    let deleted = deleted_indices.len();
    if !deleted_entries.is_empty() {
        write_delete_log(&state.trash_root, "delete", &batch_id, &deleted_entries)?;
        state.undo_stack.push(deleted_entries);
    }
    state.message = format!("Deleted {deleted} files. Failed: {failed}. Press 'u' to undo.");

    Ok(())
}

fn undo_last_deletion(state: &mut DeleteState) -> Result<()> {
    let Some(mut last_batch) = state.undo_stack.pop() else {
        state.message = "Nothing to undo.".to_string();
        return Ok(());
    };

    let mut restored = 0usize;
    let mut failed = 0usize;
    let mut restored_candidates = Vec::new();
    let mut restored_entries = Vec::new();
    let batch_id = generate_batch_id();

    for entry in last_batch.drain(..) {
        if let Some(parent) = entry.original_abs.parent() {
            fs::create_dir_all(parent)?;
        }

        if entry.original_abs.exists() {
            failed += 1;
            continue;
        }

        match fs::rename(&entry.trash_abs, &entry.original_abs) {
            Ok(_) => {
                restored += 1;
                restored_candidates.push(entry.candidate.clone());
                restored_entries.push(entry);
            }
            Err(_) => failed += 1,
        }
    }

    for candidate in restored_candidates {
        upsert_candidate_state(
            &mut state.items,
            &candidate.rel_path,
            candidate.kind,
            CandidateState::Active,
        );
    }
    state
        .items
        .sort_by(|a, b| a.rel_path.cmp(&b.rel_path).then_with(|| a.kind.cmp(b.kind)));
    state.selected.clear();
    clamp_delete_cursor(state);

    state.message = format!("Restored {restored} files. Failed: {failed}.");

    // Undo log records are informational and should not block UX.
    if !restored_entries.is_empty() {
        let _ = write_delete_log(&state.trash_root, "undo", &batch_id, &restored_entries);
    }

    Ok(())
}

fn move_to_trash(
    root: &Path,
    trash_root: &Path,
    item: &DeleteCandidate,
    original_abs: &Path,
    batch_id: &str,
) -> Result<DeletedEntry> {
    let rel = PathBuf::from(&item.rel_path);
    let trash_abs = trash_root.join("sessions").join(batch_id).join(&rel);
    if let Some(parent) = trash_abs.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(original_abs, &trash_abs)?;

    Ok(DeletedEntry {
        candidate: item.clone(),
        original_abs: root.join(&item.rel_path),
        trash_abs,
    })
}

fn write_delete_log(
    trash_root: &Path,
    action: &'static str,
    batch_id: &str,
    entries: &[DeletedEntry],
) -> Result<()> {
    fs::create_dir_all(trash_root)?;
    let log_path = trash_root.join("deletions.jsonl");
    let mut payload = String::new();
    let ts = now_unix_ms();

    if entries.is_empty() {
        let record = DeleteLogRecord {
            action,
            batch_id: batch_id.to_string(),
            kind: String::new(),
            rel_path: String::new(),
            original_abs: String::new(),
            trash_abs: String::new(),
            ts_unix_ms: ts,
        };
        payload.push_str(&serde_json::to_string(&record)?);
        payload.push('\n');
    } else {
        for entry in entries {
            let record = DeleteLogRecord {
                action,
                batch_id: batch_id.to_string(),
                kind: entry.candidate.kind.to_string(),
                rel_path: entry.candidate.rel_path.clone(),
                original_abs: entry.original_abs.display().to_string(),
                trash_abs: entry.trash_abs.display().to_string(),
                ts_unix_ms: ts,
            };
            payload.push_str(&serde_json::to_string(&record)?);
            payload.push('\n');
        }
    }

    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    file.write_all(payload.as_bytes())?;
    Ok(())
}

fn empty_trash(state: &mut DeleteState) -> Result<()> {
    let sessions = state.trash_root.join("sessions");
    let mut removed = 0usize;

    if sessions.exists() {
        for entry in fs::read_dir(&sessions)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                fs::remove_dir_all(&path)?;
                removed += 1;
            } else if path.is_file() {
                fs::remove_file(&path)?;
                removed += 1;
            }
        }
    }

    state.undo_stack.clear();
    state.message = format!("Trash emptied. Removed {removed} session entries.");
    let batch_id = generate_batch_id();
    let _ = write_delete_log(&state.trash_root, "empty_trash", &batch_id, &[]);
    Ok(())
}

fn restore_previous_session(state: &mut DeleteState) -> Result<()> {
    let sessions_root = state.trash_root.join("sessions");
    if !sessions_root.exists() {
        state.message = "No previous trash sessions found.".to_string();
        return Ok(());
    }

    let mut sessions: Vec<(String, PathBuf)> = fs::read_dir(&sessions_root)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_string();
            Some((name, path))
        })
        .collect();

    if sessions.is_empty() {
        state.message = "No previous trash sessions found.".to_string();
        return Ok(());
    }

    sessions.sort_by(|a, b| a.0.cmp(&b.0));
    let (session_id, session_path) = sessions.pop().unwrap_or_default();
    restore_session_path(state, &session_id, &session_path)
}

fn restore_all_sessions(state: &mut DeleteState) -> Result<()> {
    let sessions_root = state.trash_root.join("sessions");
    if !sessions_root.exists() {
        state.message = "No trash sessions found.".to_string();
        return Ok(());
    }

    let mut sessions: Vec<(String, PathBuf)> = fs::read_dir(&sessions_root)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_string();
            Some((name, path))
        })
        .collect();

    if sessions.is_empty() {
        state.message = "No trash sessions found.".to_string();
        return Ok(());
    }

    sessions.sort_by(|a, b| a.0.cmp(&b.0));

    let mut total_restored = 0usize;
    let mut total_failed = 0usize;
    let mut restored_session_count = 0usize;

    for (_session_id, session_path) in sessions {
        let (restored, failed) =
            restore_session_path_counts(state, &session_path, "restore_all_sessions")?;
        if restored > 0 || failed > 0 {
            restored_session_count += 1;
        }
        total_restored += restored;
        total_failed += failed;
    }

    state.message = if total_failed > 0 {
        format!(
            "Restored {} files from {} session(s). Failed: {} (kept in trash).",
            total_restored, restored_session_count, total_failed
        )
    } else {
        format!(
            "Restored {} files from {} session(s). Failed: {}.",
            total_restored, restored_session_count, total_failed
        )
    };

    Ok(())
}

fn restore_specific_file_from_trash(state: &mut DeleteState) -> Result<()> {
    let selected_deleted: HashSet<String> = state
        .selected
        .iter()
        .filter_map(|idx| state.items.get(*idx))
        .filter(|item| item.state == CandidateState::Deleted)
        .map(|item| item.rel_path.clone())
        .collect();
    if !selected_deleted.is_empty() {
        return restore_from_trash_matching(
            state,
            "restore_file",
            "file",
            "selected rows",
            |rel| selected_deleted.contains(rel),
        );
    }

    let query = normalized_rel_query(&state.search_query);
    if query.is_empty() {
        state.message =
            "Select deleted rows (or set search to exact file path), then press i.".to_string();
        return Ok(());
    }

    restore_from_trash_matching(state, "restore_file", "file", &query, |rel| rel == query)
}

fn restore_folder_from_trash(state: &mut DeleteState) -> Result<()> {
    let query = normalized_rel_query(&state.search_query);
    if query.is_empty() {
        state.message =
            "Set search to a folder path, then press o to restore that folder from trash."
                .to_string();
        return Ok(());
    }

    let prefix = format!("{query}/");
    restore_from_trash_matching(state, "restore_folder", "folder", &query, |rel| {
        rel == query || rel.starts_with(&prefix)
    })
}

fn restore_from_trash_matching<F>(
    state: &mut DeleteState,
    log_action: &'static str,
    scope: &'static str,
    query: &str,
    mut matcher: F,
) -> Result<()>
where
    F: FnMut(&str) -> bool,
{
    let root = fs::canonicalize(&state.root).unwrap_or_else(|_| state.root.clone());
    let trashed = latest_trashed_entries(&state.trash_root)?;
    if trashed.is_empty() {
        state.message = "Trash is empty.".to_string();
        return Ok(());
    }

    let mut matches: Vec<(String, PathBuf)> = trashed
        .into_iter()
        .filter(|(rel, _)| matcher(rel.as_str()))
        .collect();

    if matches.is_empty() {
        state.message = format!("No trashed {scope} matched '{query}'.");
        return Ok(());
    }

    matches.sort_by(|a, b| a.0.cmp(&b.0));

    let mut restored = 0usize;
    let mut failed = 0usize;
    let mut restored_entries = Vec::new();

    for (rel_path, trash_abs) in matches {
        let target = root.join(&rel_path);
        if !target.starts_with(&root) {
            failed += 1;
            continue;
        }
        if target.exists() {
            failed += 1;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }

        match fs::rename(&trash_abs, &target) {
            Ok(_) => {
                restored += 1;
                let kind = if has_asset_extension(&target) {
                    "asset"
                } else {
                    "file"
                };
                let candidate = DeleteCandidate {
                    rel_path: rel_path.clone(),
                    kind,
                    state: CandidateState::Active,
                };
                upsert_candidate_state(
                    &mut state.items,
                    &candidate.rel_path,
                    candidate.kind,
                    CandidateState::Active,
                );
                restored_entries.push(DeletedEntry {
                    candidate,
                    original_abs: target,
                    trash_abs,
                });
            }
            Err(_) => failed += 1,
        }
    }

    let _ = prune_empty_trash_sessions(&state.trash_root);
    state
        .items
        .sort_by(|a, b| a.rel_path.cmp(&b.rel_path).then_with(|| a.kind.cmp(b.kind)));
    state.selected.clear();
    clamp_delete_cursor(state);

    if !restored_entries.is_empty() {
        let batch_id = generate_batch_id();
        let _ = write_delete_log(&state.trash_root, log_action, &batch_id, &restored_entries);
    }
    state.message = format!("Restored {restored} {scope} match(es). Failed: {failed}.");

    Ok(())
}

fn latest_trashed_entries(trash_root: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let sessions_root = trash_root.join("sessions");
    if !sessions_root.exists() {
        return Ok(BTreeMap::new());
    }

    let mut rows: Vec<(String, String, PathBuf)> = Vec::new();
    for entry in fs::read_dir(&sessions_root)? {
        let Ok(entry) = entry else {
            continue;
        };
        let session_path = entry.path();
        if !session_path.is_dir() {
            continue;
        }
        let Some(session_id) = session_path
            .file_name()
            .and_then(|v| v.to_str())
            .map(|v| v.to_string())
        else {
            continue;
        };

        for walked in WalkDir::new(&session_path).into_iter().filter_map(|e| e.ok()) {
            let file = walked.path();
            if !file.is_file() {
                continue;
            }
            let Ok(rel) = file.strip_prefix(&session_path) else {
                continue;
            };
            rows.push((
                session_id.clone(),
                rel.to_string_lossy().replace('\\', "/"),
                file.to_path_buf(),
            ));
        }
    }

    rows.sort_by(|a, b| b.0.cmp(&a.0));
    let mut latest = BTreeMap::new();
    for (_session_id, rel, path) in rows {
        latest.entry(rel).or_insert(path);
    }
    Ok(latest)
}

fn upsert_candidate_state(
    items: &mut Vec<DeleteCandidate>,
    rel_path: &str,
    kind: &'static str,
    state: CandidateState,
) {
    if let Some(item) = items
        .iter_mut()
        .find(|item| item.rel_path == rel_path && item.kind == kind)
    {
        item.state = state;
    } else {
        items.push(DeleteCandidate {
            rel_path: rel_path.to_string(),
            kind,
            state,
        });
    }
}

fn normalized_rel_query(input: &str) -> String {
    input
        .trim()
        .replace('\\', "/")
        .trim_start_matches("./")
        .trim_matches('/')
        .to_string()
}

fn prune_empty_trash_sessions(trash_root: &Path) -> Result<usize> {
    let sessions_root = trash_root.join("sessions");
    if !sessions_root.exists() {
        return Ok(0);
    }

    let mut removed = 0usize;
    for entry in fs::read_dir(&sessions_root)? {
        let Ok(entry) = entry else {
            continue;
        };
        let session_path = entry.path();
        if !session_path.is_dir() {
            continue;
        }
        let has_files = WalkDir::new(&session_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .any(|e| e.path().is_file());
        if !has_files {
            let _ = fs::remove_dir_all(&session_path);
            removed += 1;
        }
    }
    Ok(removed)
}

fn restore_session_path(
    state: &mut DeleteState,
    session_id: &str,
    session_path: &Path,
) -> Result<()> {
    let (restored, failed) =
        restore_session_path_counts(state, session_path, "restore_previous_session")?;
    state.message = if failed > 0 {
        format!(
            "Restored {} files from session {}. Failed: {} (kept in trash).",
            restored, session_id, failed
        )
    } else {
        format!(
            "Restored {} files from session {}. Failed: {}.",
            restored, session_id, failed
        )
    };
    Ok(())
}

fn restore_session_path_counts(
    state: &mut DeleteState,
    session_path: &Path,
    log_action: &'static str,
) -> Result<(usize, usize)> {
    let root = fs::canonicalize(&state.root).unwrap_or_else(|_| state.root.clone());
    let mut restored = 0usize;
    let mut failed = 0usize;
    let mut restored_entries = Vec::new();

    for entry in WalkDir::new(&session_path)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let trash_file = entry.path();
        if !trash_file.is_file() {
            continue;
        }

        let Ok(rel) = trash_file.strip_prefix(&session_path) else {
            failed += 1;
            continue;
        };

        let target = root.join(rel);
        if !target.starts_with(&root) {
            failed += 1;
            continue;
        }
        if target.exists() {
            failed += 1;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }

        match fs::rename(trash_file, &target) {
            Ok(_) => {
                restored += 1;
                let rel_display = rel.to_string_lossy().replace('\\', "/");
                let kind = if has_asset_extension(&target) {
                    "asset"
                } else {
                    "file"
                };
                let candidate = DeleteCandidate {
                    rel_path: rel_display,
                    kind,
                    state: CandidateState::Active,
                };
                upsert_candidate_state(
                    &mut state.items,
                    &candidate.rel_path,
                    candidate.kind,
                    CandidateState::Active,
                );
                restored_entries.push(DeletedEntry {
                    candidate,
                    original_abs: target.clone(),
                    trash_abs: trash_file.to_path_buf(),
                });
            }
            Err(_) => failed += 1,
        }
    }

    state
        .items
        .sort_by(|a, b| a.rel_path.cmp(&b.rel_path).then_with(|| a.kind.cmp(b.kind)));
    state.selected.clear();
    clamp_delete_cursor(state);

    // Do not delete the whole session blindly: failed files must remain recoverable.
    let _ = prune_empty_trash_sessions(&state.trash_root);
    let batch_id = generate_batch_id();
    if !restored_entries.is_empty() {
        let _ = write_delete_log(&state.trash_root, log_action, &batch_id, &restored_entries);
    }

    Ok((restored, failed))
}

fn generate_batch_id() -> String {
    format!("batch-{}", now_unix_ms())
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn hydrate_deleted_candidates_from_trash(state: &mut DeleteState) -> Result<()> {
    let root = fs::canonicalize(&state.root).unwrap_or_else(|_| state.root.clone());

    for (rel_path, trash_abs) in latest_trashed_entries(&state.trash_root)? {
        let active_abs = root.join(&rel_path);
        let state_kind = if has_asset_extension(Path::new(&rel_path))
            || has_asset_extension(&trash_abs)
            || has_asset_extension(&active_abs)
        {
            "asset"
        } else {
            "file"
        };

        upsert_candidate_state(
            &mut state.items,
            &rel_path,
            state_kind,
            if active_abs.exists() {
                CandidateState::Active
            } else {
                CandidateState::Deleted
            },
        );
    }

    state
        .items
        .sort_by(|a, b| a.rel_path.cmp(&b.rel_path).then_with(|| a.kind.cmp(b.kind)));
    clamp_delete_cursor(state);
    Ok(())
}

fn draw_page(frame: &mut Frame, report: &Report, state: &TuiState) {
    match state.page {
        TuiPage::Summary => draw_summary_page(frame, report),
        TuiPage::Delete => draw_delete_page(frame, report, state),
    }
}

fn draw_summary_page(frame: &mut Frame, report: &Report) {
    let root_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(8),
            Constraint::Min(8),
            Constraint::Min(8),
        ])
        .split(frame.area());

    let title = Paragraph::new(format!(
        "haadi summary | {} | d delete page | q quit",
        report.root
    ))
    .block(Block::default().borders(Borders::ALL).title("Report"));
    frame.render_widget(title, root_chunks[0]);

    let summary = Paragraph::new(vec![
        Line::from(format!(
            "total source files: {}",
            report.summary.total_source_files
        )),
        Line::from(format!(
            "total asset files: {}",
            report.summary.total_asset_files
        )),
        Line::from(format!(
            "reachable source files: {}",
            report.summary.total_reachable_files
        )),
        Line::from(format!("entry files: {}", report.summary.total_entries)),
        Line::from(format!(
            "unused files: {}",
            report.summary.unused_files_count
        )),
        Line::from(format!("used assets: {}", report.summary.used_assets_count)),
        Line::from(format!(
            "unused assets: {}",
            report.summary.unused_assets_count
        )),
        Line::from(format!(
            "asset coverage: {:.1}%",
            report.summary.asset_usage_coverage_pct
        )),
        Line::from(format!(
            "unused dependencies: {}",
            report.summary.unused_dependencies_count
        )),
        Line::from(format!(
            "unused exports: {}",
            report.summary.unused_exports_count
        )),
        Line::from(format!(
            "unresolved local imports: {}",
            report.summary.unresolved_local_imports
        )),
        Line::from(format!(
            "high-confidence graph: {}",
            report.summary.high_confidence_graph
        )),
        Line::from(format!(
            "omitted risky findings: {}",
            report.summary.omitted_risky_findings
        )),
    ])
    .block(Block::default().borders(Borders::ALL).title("Summary"))
    .wrap(Wrap { trim: true });
    frame.render_widget(summary, root_chunks[1]);

    let middle = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
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

    frame.render_widget(
        List::new(top_items(&report.used_assets, 8)).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Used assets (top)"),
        ),
        middle[2],
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

fn draw_delete_page(frame: &mut Frame, _report: &Report, state: &TuiState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(8),
            Constraint::Length(4),
        ])
        .split(frame.area());

    let header = Paragraph::new(vec![
        Line::from("Delete page: select unused files/assets only"),
        Line::from("Controls: j/k move | space toggle | a all | c clear | f filter | / search | g reset search+filter | x delete | u undo | i restore file (search) | o restore folder (search) | r restore prev | R restore all | z empty trash | y approve | b back | q quit"),
        Line::from("Deleted files are shown in red and remain searchable for restore."),
    ])
    .block(Block::default().borders(Borders::ALL).title("Delete mode"))
    .wrap(Wrap { trim: true });
    frame.render_widget(header, chunks[0]);

    let filtered = filtered_indices(&state.delete);
    let mut rows = Vec::new();
    if filtered.is_empty() {
        rows.push(ListItem::new("No delete candidates."));
    } else {
        let list_height = chunks[1].height.saturating_sub(2) as usize;
        let window = list_height.max(1);
        let start = state.delete.cursor.saturating_sub(window.saturating_sub(1));
        let end = (start + window).min(filtered.len());

        for (visual_idx, item_idx) in filtered[start..end].iter().enumerate() {
            let item = &state.delete.items[*item_idx];
            let cursor_idx = start + visual_idx;
            let marker = if cursor_idx == state.delete.cursor {
                ">"
            } else {
                " "
            };
            let selected = if state.delete.selected.contains(item_idx) {
                "[x]"
            } else {
                "[ ]"
            };
            let text = format!(
                "{marker} {selected} ({}) {}",
                if item.state == CandidateState::Deleted {
                    "deleted"
                } else {
                    item.kind
                },
                item.rel_path
            );
            let mut row = ListItem::new(text);
            if item.state == CandidateState::Deleted {
                row = row.style(Style::default().fg(Color::Red));
            }
            rows.push(row);
        }
    }

    frame.render_widget(
        List::new(rows).block(Block::default().borders(Borders::ALL).title(format!(
            "Candidates {} | filter={} | search='{}'",
            filtered.len(),
            state.delete.filter.label(),
            if state.delete.search_query.is_empty() {
                "(none)"
            } else {
                state.delete.search_query.as_str()
            }
        ))),
        chunks[1],
    );

    let mut footer_lines = vec![Line::from(state.delete.message.as_str())];
    if state.delete.confirm_delete {
        footer_lines.push(Line::from(
            "Approve delete: press y to confirm, n/Esc to cancel.",
        ));
    } else if state.delete.confirm_empty_trash {
        footer_lines.push(Line::from(
            "Approve empty trash: press y to confirm, n/Esc to cancel.",
        ));
    } else if state.delete.confirm_restore_previous {
        footer_lines.push(Line::from(
            "Approve restore previous session: press y to confirm, n/Esc to cancel.",
        ));
    } else if state.delete.confirm_restore_all {
        footer_lines.push(Line::from(
            "Approve restore ALL sessions: press y to confirm, n/Esc to cancel.",
        ));
    } else if state.delete.editing_search {
        footer_lines.push(Line::from(format!(
            "Search input: {}",
            state.delete.search_input
        )));
    } else {
        footer_lines.push(Line::from(format!(
            "Selected: {}",
            state.delete.selected.len()
        )));
    }

    let footer = Paragraph::new(footer_lines)
        .block(Block::default().borders(Borders::ALL).title("Status"))
        .wrap(Wrap { trim: true });
    frame.render_widget(footer, chunks[2]);
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

fn build_delete_candidates(report: &Report) -> Vec<DeleteCandidate> {
    let mut items = Vec::new();

    for path in &report.unused_files {
        items.push(DeleteCandidate {
            rel_path: path.clone(),
            kind: "file",
            state: CandidateState::Active,
        });
    }

    for path in &report.unused_assets {
        items.push(DeleteCandidate {
            rel_path: path.clone(),
            kind: "asset",
            state: CandidateState::Active,
        });
    }

    items.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    items
}

fn filtered_indices(state: &DeleteState) -> Vec<usize> {
    let query = state.search_query.trim();
    let matcher = build_search_matcher(query);
    state
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            // Deleted rows stay hidden unless user is actively searching.
            if item.state == CandidateState::Deleted && query.is_empty() {
                return false;
            }

            let kind_ok = match state.filter {
                DeleteFilter::All => true,
                DeleteFilter::Files => item.kind == "file",
                DeleteFilter::Assets => item.kind == "asset",
            };
            if !kind_ok {
                return false;
            }
            if query.is_empty() {
                return true;
            }
            matcher.matches(&item.rel_path)
        })
        .map(|(idx, _)| idx)
        .collect()
}

fn compile_case_insensitive_regex(pattern: &str) -> Option<regex::Regex> {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .ok()
}

fn glob_like_to_regex(glob: &str) -> String {
    let mut out = String::from("^");
    for ch in glob.chars() {
        match ch {
            '*' => out.push_str(".*"),
            '?' => out.push('.'),
            _ => out.push_str(&regex::escape(&ch.to_string())),
        }
    }
    out.push('$');
    out
}

fn looks_like_regex(query: &str) -> bool {
    query
        .chars()
        .any(|c| matches!(c, '[' | ']' | '(' | ')' | '|' | '+' | '^' | '$' | '{' | '}' | '\\' | '.'))
}

enum SearchMatcher {
    Any,
    Substring(String),
    Regex(regex::Regex),
}

impl SearchMatcher {
    fn matches(&self, path: &str) -> bool {
        match self {
            SearchMatcher::Any => true,
            SearchMatcher::Substring(q) => path.to_ascii_lowercase().contains(q),
            SearchMatcher::Regex(re) => re.is_match(path),
        }
    }
}

fn build_search_matcher(query: &str) -> SearchMatcher {
    let q = query.trim();
    if q.is_empty() {
        return SearchMatcher::Any;
    }

    if let Some(pattern) = q.strip_prefix("re:") {
        if let Some(re) = compile_case_insensitive_regex(pattern) {
            return SearchMatcher::Regex(re);
        }
        return SearchMatcher::Substring(q.to_ascii_lowercase());
    }

    if q.len() >= 2 && q.starts_with('/') && q.ends_with('/') {
        let pattern = &q[1..q.len() - 1];
        if let Some(re) = compile_case_insensitive_regex(pattern) {
            return SearchMatcher::Regex(re);
        }
        return SearchMatcher::Substring(q.to_ascii_lowercase());
    }

    if q.contains('*') || q.contains('?') {
        if let Some(re) = compile_case_insensitive_regex(&glob_like_to_regex(q)) {
            return SearchMatcher::Regex(re);
        }
        return SearchMatcher::Substring(q.to_ascii_lowercase());
    }

    if looks_like_regex(q) {
        if let Some(re) = compile_case_insensitive_regex(q) {
            return SearchMatcher::Regex(re);
        }
    }

    SearchMatcher::Substring(q.to_ascii_lowercase())
}

fn clamp_delete_cursor(state: &mut DeleteState) {
    let len = filtered_indices(state).len();
    if len == 0 {
        state.cursor = 0;
        return;
    }
    if state.cursor >= len {
        state.cursor = len - 1;
    }
}
