use anyhow::{Context, Result};
use clap::Parser;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

mod entries;
mod output;
mod parser;
mod scanner;
mod tokens;

use entries::discover_entries;
use output::{print_human_report, print_tui_report, relative_display};
use parser::{parse_module, strip_comments};
use scanner::{collect_asset_files, collect_source_files, collect_used_assets};
use tokens::{
    build_file_token_cache, count_tokens_in_scope, export_appears_in_other_project_files,
    export_appears_in_other_reachable_files,
};

const JS_TS_EXTENSIONS: &[&str] = &["js", "jsx", "ts", "tsx", "mjs", "cjs"];
const ASSET_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "avif", "svg", "ico", "bmp", "tiff", "mp4", "webm", "mp3",
    "wav", "ogg", "woff", "woff2", "ttf", "otf", "eot", "pdf", "txt", "css", "scss", "sass",
    "less",
];
const LOCAL_EXISTING_EXTENSIONS: &[&str] = &[
    "js", "jsx", "ts", "tsx", "mjs", "cjs", "json", "css", "scss", "sass", "less", "png", "jpg",
    "jpeg", "gif", "webp", "avif", "svg", "ico", "bmp", "tiff", "mp4", "webm", "mp3", "wav", "ogg",
    "woff", "woff2", "ttf", "otf", "eot", "pdf", "txt",
];
const NEXT_APP_ROUTE_FILES: &[&str] = &[
    "page",
    "layout",
    "route",
    "loading",
    "error",
    "not-found",
    "template",
    "default",
    "head",
];

static IMPORT_FROM_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*import\s+([^;\n]+?)\s+from\s+['\"]([^'\"]+)['\"]"#).unwrap()
});
static IMPORT_SIDE_EFFECT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*import\s+['\"]([^'\"]+)['\"]"#).unwrap());
static EXPORT_DECL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*export\s+(?:const|let|var|function|class|interface|type|enum)\s+([A-Za-z_$][\w$]*)"#)
        .unwrap()
});
static EXPORT_LIST_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)^\s*export\s*\{\s*([^}]+)\s*\}(?:\s*from\s*['\"]([^'\"]+)['\"])?"#).unwrap()
});
static EXPORT_DEFAULT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*export\s+default\b"#).unwrap());
static EXPORT_ALL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)^\s*export\s+\*\s+from\s+['\"]([^'\"]+)['\"]"#).unwrap());
static REQUIRE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?m)(?:^|\s|=)require\(\s*['\"]([^'\"]+)['\"]\s*\)"#).unwrap());
static DESTRUCTURE_REQUIRE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?m)\{\s*([^}]+)\s*\}\s*=\s*require\(\s*['\"]([^'\"]+)['\"]\s*\)"#).unwrap()
});
static DYN_IMPORT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"import\(\s*['\"]([^'\"]+)['\"]\s*\)"#).unwrap());
static TRAILING_COMMA_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#",\s*([}\]])"#).unwrap());
static IDENT_TOKEN_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"[A-Za-z_$][A-Za-z0-9_$]*"#).unwrap());
static STRING_LITERAL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?s)(?:'([^'\\]*(?:\\.[^'\\]*)*)'|"([^"\\]*(?:\\.[^"\\]*)*)"|`([^`\\]*(?:\\.[^`\\]*)*)`)"#,
    )
    .unwrap()
});

#[derive(Parser, Debug)]
#[command(name = "haadi")]
#[command(about = "Find high-confidence unused files, dependencies, and exports in JS/TS projects")]
struct Cli {
    /// Project root
    #[arg(long, default_value = ".")]
    root: PathBuf,

    /// Entry files (can be used multiple times)
    #[arg(long = "entry")]
    entries: Vec<String>,

    /// Include dev/peer/optional dependencies in unused dependency checks
    #[arg(long)]
    include_non_prod_deps: bool,

    /// Emit low-confidence findings too (may increase false positives)
    #[arg(long)]
    include_low_confidence: bool,

    /// Limit asset analysis to these roots (repeatable or comma-separated), e.g. --asset-roots src/assets,public
    #[arg(long = "asset-roots", value_delimiter = ',')]
    asset_roots: Vec<String>,

    /// Emit JSON output
    #[arg(long)]
    json: bool,

    /// Render an interactive terminal dashboard (press q to quit)
    #[arg(long)]
    tui: bool,
}

