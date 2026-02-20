use super::*;
pub(crate) fn parse_module(file: &Path) -> Result<ModuleInfo> {
    let source = fs::read_to_string(file)
        .with_context(|| format!("Failed to read source file: {}", file.display()))?;
    let source = strip_comments(&source);

    let mut info = ModuleInfo::default();

    for caps in IMPORT_FROM_RE.captures_iter(&source) {
        let clause = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        let specifier = caps.get(2).map(|m| m.as_str()).unwrap_or_default();

        let mut record = ImportRecord {
            specifier: specifier.to_string(),
            ..Default::default()
        };
        parse_import_clause(clause, &mut record);
        info.imports.push(record);
    }

    for caps in IMPORT_SIDE_EFFECT_RE.captures_iter(&source) {
        let specifier = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        info.imports.push(ImportRecord {
            specifier: specifier.to_string(),
            side_effect_only: true,
            ..Default::default()
        });
    }

    for caps in REQUIRE_RE.captures_iter(&source) {
        let specifier = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        info.imports.push(ImportRecord {
            specifier: specifier.to_string(),
            uses_namespace: true,
            ..Default::default()
        });
    }

    for caps in DESTRUCTURE_REQUIRE_RE.captures_iter(&source) {
        let names = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        let specifier = caps.get(2).map(|m| m.as_str()).unwrap_or_default();
        let mut record = ImportRecord {
            specifier: specifier.to_string(),
            ..Default::default()
        };
        for name in parse_destructured_names(names) {
            record.names.insert(name);
        }
        info.imports.push(record);
    }

    for caps in DYN_IMPORT_RE.captures_iter(&source) {
        let specifier = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        info.imports.push(ImportRecord {
            specifier: specifier.to_string(),
            uses_namespace: true,
            ..Default::default()
        });
    }

    for caps in EXPORT_DECL_RE.captures_iter(&source) {
        let name = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        if !name.is_empty() {
            info.exports.insert(name.to_string());
        }
    }

    for caps in EXPORT_LIST_RE.captures_iter(&source) {
        let names = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        let src = caps.get(2).map(|m| m.as_str());

        if let Some(specifier) = src {
            let mut record = ImportRecord {
                specifier: specifier.to_string(),
                is_reexport: true,
                ..Default::default()
            };
            parse_export_list_as_import(names, &mut record);
            info.imports.push(record);
        } else {
            for name in parse_export_names(names) {
                info.exports.insert(name);
            }
        }
    }

    if EXPORT_DEFAULT_RE.is_match(&source) {
        info.has_default_export = true;
    }

    for caps in EXPORT_ALL_RE.captures_iter(&source) {
        info.has_export_all = true;
        let specifier = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
        info.imports.push(ImportRecord {
            specifier: specifier.to_string(),
            uses_namespace: true,
            is_reexport: true,
            ..Default::default()
        });
    }

    Ok(info)
}

pub(crate) fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let chars: Vec<char> = source.chars().collect();
    let mut i = 0;
    let mut in_string: Option<char> = None;

    while i < chars.len() {
        let c = chars[i];

        if let Some(quote) = in_string {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                i += 1;
                out.push(chars[i]);
            } else if c == quote {
                in_string = None;
            }
            i += 1;
            continue;
        }

        if c == '\'' || c == '"' || c == '`' {
            in_string = Some(c);
            out.push(c);
            i += 1;
            continue;
        }

        if c == '/' && i + 1 < chars.len() {
            if chars[i + 1] == '/' {
                i += 2;
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                if i < chars.len() {
                    out.push('\n');
                    i += 1;
                }
                continue;
            }

            if chars[i + 1] == '*' {
                i += 2;
                while i + 1 < chars.len() {
                    if chars[i] == '*' && chars[i + 1] == '/' {
                        i += 2;
                        break;
                    }
                    if chars[i] == '\n' {
                        out.push('\n');
                    }
                    i += 1;
                }
                continue;
            }
        }

        out.push(c);
        i += 1;
    }

    out
}

fn parse_import_clause(clause: &str, record: &mut ImportRecord) {
    let cleaned = clause.trim();
    let cleaned = cleaned.strip_prefix("type ").unwrap_or(cleaned).trim();

    if cleaned.contains("* as") {
        record.uses_namespace = true;
    }

    if cleaned.starts_with('{') {
        record.names.extend(parse_export_names(cleaned));
        return;
    }

    if let Some((first, rest)) = cleaned.split_once(',') {
        if !first.trim().is_empty() {
            record.uses_default = true;
        }
        if rest.contains('*') {
            record.uses_namespace = true;
        }
        if rest.contains('{') {
            record.names.extend(parse_export_names(rest));
        }
        return;
    }

    if cleaned.contains('{') {
        record.names.extend(parse_export_names(cleaned));
    } else if !cleaned.is_empty() {
        record.uses_default = true;
    }
}

fn parse_export_list_as_import(names: &str, record: &mut ImportRecord) {
    for raw in names.split(',') {
        let part = raw.trim();
        if part.is_empty() {
            continue;
        }

        if part == "default" {
            record.uses_default = true;
            continue;
        }

        if part.starts_with('*') {
            record.uses_namespace = true;
            continue;
        }

        let import_name = part
            .split_once(" as ")
            .map(|(left, _)| left.trim())
            .unwrap_or(part)
            .trim_start_matches("type ")
            .trim();

        if !import_name.is_empty() {
            record.names.insert(import_name.to_string());
        }
    }
}

fn parse_export_names(names: &str) -> HashSet<String> {
    let mut out = HashSet::new();

    let trimmed = names.trim().trim_start_matches('{').trim_end_matches('}');
    for raw in trimmed.split(',') {
        let part = raw.trim();
        if part.is_empty() {
            continue;
        }

        if part == "default" {
            out.insert("default".to_string());
            continue;
        }

        let exported = part
            .split_once(" as ")
            .map(|(_, right)| right.trim())
            .unwrap_or(part)
            .trim_start_matches("type ")
            .trim();

        if !exported.is_empty() {
            out.insert(exported.to_string());
        }
    }

    out
}

fn parse_destructured_names(names: &str) -> HashSet<String> {
    let mut out = HashSet::new();

    for raw in names.split(',') {
        let item = raw.trim();
        if item.is_empty() {
            continue;
        }

        let left = item
            .split_once(':')
            .map(|(l, _)| l.trim())
            .unwrap_or(item)
            .trim();

        if !left.is_empty() {
            out.insert(left.to_string());
        }
    }

    out
}
