use super::*;
pub(crate) fn discover_entries(
    root: &Path,
    files: &HashSet<PathBuf>,
    cli_entries: &[String],
) -> Result<Vec<PathBuf>> {
    let mut entries: BTreeSet<PathBuf> = BTreeSet::new();

    for entry in cli_entries {
        if let Some(path) = resolve_candidate_path(&root.join(entry), files)? {
            entries.insert(path);
        }
    }

    if !entries.is_empty() {
        return Ok(entries.into_iter().collect());
    }

    for entry in package_json_entry_candidates(root)? {
        if let Some(path) = resolve_candidate_path(&root.join(&entry), files)? {
            entries.insert(path);
        }
    }

    for candidate in [
        "src/index.ts",
        "src/index.tsx",
        "src/index.js",
        "src/index.jsx",
        "src/main.ts",
        "src/main.tsx",
        "src/main.js",
        "src/main.jsx",
        "index.ts",
        "index.js",
    ] {
        if let Some(path) = resolve_candidate_path(&root.join(candidate), files)? {
            entries.insert(path);
        }
    }

    for file in files {
        if is_framework_convention_entry(root, file) || is_test_like_file(file) {
            entries.insert(file.clone());
        }
    }

    Ok(entries.into_iter().collect())
}

fn is_framework_convention_entry(root: &Path, file: &Path) -> bool {
    let Ok(rel) = file.strip_prefix(root) else {
        return false;
    };

    let rel_str = rel.to_string_lossy();
    let rel_norm = rel_str.replace('\\', "/");

    if rel_norm.starts_with("pages/") || rel_norm.starts_with("src/pages/") {
        return true;
    }

    if rel_norm.starts_with("app/") || rel_norm.starts_with("src/app/") {
        let stem = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        return NEXT_APP_ROUTE_FILES.contains(&stem);
    }

    false
}

fn package_json_entry_candidates(root: &Path) -> Result<Vec<String>> {
    let package_json = root.join("package.json");
    if !package_json.exists() {
        return Ok(Vec::new());
    }

    let raw = fs::read_to_string(package_json)?;
    let value: serde_json::Value = serde_json::from_str(&raw)?;
    let mut out = Vec::new();

    for key in ["main", "module", "types", "browser"] {
        if let Some(v) = value.get(key).and_then(|v| v.as_str()) {
            out.push(v.to_string());
        }
    }

    if let Some(bin) = value.get("bin") {
        match bin {
            serde_json::Value::String(s) => out.push(s.to_string()),
            serde_json::Value::Object(map) => {
                for v in map.values().filter_map(|v| v.as_str()) {
                    out.push(v.to_string());
                }
            }
            _ => {}
        }
    }

    if let Some(exports) = value.get("exports") {
        collect_strings(exports, &mut out);
    }

    Ok(out)
}

fn collect_strings(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(s) => out.push(s.to_string()),
        serde_json::Value::Object(map) => {
            for v in map.values() {
                collect_strings(v, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                collect_strings(v, out);
            }
        }
        _ => {}
    }
}