#[derive(Debug, Default)]
struct ImportRecord {
    specifier: String,
    uses_default: bool,
    uses_namespace: bool,
    names: HashSet<String>,
    side_effect_only: bool,
    is_reexport: bool,
}

#[derive(Debug, Default)]
struct ModuleInfo {
    imports: Vec<ImportRecord>,
    exports: HashSet<String>,
    has_default_export: bool,
    has_export_all: bool,
}

#[derive(Debug, Default, Clone)]
struct ExportUsage {
    all: bool,
    default_used: bool,
    names: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DepKind {
    Prod,
    Dev,
    Peer,
    Optional,
}

#[derive(Debug, Serialize)]
struct UnusedExport {
    file: String,
    export: String,
}

#[derive(Debug, Serialize)]
struct Report {
    root: String,
    summary: ReportSummary,
    entries: Vec<String>,
    warnings: Vec<String>,
    unused_files: Vec<String>,
    used_assets: Vec<String>,
    unused_assets: Vec<String>,
    unused_dependencies: Vec<String>,
    unused_exports: Vec<UnusedExport>,
}

#[derive(Debug, Serialize)]
struct ReportSummary {
    total_source_files: usize,
    total_asset_files: usize,
    total_reachable_files: usize,
    total_entries: usize,
    unresolved_local_imports: usize,
    high_confidence_graph: bool,
    omitted_risky_findings: bool,
    unused_files_count: usize,
    used_assets_count: usize,
    unused_assets_count: usize,
    asset_usage_coverage_pct: f64,
    unused_dependencies_count: usize,
    unused_exports_count: usize,
}

#[derive(Debug, Default)]
struct Resolver {
    files: HashSet<PathBuf>,
    root: PathBuf,
    base_dirs: Vec<PathBuf>,
    alias_rules: Vec<AliasRule>,
}

#[derive(Debug, Clone)]
struct AliasRule {
    key: String,
    target: String,
    base_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct UnresolvedImport {
    from_file: PathBuf,
    specifier: String,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let root = fs::canonicalize(&cli.root)
        .with_context(|| format!("Failed to access root: {}", cli.root.display()))?;

    let files = collect_source_files(&root)?;
    let all_assets = collect_asset_files(&root)?;
    let assets = filter_assets_by_roots(&root, &all_assets, &cli.asset_roots);
    let resolver = build_resolver(&root, &files)?;

    let mut warnings =
        vec!["Analysis is conservative by default to minimize false positives.".to_string()];
    if !cli.asset_roots.is_empty() && assets.is_empty() {
        warnings.push(
            "No assets matched --asset-roots filter; asset findings may be empty.".to_string(),
        );
    }

    let mut modules: HashMap<PathBuf, ModuleInfo> = HashMap::new();
    for file in &files {
        modules.insert(file.clone(), parse_module(file)?);
    }

    let entries = discover_entries(&root, &files, &cli.entries)?;
    if entries.is_empty() {
        warnings.push(
            "No entry files discovered. Pass --entry to improve unused file accuracy.".to_string(),
        );
    }

    let reachable = reachable_files(&entries, &modules, &resolver)?;

    let unresolved = collect_unresolved_local_imports(&reachable, &modules, &resolver)?;
    let maybe_used_from_unresolved =
        infer_potentially_used_files_from_unresolved(&files, &unresolved, &root);
    let high_confidence_graph = unresolved.is_empty();
    if !unresolved.is_empty() {
        warnings.push(format!(
            "Skipped high-risk findings because {} local/alias imports could not be resolved.",
            unresolved.len()
        ));
        if !maybe_used_from_unresolved.is_empty() {
            warnings.push(format!(
                "Suppressed unused-export findings for {} files potentially referenced by unresolved imports.",
                maybe_used_from_unresolved.len()
            ));
        }
    }

    let used_packages = collect_used_packages(&reachable, &modules, &resolver)?;
    let declared_deps = collect_declared_dependencies(&root)?;
    let mut unused_dependencies: Vec<String> = declared_deps
        .iter()
        .filter(|(name, kind)| {
            if name.starts_with("@types/") {
                return false;
            }

            if !cli.include_non_prod_deps {
                return **kind == DepKind::Prod;
            }

            true
        })
        .map(|(name, _)| name)
        .filter(|name| !used_packages.contains(*name))
        .cloned()
        .collect();
    unused_dependencies.sort();

    let mut unused_files = Vec::new();
    let mut used_assets = Vec::new();
    let mut unused_assets = Vec::new();
    let mut unused_exports = Vec::new();

    if high_confidence_graph || cli.include_low_confidence {
        unused_files = files
            .difference(&reachable)
            .filter(|path| {
                !is_test_like_file(path)
                    && !is_declaration_file(path)
                    && !is_common_config_file(path)
            })
            .map(|path| relative_display(&root, path))
            .collect();
        unused_files.sort();
        let used_asset_paths = collect_used_assets(&root, &files, &assets)?;
        used_assets = used_asset_paths
            .iter()
            .map(|path| relative_display(&root, path))
            .collect();
        used_assets.sort();
        unused_assets = assets
            .difference(&used_asset_paths)
            .filter(|path| !is_public_asset(path))
            .map(|path| relative_display(&root, path))
            .collect();
        unused_assets.sort();

        let entry_set: HashSet<PathBuf> = entries.iter().cloned().collect();
        let mut usage: HashMap<PathBuf, ExportUsage> = HashMap::new();
        let token_cache = build_file_token_cache(&files)?;
        let token_file_counts = count_tokens_in_scope(&reachable, &token_cache);
        let global_token_file_counts = count_tokens_in_scope(&files, &token_cache);
        let mut suppressed_by_symbol_ref = 0usize;

        // High-confidence: usage only comes from reachable files.
        for file in &reachable {
            let Some(module) = modules.get(file) else {
                continue;
            };

            for import in &module.imports {
                if import.side_effect_only || import.is_reexport {
                    continue;
                }

                if let Some(resolved) = resolver.resolve_specifier(file, &import.specifier)? {
                    let slot = usage.entry(resolved).or_default();
                    if import.uses_namespace {
                        slot.all = true;
                    }
                    if import.uses_default {
                        slot.default_used = true;
                    }
                    slot.names.extend(import.names.iter().cloned());
                }
            }
        }

        // Conservative re-export handling: any reachable re-export marks source module as used.
        for file in &reachable {
            let Some(module) = modules.get(file) else {
                continue;
            };

            for import in &module.imports {
                if !import.is_reexport {
                    continue;
                }

                if let Some(resolved) = resolver.resolve_specifier(file, &import.specifier)? {
                    let slot = usage.entry(resolved).or_default();
                    slot.all = true;
                }
            }
        }

        for (file, module) in &modules {
            if !reachable.contains(file) {
                continue;
            }
            if maybe_used_from_unresolved.contains(file) {
                continue;
            }
            if entry_set.contains(file) || is_test_like_file(file) || is_declaration_file(file) {
                continue;
            }

            let used = usage.get(file).cloned().unwrap_or_default();

            if !used.all {
                for export_name in &module.exports {
                    if export_appears_in_other_reachable_files(
                        &token_file_counts,
                        export_name,
                        &reachable,
                        file,
                    ) {
                        suppressed_by_symbol_ref += 1;
                        continue;
                    }
                    if export_appears_in_other_project_files(
                        &global_token_file_counts,
                        export_name,
                        &files,
                        file,
                    ) {
                        suppressed_by_symbol_ref += 1;
                        continue;
                    }

                    if !used.names.contains(export_name) {
                        unused_exports.push(UnusedExport {
                            file: relative_display(&root, file),
                            export: export_name.clone(),
                        });
                    }
                }

                if module.has_default_export && !used.default_used {
                    unused_exports.push(UnusedExport {
                        file: relative_display(&root, file),
                        export: "default".to_string(),
                    });
                }
            }

            if module.has_export_all && !used.all {
                warnings.push(format!(
                    "{} re-exports '*' and may need manual verification.",
                    relative_display(&root, file)
                ));
            }
        }

        unused_exports.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.export.cmp(&b.export)));
        unused_exports.dedup_by(|a, b| a.file == b.file && a.export == b.export);
        if suppressed_by_symbol_ref > 0 {
            warnings.push(format!(
                "Suppressed {} unused-export findings because the symbol appears in other reachable files.",
                suppressed_by_symbol_ref
            ));
        }
    } else {
        warnings.push(
            "unused_files and unused_exports omitted (use --include-low-confidence to force)."
                .to_string(),
        );
        warnings.push(
            "unused_assets omitted because graph confidence is low (use --include-low-confidence to force)."
                .to_string(),
        );
    }
    let total_asset_files = assets.len();
    let unused_assets_count = unused_assets.len();
    let used_assets_count = total_asset_files.saturating_sub(unused_assets_count);

