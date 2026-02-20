use super::*;
use walkdir::WalkDir;
pub(crate) fn collect_source_files(root: &Path) -> Result<HashSet<PathBuf>> {
    let mut files = HashSet::new();

    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !is_ignored_dir(e.path()))
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.is_file() && has_source_extension(path) {
            files.insert(fs::canonicalize(path)?);
        }
    }

    Ok(files)
}

pub(crate) fn collect_asset_files(root: &Path) -> Result<HashSet<PathBuf>> {
    let mut files = HashSet::new();

    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !is_ignored_dir(e.path()))
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.is_file() && has_asset_extension(path) {
            files.insert(fs::canonicalize(path)?);
        }
    }

    Ok(files)
}

pub(crate) fn collect_used_assets(
    root: &Path,
    source_files: &HashSet<PathBuf>,
    assets: &HashSet<PathBuf>,
) -> Result<HashSet<PathBuf>> {
    let mut used = HashSet::new();
    let mut string_literals = HashSet::new();
    let indexed_assets: Vec<(PathBuf, String)> = assets
        .iter()
        .map(|asset| (asset.clone(), relative_display(root, asset).replace('\\', "/")))
        .collect();

    // Single-pass source scan: collect string literals, direct asset imports, and import.meta.glob usage.
    for source_file in source_files {
        let source = fs::read_to_string(source_file).unwrap_or_default();

        collect_literals_and_direct_asset_usages(
            root,
            source_file,
            assets,
            &source,
            &mut string_literals,
            &mut used,
        )?;
        collect_asset_glob_usages(root, source_file, &source, &indexed_assets, &mut used)?;
    }

    for asset in assets {
        if is_public_asset(asset) {
            used.insert(asset.clone());
            continue;
        }

        let refs = asset_reference_candidates(root, asset);
        if refs.is_empty() {
            continue;
        }

        if refs.iter().any(|r| string_literals.contains(r)) {
            used.insert(asset.clone());
        }
    }

    Ok(used)
}

fn collect_asset_glob_usages(
    root: &Path,
    source_file: &Path,
    source: &str,
    indexed_assets: &[(PathBuf, String)],
    out_used: &mut HashSet<PathBuf>,
) -> Result<()> {
    for caps in IMPORT_META_GLOB_RE.captures_iter(source) {
        let raw = [1usize, 2, 3]
            .into_iter()
            .find_map(|idx| caps.get(idx).map(|m| m.as_str()))
            .unwrap_or_default();
        if raw.is_empty() {
            continue;
        }

        let spec = normalize_specifier(raw);
        if spec.is_empty() {
            continue;
        }

        let Some(rel_pattern) = resolve_glob_specifier_to_rel_pattern(root, source_file, &spec)
        else {
            continue;
        };

        let Some(glob_re) = regex::Regex::new(&glob_path_pattern_to_regex(&rel_pattern)).ok()
        else {
            continue;
        };

        for (asset_abs, asset_rel) in indexed_assets {
            if glob_re.is_match(asset_rel) {
                out_used.insert(asset_abs.clone());
            }
        }
    }

    Ok(())
}

fn resolve_glob_specifier_to_rel_pattern(
    root: &Path,
    from_file: &Path,
    specifier: &str,
) -> Option<String> {
    if is_relative_specifier(specifier) {
        let parent = from_file.parent()?;
        let joined = parent.join(specifier);
        return to_rel_pattern(root, &joined);
    }

    if let Some(trimmed) = specifier.strip_prefix('/') {
        return Some(trimmed.replace('\\', "/"));
    }

    if let Some(trimmed) = specifier.strip_prefix("@/") {
        return Some(format!("src/{}", trimmed.replace('\\', "/")));
    }

    if let Some(trimmed) = specifier.strip_prefix("~/") {
        return Some(format!("src/{}", trimmed.replace('\\', "/")));
    }

    if specifier.starts_with("src/") {
        return Some(specifier.replace('\\', "/"));
    }

    None
}

fn to_rel_pattern(root: &Path, path: &Path) -> Option<String> {
    let root_norm = normalize_path_components(root);
    let path_norm = normalize_path_components(path);
    if path_norm.len() < root_norm.len() || path_norm[..root_norm.len()] != root_norm[..] {
        return None;
    }
    let rel = path_norm[root_norm.len()..].join("/");
    if rel.is_empty() { None } else { Some(rel) }
}

fn normalize_path_components(path: &Path) -> Vec<String> {
    let mut out = Vec::<String>::new();
    for comp in path.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                let _ = out.pop();
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {}
            std::path::Component::Normal(v) => out.push(v.to_string_lossy().to_string()),
        }
    }
    out
}

