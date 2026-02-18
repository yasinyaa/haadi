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
    reachable: &HashSet<PathBuf>,
    assets: &HashSet<PathBuf>,
) -> Result<HashSet<PathBuf>> {
    let mut used = HashSet::new();
    let string_literals = collect_string_literals(reachable)?;

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

fn collect_string_literals(files: &HashSet<PathBuf>) -> Result<HashSet<String>> {
    let mut out = HashSet::new();

    for file in files {
        let source = fs::read_to_string(file).unwrap_or_default();
        for caps in STRING_LITERAL_RE.captures_iter(&source) {
            if let Some(single) = caps.get(1) {
                let s = single.as_str();
                if !s.is_empty() {
                    out.insert(s.to_string());
                }
            }
            if let Some(double) = caps.get(2) {
                let s = double.as_str();
                if !s.is_empty() {
                    out.insert(s.to_string());
                }
            }
        }
    }

    Ok(out)
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
    }

    if let Some(stripped) = rel_norm.strip_prefix("public/") {
        refs.insert(stripped.to_string());
        refs.insert(format!("/{stripped}"));
    }

    if let Some(file_name) = asset.file_name().and_then(|s| s.to_str()) {
        refs.insert(file_name.to_string());
    }

    refs.into_iter().collect()
}