    let summary = ReportSummary {
        total_source_files: files.len(),
        total_asset_files,
        total_reachable_files: reachable.len(),
        total_entries: entries.len(),
        unresolved_local_imports: unresolved.len(),
        high_confidence_graph,
        omitted_risky_findings: !(high_confidence_graph || cli.include_low_confidence),
        unused_files_count: unused_files.len(),
        used_assets_count,
        unused_assets_count,
        asset_usage_coverage_pct: if total_asset_files == 0 {
            0.0
        } else {
            (used_assets_count as f64 * 100.0) / total_asset_files as f64
        },
        unused_dependencies_count: unused_dependencies.len(),
        unused_exports_count: unused_exports.len(),
    };

    let report = Report {
        root: root.display().to_string(),
        summary,
        entries: entries
            .iter()
            .map(|entry| relative_display(&root, entry))
            .collect(),
        warnings,
        unused_files,
        used_assets,
        unused_assets,
        unused_dependencies,
        unused_exports,
    };

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if cli.tui {
        print_tui_report(&report)?;
    } else {
        print_human_report(&report);
    }

    Ok(())
}

fn build_resolver(root: &Path, files: &HashSet<PathBuf>) -> Result<Resolver> {
    let mut resolver = Resolver {
        files: files.clone(),
        root: root.to_path_buf(),
        base_dirs: vec![root.to_path_buf(), root.join("src")],
        alias_rules: Vec::new(),
    };

    let mut config_paths = BTreeSet::new();
    for seed_name in [
        "tsconfig.json",
        "jsconfig.json",
        "tsconfig.app.json",
        "tsconfig.base.json",
    ] {
        let seed = root.join(seed_name);
        if seed.exists() {
            discover_related_tsconfigs(&seed, &mut config_paths, &mut HashSet::new())?;
        }
    }

    for config_path in config_paths {
        apply_compiler_options_from_config(&config_path, &mut resolver, root)?;
    }

    resolver.base_dirs = dedup_paths(resolver.base_dirs);

    Ok(resolver)
}