fn glob_path_pattern_to_regex(glob: &str) -> String {
    let mut out = String::from("^");
    let mut chars = glob.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '*' => {
                if matches!(chars.peek(), Some('*')) {
                    let _ = chars.next();
                    out.push_str(".*");
                } else {
                    out.push_str("[^/]*");
                }
            }
            '?' => out.push_str("[^/]"),
            _ => out.push_str(&regex::escape(&ch.to_string())),
        }
    }

    out.push('$');
    out
}

fn collect_literals_and_direct_asset_usages(
    root: &Path,
    source_file: &Path,
    assets: &HashSet<PathBuf>,
    source: &str,
    out_literals: &mut HashSet<String>,
    out_used: &mut HashSet<PathBuf>,
) -> Result<()> {
    for caps in STRING_LITERAL_RE.captures_iter(source) {
        for idx in [1usize, 2, 3] {
            let Some(m) = caps.get(idx) else {
                continue;
            };
            let raw = m.as_str();
            if raw.is_empty() {
                continue;
            }

            out_literals.insert(raw.to_string());
            let spec = normalize_specifier(raw);
            if spec.is_empty() {
                continue;
            }
            out_literals.insert(spec.clone());

            if let Some(resolved) = resolve_asset_specifier(root, source_file, &spec, assets)? {
                out_used.insert(resolved);
            }
        }
    }

    Ok(())
}

fn resolve_asset_specifier(
    root: &Path,
    from_file: &Path,
    specifier: &str,
    assets: &HashSet<PathBuf>,
) -> Result<Option<PathBuf>> {
    if is_relative_specifier(specifier) {
        let Some(parent) = from_file.parent() else {
            return Ok(None);
        };
        return resolve_asset_candidate(&parent.join(specifier), assets);
    }

    if let Some(trimmed) = specifier.strip_prefix('/') {
        return resolve_asset_candidate(&root.join(trimmed), assets);
    }

    if let Some(trimmed) = specifier.strip_prefix("@/") {
        return resolve_asset_candidate(&root.join("src").join(trimmed), assets);
    }

    if let Some(trimmed) = specifier.strip_prefix("~/") {
        return resolve_asset_candidate(&root.join("src").join(trimmed), assets);
    }

    if specifier.starts_with("src/") {
        return resolve_asset_candidate(&root.join(specifier), assets);
    }

    Ok(None)
}

fn resolve_asset_candidate(
    raw_candidate: &Path,
    assets: &HashSet<PathBuf>,
) -> Result<Option<PathBuf>> {
    let mut candidates = Vec::new();

    if raw_candidate.extension().is_some() {
        candidates.push(raw_candidate.to_path_buf());
    } else {
        candidates.push(raw_candidate.to_path_buf());
        for ext in ASSET_EXTENSIONS {
            candidates.push(raw_candidate.with_extension(ext));
        }
        for ext in ASSET_EXTENSIONS {
            candidates.push(raw_candidate.join(format!("index.{ext}")));
        }
    }

    for candidate in candidates {
        let normalized = normalize_path(candidate);
        if assets.contains(&normalized) {
            return Ok(Some(normalized));
        }
    }

    Ok(None)
}

fn normalize_path(path: PathBuf) -> PathBuf {
    use std::path::Component;

    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = out.pop();
            }
            Component::RootDir => out.push(Path::new("/")),
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::Normal(v) => out.push(v),
        }
    }

    out
}

fn asset_reference_candidates(root: &Path, asset: &Path) -> Vec<String> {
    let mut refs = HashSet::new();
    let rel = relative_display(root, asset);
    let rel_norm = rel.replace('\\', "/");
    refs.insert(rel_norm.clone());
    refs.insert(format!("/{rel_norm}"));

    if let Some(stripped) = rel_norm.strip_prefix("src/") {
        refs.insert(stripped.to_string());
        refs.insert(format!("/{stripped}"));
        refs.insert(format!("@/{stripped}"));
        refs.insert(format!("~/{stripped}"));
    }

    if let Some(stripped) = rel_norm.strip_prefix("public/") {
        refs.insert(stripped.to_string());
        refs.insert(format!("/{stripped}"));
    }

    if let Some(file_name) = asset.file_name().and_then(|s| s.to_str()) {
        refs.insert(file_name.to_string());
    }

    let base_refs: Vec<String> = refs.iter().cloned().collect();
    let query_suffixes = ["?react", "?url", "?raw", "?inline", "?component"];
    for base in base_refs {
        for suffix in query_suffixes {
            refs.insert(format!("{base}{suffix}"));
        }
    }

    refs.into_iter().collect()
}
