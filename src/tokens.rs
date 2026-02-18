use super::*;
pub(crate) fn build_file_token_cache(
    files: &HashSet<PathBuf>,
) -> Result<HashMap<PathBuf, HashSet<String>>> {
    let mut cache = HashMap::new();

    for file in files {
        let source = fs::read_to_string(file).unwrap_or_default();
        let mut tokens = HashSet::new();
        for m in IDENT_TOKEN_RE.find_iter(&source) {
            tokens.insert(m.as_str().to_string());
        }
        cache.insert(file.clone(), tokens);
    }

    Ok(cache)
}

pub(crate) fn count_tokens_in_scope(
    scope: &HashSet<PathBuf>,
    token_cache: &HashMap<PathBuf, HashSet<String>>,
) -> HashMap<String, usize> {
    let mut counts = HashMap::new();

    for file in scope {
        let Some(tokens) = token_cache.get(file) else {
            continue;
        };

        for token in tokens {
            *counts.entry(token.clone()).or_insert(0) += 1;
        }
    }

    counts
}

pub(crate) fn export_appears_in_other_reachable_files(
    token_file_counts: &HashMap<String, usize>,
    export_name: &str,
    reachable: &HashSet<PathBuf>,
    file: &Path,
) -> bool {
    if export_name.is_empty() {
        return false;
    }

    let Some(count) = token_file_counts.get(export_name) else {
        return false;
    };

    if *count == 0 {
        return false;
    }

    // Same file always contributes at least one token; more than one file is a conservative
    // indicator that the symbol may be used externally.
    if *count > 1 {
        return true;
    }

    // Degenerate case fallback for tiny projects where token counting might skip files.
    reachable.len() == 1 && reachable.contains(file) && *count > 0
}

pub(crate) fn export_appears_in_other_project_files(
    token_file_counts: &HashMap<String, usize>,
    export_name: &str,
    all_files: &HashSet<PathBuf>,
    file: &Path,
) -> bool {
    if export_name.is_empty() {
        return false;
    }
    let Some(count) = token_file_counts.get(export_name) else {
        return false;
    };
    if *count == 0 {
        return false;
    }
    if *count > 1 {
        return true;
    }
    all_files.len() == 1 && all_files.contains(file) && *count > 0
}