fn discover_related_tsconfigs(
    config_path: &Path,
    out: &mut BTreeSet<PathBuf>,
    visiting: &mut HashSet<PathBuf>,
) -> Result<()> {
    let canonical = fs::canonicalize(config_path).unwrap_or_else(|_| config_path.to_path_buf());
    if !canonical.exists() || !visiting.insert(canonical.clone()) {
        return Ok(());
    }

    out.insert(canonical.clone());

    let raw = fs::read_to_string(&canonical).unwrap_or_default();
    let sanitized = sanitize_jsonc(&raw);
    let value: serde_json::Value = match serde_json::from_str(&sanitized) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    let config_dir = canonical.parent().unwrap_or(Path::new("."));

    if let Some(extends) = value.get("extends").and_then(|v| v.as_str()) {
        if let Some(path) = resolve_tsconfig_reference_path(config_dir, extends) {
            discover_related_tsconfigs(&path, out, visiting)?;
        }
    }

    if let Some(refs) = value.get("references").and_then(|v| v.as_array()) {
        for ref_item in refs {
            let Some(path_str) = ref_item.get("path").and_then(|v| v.as_str()) else {
                continue;
            };
            if let Some(path) = resolve_tsconfig_reference_path(config_dir, path_str) {
                discover_related_tsconfigs(&path, out, visiting)?;
            }
        }
    }

    Ok(())
}

fn resolve_tsconfig_reference_path(base_dir: &Path, raw_ref: &str) -> Option<PathBuf> {
    if raw_ref.trim().is_empty() {
        return None;
    }

    let mut candidate = if Path::new(raw_ref).is_absolute() {
        PathBuf::from(raw_ref)
    } else {
        base_dir.join(raw_ref)
    };

    if candidate.is_dir() {
        candidate = candidate.join("tsconfig.json");
    }

    if candidate.exists() {
        return Some(candidate);
    }

    if candidate.extension().is_none() {
        let with_json = candidate.with_extension("json");
        if with_json.exists() {
            return Some(with_json);
        }
    }

    None
}

fn apply_compiler_options_from_config(
    config_path: &Path,
    resolver: &mut Resolver,
    root: &Path,
) -> Result<()> {
    let raw = fs::read_to_string(config_path).unwrap_or_default();
    let sanitized = sanitize_jsonc(&raw);
    let value: serde_json::Value = match serde_json::from_str(&sanitized) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    let config_dir = config_path.parent().unwrap_or(root);
    let compiler = value
        .get("compilerOptions")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    if let Some(base_url) = compiler.get("baseUrl").and_then(|v| v.as_str()) {
        resolver.base_dirs.push(config_dir.join(base_url));
    }

    if let Some(paths) = compiler.get("paths").and_then(|v| v.as_object()) {
        for (key, targets) in paths {
            let Some(arr) = targets.as_array() else {
                continue;
            };

            for target in arr.iter().filter_map(|v| v.as_str()) {
                resolver.alias_rules.push(AliasRule {
                    key: key.to_string(),
                    target: target.to_string(),
                    base_dir: config_dir.to_path_buf(),
                });
            }
        }
    }

    Ok(())
}

fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for path in paths {
        let canonical = fs::canonicalize(&path).unwrap_or(path);
        if seen.insert(canonical.clone()) {
            out.push(canonical);
        }
    }

    out
}

fn sanitize_jsonc(input: &str) -> String {
    let without_comments = strip_comments(input);
    let mut current = without_comments;

    loop {
        let next = TRAILING_COMMA_RE.replace_all(&current, "$1").into_owned();
        if next == current {
            return next;
        }
        current = next;
    }
}

impl Resolver {
    fn resolve_specifier(&self, from_file: &Path, specifier: &str) -> Result<Option<PathBuf>> {
        let normalized = normalize_specifier(specifier);
        if normalized.is_empty() {
            return Ok(None);
        }

        if is_relative_specifier(&normalized) {
            let Some(parent) = from_file.parent() else {
                return Ok(None);
            };
            return resolve_candidate_path(&parent.join(&normalized), &self.files);
        }

        if let Some(trimmed) = normalized.strip_prefix('/') {
            return resolve_candidate_path(&self.root.join(trimmed), &self.files);
        }

        for rule in &self.alias_rules {
            if let Some(star) = match_alias(&rule.key, &normalized) {
                let target = apply_alias_target(&rule.target, &star);
                if let Some(path) =
                    resolve_candidate_path(&rule.base_dir.join(target), &self.files)?
                {
                    return Ok(Some(path));
                }
            }
        }

        // Absolute-style imports through baseUrl (e.g., import x from "utils/foo").
        if !looks_like_package_specifier(&normalized) {
            for base in &self.base_dirs {
                if let Some(path) = resolve_candidate_path(&base.join(&normalized), &self.files)? {
                    return Ok(Some(path));
                }
            }
        }

        Ok(None)
    }

    fn is_likely_local_specifier(&self, specifier: &str) -> bool {
        let normalized = normalize_specifier(specifier);
        if normalized.is_empty() {
            return false;
        }

        if is_relative_specifier(&normalized) || normalized.starts_with('/') {
            return true;
        }

        if self
            .alias_rules
            .iter()
            .any(|rule| match_alias(&rule.key, &normalized).is_some())
        {
            return true;
        }

        if !looks_like_package_specifier(&normalized) {
            return true;
        }

        false
    }

    fn local_specifier_exists(&self, from_file: &Path, specifier: &str) -> Result<bool> {
        let normalized = normalize_specifier(specifier);
        if normalized.is_empty() {
            return Ok(false);
        }

        if is_relative_specifier(&normalized) {
            let Some(parent) = from_file.parent() else {
                return Ok(false);
            };
            return local_target_exists(&parent.join(&normalized));
        }

        if let Some(trimmed) = normalized.strip_prefix('/') {
            return local_target_exists(&self.root.join(trimmed));
        }

        for rule in &self.alias_rules {
            if let Some(star) = match_alias(&rule.key, &normalized) {
                let target = apply_alias_target(&rule.target, &star);
                if local_target_exists(&rule.base_dir.join(target))? {
                    return Ok(true);
                }
            }
        }

        if !looks_like_package_specifier(&normalized) {
            for base in &self.base_dirs {
                if local_target_exists(&base.join(&normalized))? {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }
}

fn collect_used_packages(
    reachable: &HashSet<PathBuf>,
    modules: &HashMap<PathBuf, ModuleInfo>,
    resolver: &Resolver,
) -> Result<HashSet<String>> {
    let mut used = HashSet::new();

    for file in reachable {
        let Some(module) = modules.get(file) else {
            continue;
        };

        for import in &module.imports {
            let normalized = normalize_specifier(&import.specifier);
            if resolver.resolve_specifier(file, &normalized)?.is_none()
                && looks_like_package_specifier(&normalized)
            {
                used.insert(package_name(&normalized));
            }
        }
    }

    Ok(used)
}

fn collect_declared_dependencies(root: &Path) -> Result<HashMap<String, DepKind>> {
    let package_json = root.join("package.json");
    if !package_json.exists() {
        return Ok(HashMap::new());
    }

    let raw = fs::read_to_string(package_json)?;
    let value: serde_json::Value = serde_json::from_str(&raw)?;

    let mut deps = HashMap::new();
    insert_dep_kind(&mut deps, &value, "dependencies", DepKind::Prod);
    insert_dep_kind(&mut deps, &value, "devDependencies", DepKind::Dev);
    insert_dep_kind(&mut deps, &value, "peerDependencies", DepKind::Peer);
    insert_dep_kind(&mut deps, &value, "optionalDependencies", DepKind::Optional);

    Ok(deps)
}

fn insert_dep_kind(
    out: &mut HashMap<String, DepKind>,
    root: &serde_json::Value,
    key: &str,
    kind: DepKind,
) {
    if let Some(obj) = root.get(key).and_then(|v| v.as_object()) {
        for name in obj.keys() {
            out.entry(name.clone()).or_insert(kind);
        }
    }
}

fn reachable_files(
    entries: &[PathBuf],
    modules: &HashMap<PathBuf, ModuleInfo>,
    resolver: &Resolver,
) -> Result<HashSet<PathBuf>> {
    let mut seen = HashSet::new();
    let mut queue: VecDeque<PathBuf> = entries.iter().cloned().collect();

    while let Some(current) = queue.pop_front() {
        if !seen.insert(current.clone()) {
            continue;
        }

        if let Some(module) = modules.get(&current) {
            for import in &module.imports {
                if let Some(next) = resolver.resolve_specifier(&current, &import.specifier)? {
                    if !seen.contains(&next) {
                        queue.push_back(next);
                    }
                }
            }
        }
    }

    Ok(seen)
}

fn collect_unresolved_local_imports(
    reachable: &HashSet<PathBuf>,
    modules: &HashMap<PathBuf, ModuleInfo>,
    resolver: &Resolver,
) -> Result<Vec<UnresolvedImport>> {
    let mut unresolved = BTreeSet::new();

    for file in reachable {
        let Some(module) = modules.get(file) else {
            continue;
        };

        for import in &module.imports {
            if !resolver.is_likely_local_specifier(&import.specifier) {
                continue;
            }

            if resolver
                .resolve_specifier(file, &import.specifier)?
                .is_none()
                && !resolver.local_specifier_exists(file, &import.specifier)?
            {
                unresolved.insert(UnresolvedImport {
                    from_file: file.clone(),
                    specifier: import.specifier.clone(),
                });
            }
        }
    }

    Ok(unresolved.into_iter().collect())
}

fn infer_potentially_used_files_from_unresolved(
    files: &HashSet<PathBuf>,
    unresolved: &[UnresolvedImport],
    root: &Path,
) -> HashSet<PathBuf> {
    let mut maybe_used = HashSet::new();

    let file_indexes: Vec<(PathBuf, String, String)> = files
        .iter()
        .map(|file| {
            let rel = relative_display(root, file).replace('\\', "/");
            let rel_no_ext = strip_file_extension(&rel);
            (file.clone(), rel, rel_no_ext)
        })
        .collect();

    for item in unresolved {
        let suffixes = unresolved_specifier_suffixes(&item.specifier);
        let leaf = unresolved_leaf_name(&item.specifier);

        for (file, rel, rel_no_ext) in &file_indexes {
            if suffixes.iter().any(|suffix| {
                rel_no_ext == suffix
                    || rel_no_ext.ends_with(&format!("/{suffix}"))
                    || rel.ends_with(&format!("/{suffix}"))
                    || rel_no_ext.ends_with(&format!("/{suffix}/index"))
            }) {
                maybe_used.insert(file.clone());
                continue;
            }

            if let Some(leaf_name) = &leaf {
                if file.file_stem().and_then(|v| v.to_str()) == Some(leaf_name.as_str()) {
                    maybe_used.insert(file.clone());
                }
            }
        }
    }

    maybe_used
}

fn unresolved_specifier_suffixes(specifier: &str) -> Vec<String> {
    let clean = specifier
        .split('?')
        .next()
        .unwrap_or(specifier)
        .split('#')
        .next()
        .unwrap_or(specifier)
        .replace('\\', "/");

    let mut out = BTreeSet::new();
    let mut base = clean.trim().to_string();

    while base.starts_with("./") || base.starts_with("../") {
        if base.starts_with("./") {
            base = base[2..].to_string();
        } else {
            base = base[3..].to_string();
        }
    }
    if let Some(stripped) = base.strip_prefix('/') {
        out.insert(stripped.to_string());
    }
    out.insert(base.clone());

    if let Some(stripped) = base.strip_prefix("@/") {
        out.insert(stripped.to_string());
    }
    if let Some(stripped) = base.strip_prefix("~/") {
        out.insert(stripped.to_string());
    }
    if base.starts_with('@') {
        if let Some((_, rest)) = base.split_once('/') {
            out.insert(rest.to_string());
        }
    }
    if let Some(stripped) = base.strip_prefix("src/") {
        out.insert(stripped.to_string());
    }

    out.into_iter().filter(|v| !v.is_empty()).collect()
}

fn unresolved_leaf_name(specifier: &str) -> Option<String> {
    let clean = specifier
        .split('?')
        .next()?
        .split('#')
        .next()?
        .replace('\\', "/");
    let leaf = clean.split('/').filter(|v| !v.is_empty()).next_back()?;
    if leaf == "." || leaf == ".." {
        return None;
    }
    Some(strip_file_extension(leaf))
}

fn strip_file_extension(path_like: &str) -> String {
    let file_name = path_like.rsplit('/').next().unwrap_or(path_like);
    if let Some(dot_index) = file_name.rfind('.') {
        let prefix = &path_like[..path_like.len() - (file_name.len() - dot_index)];
        return prefix.to_string();
    }
    path_like.to_string()
}

fn resolve_candidate_path(
    raw_candidate: &Path,
    files: &HashSet<PathBuf>,
) -> Result<Option<PathBuf>> {
    let mut candidates = Vec::new();

    if raw_candidate.extension().is_some() {
        candidates.push(raw_candidate.to_path_buf());
    } else {
        candidates.push(raw_candidate.to_path_buf());
        for ext in JS_TS_EXTENSIONS {
            candidates.push(raw_candidate.with_extension(ext));
        }
        for ext in JS_TS_EXTENSIONS {
            candidates.push(raw_candidate.join(format!("index.{ext}")));
        }
    }

    for candidate in candidates {
        if candidate.exists() {
            let canonical = fs::canonicalize(candidate)?;
            if files.contains(&canonical) {
                return Ok(Some(canonical));
            }
        }
    }

    Ok(None)
}

fn local_target_exists(raw_candidate: &Path) -> Result<bool> {
    let mut candidates = Vec::new();

    if raw_candidate.extension().is_some() {
        candidates.push(raw_candidate.to_path_buf());
    } else {
        candidates.push(raw_candidate.to_path_buf());
        for ext in LOCAL_EXISTING_EXTENSIONS {
            candidates.push(raw_candidate.with_extension(ext));
        }
        for ext in LOCAL_EXISTING_EXTENSIONS {
            candidates.push(raw_candidate.join(format!("index.{ext}")));
        }
    }

    Ok(candidates.into_iter().any(|path| path.exists()))
}

fn normalize_specifier(specifier: &str) -> String {
    let mut out = specifier.trim().to_string();
    if out.is_empty() {
        return out;
    }

    if let Some((left, _)) = out.split_once('?') {
        out = left.to_string();
    }
    if let Some((left, _)) = out.split_once('#') {
        out = left.to_string();
    }

    out.trim().to_string()
}

fn match_alias(alias_key: &str, specifier: &str) -> Option<String> {
    if let Some((prefix, suffix)) = alias_key.split_once('*') {
        if !specifier.starts_with(prefix) || !specifier.ends_with(suffix) {
            return None;
        }
        let mid_start = prefix.len();
        let mid_end = specifier.len().saturating_sub(suffix.len());
        return Some(specifier[mid_start..mid_end].to_string());
    }

    if alias_key == specifier {
        Some(String::new())
    } else {
        None
    }
}

fn apply_alias_target(target: &str, wildcard: &str) -> String {
    if target.contains('*') {
        target.replace('*', wildcard)
    } else {
        target.to_string()
    }
}

fn has_source_extension(path: &Path) -> bool {
    if is_declaration_file(path) {
        return false;
    }

    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| JS_TS_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
}

fn has_asset_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ASSET_EXTENSIONS.contains(&ext))
        .unwrap_or(false)
}

fn is_public_asset(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .map(|v| v == "public")
            .unwrap_or(false)
    })
}

fn is_declaration_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|name| name.ends_with(".d.ts"))
        .unwrap_or(false)
}

fn is_test_like_file(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let path_str = path.to_string_lossy();

    file_name.contains(".test.")
        || file_name.contains(".spec.")
        || path_str.contains("/__tests__/")
        || path_str.contains("\\__tests__\\")
}

fn is_common_config_file(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let lower = file_name.to_ascii_lowercase();

    if lower.contains("config") {
        return true;
    }

    if file_name.starts_with(".eslintrc")
        || file_name.starts_with(".prettierrc")
        || file_name.starts_with(".stylelintrc")
        || file_name.starts_with(".babelrc")
    {
        return true;
    }

    // Generic tooling config pattern, e.g. i18next-parser.config.js
    let config_exts = [".js", ".cjs", ".mjs", ".ts", ".cts", ".mts", ".json"];
    if file_name.contains(".config.") && config_exts.iter().any(|ext| file_name.ends_with(ext)) {
        return true;
    }

    let known = [
        "eslint.config.js",
        "eslint.config.cjs",
        "eslint.config.mjs",
        "eslint.config.ts",
        "eslint.config.mts",
        "eslint.config.cts",
        "vite.config.js",
        "vite.config.cjs",
        "vite.config.mjs",
        "vite.config.ts",
        "vite.config.mts",
        "vite.config.cts",
        "vitest.config.js",
        "vitest.config.cjs",
        "vitest.config.mjs",
        "vitest.config.ts",
        "vitest.config.mts",
        "vitest.config.cts",
        "jest.config.js",
        "jest.config.cjs",
        "jest.config.mjs",
        "jest.config.ts",
        "jest.config.mts",
        "jest.config.cts",
        "webpack.config.js",
        "webpack.config.cjs",
        "webpack.config.mjs",
        "webpack.config.ts",
        "rollup.config.js",
        "rollup.config.cjs",
        "rollup.config.mjs",
        "rollup.config.ts",
        "postcss.config.js",
        "postcss.config.cjs",
        "tailwind.config.js",
        "tailwind.config.cjs",
        "tailwind.config.ts",
        "babel.config.js",
        "babel.config.cjs",
        "commitlint.config.js",
        "commitlint.config.cjs",
        "lint-staged.config.js",
        "lint-staged.config.cjs",
    ];

    known.contains(&file_name)
}

fn is_ignored_dir(path: &Path) -> bool {
    let ignored = [
        "node_modules",
        ".git",
        ".haadi_trash",
        "dist",
        "build",
        "coverage",
        "target",
        ".next",
        "out",
    ];

    path.file_name()
        .and_then(|n| n.to_str())
        .map(|name| ignored.contains(&name))
        .unwrap_or(false)
}

fn filter_assets_by_roots(
    root: &Path,
    assets: &HashSet<PathBuf>,
    asset_roots: &[String],
) -> HashSet<PathBuf> {
    if asset_roots.is_empty() {
        return assets.clone();
    }

    let roots: Vec<String> = asset_roots
        .iter()
        .map(|v| normalize_asset_root(v))
        .filter(|v| !v.is_empty())
        .collect();

    if roots.is_empty() {
        return assets.clone();
    }

    assets
        .iter()
        .filter(|asset| {
            let rel = relative_display(root, asset).replace('\\', "/");
            roots
                .iter()
                .any(|prefix| rel == *prefix || rel.starts_with(&format!("{prefix}/")))
        })
        .cloned()
        .collect()
}

fn normalize_asset_root(value: &str) -> String {
    value
        .trim()
        .replace('\\', "/")
        .trim_start_matches("./")
        .trim_matches('/')
        .to_string()
}

fn is_relative_specifier(specifier: &str) -> bool {
    specifier.starts_with("./") || specifier.starts_with("../")
}

fn looks_like_package_specifier(specifier: &str) -> bool {
    if is_relative_specifier(specifier) || specifier.starts_with('/') {
        return false;
    }

    if specifier.starts_with("#") {
        return false;
    }

    // Treat dotted paths and tsconfig-style root aliases as potentially local.
    if specifier.contains('.') {
        return false;
    }

    true
}

fn package_name(specifier: &str) -> String {
    let mut parts = specifier.split('/');
    let first = parts.next().unwrap_or_default();

    if first.starts_with('@') {
        let second = parts.next().unwrap_or_default();
        format!("{first}/{second}")
    } else {
        first.to_string()
    }
}
