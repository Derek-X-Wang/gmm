//! Slice 12: hash-conflict detection.
//!
//! For each enabled Mod we parse every `.ini` under the Mod's
//! effective directory (Library + active Variant, resolved upstream)
//! and extract `hash = …` values out of `[TextureOverride*]` /
//! `[ResourceOverride*]` sections. Two Mods that bind the same 3dmigoto
//! resource hash define a Conflict (`CONTEXT.md` § Conflict). v1 surfaces
//! conflicts as warnings; priority-order resolution is deferred to v1.1.
//!
//! 3dmigoto INI syntax we honour here is intentionally minimal:
//!
//! * `[Section Name]` headers, treated case-insensitively for the
//!   `texture-override` / `resource-override` prefixes.
//! * `key = value` rows. Keys are matched case-insensitively. Anything
//!   after a leading `;` is a comment.
//! * `if 0` / `if false` blocks are skipped — those are the canonical
//!   "this is disabled" sentinels and the slice's AC calls them out
//!   specifically. Other `if`/`endif` conditions can't be evaluated
//!   statically; we treat their bodies as live (conservative for the
//!   conflict surface, which lives in the warnings layer).

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::error::{Error, Result};

/// One binding produced by the parser: a hash literal seen inside a
/// `[TextureOverride*]` or `[ResourceOverride*]` section, with the
/// section name preserved so the UI can render context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashBinding {
    pub hash: String,
    pub section: String,
}

/// Read `path` and return every hash binding the parser found. Returns
/// an empty `Vec` if the file is not an INI we recognise.
pub fn extract_hashes_from_file(path: &Path) -> Result<Vec<HashBinding>> {
    let contents = fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(extract_hashes_from_str(&contents))
}

/// Recursively scan `root` for `.ini` files and concatenate their
/// hash bindings. Symlinks and junctions are followed (the Library
/// owns the bytes and junctions just project them into the game dir).
pub fn extract_hashes_from_dir(root: &Path) -> Result<Vec<HashBinding>> {
    let mut out = Vec::new();
    visit(root, &mut out)?;
    Ok(out)
}

fn visit(dir: &Path, out: &mut Vec<HashBinding>) -> Result<()> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            visit(&path, out)?;
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("ini"))
        {
            let bindings = extract_hashes_from_file(&path)?;
            out.extend(bindings);
        }
    }
    Ok(())
}

/// The pure parser, exposed for unit tests.
pub fn extract_hashes_from_str(contents: &str) -> Vec<HashBinding> {
    let mut out = Vec::new();
    let mut current_section: Option<String> = None;
    // Stack of `if`-block "skip" flags. When the top of the stack is
    // `true`, we skip key/value rows. Pushed on `if`, popped on
    // `endif`.
    let mut if_skip_stack: Vec<bool> = Vec::new();

    for raw in contents.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }

        // Section header.
        if let Some(stripped) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            current_section = Some(stripped.trim().to_string());
            // Section change does not reset the if-stack — sections can
            // be opened inside an if-block — but in practice 3dmigoto
            // ini structure does. We mirror that behaviour for
            // simplicity, resetting at every header.
            if_skip_stack.clear();
            continue;
        }

        // if / endif tracking.
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("endif") {
            if_skip_stack.pop();
            continue;
        }
        if let Some(rest) = lower.strip_prefix("if ") {
            // Treat as "skip" only when the condition is one of the
            // canonical literal-false sentinels. Anything else stays
            // live (conservative).
            let cond = rest.trim();
            let skip = matches!(cond, "0" | "false");
            if_skip_stack.push(skip);
            continue;
        }
        if lower == "else" {
            if let Some(top) = if_skip_stack.last_mut() {
                *top = !*top;
            }
            continue;
        }

        if if_skip_stack.iter().any(|&skip| skip) {
            continue;
        }

        // Key/value row.
        let (key, value) = match line.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        if !key.trim().eq_ignore_ascii_case("hash") {
            continue;
        }

        let Some(section) = current_section.as_ref() else {
            continue;
        };
        if !is_override_section(section) {
            continue;
        }

        let hash = value.trim().to_ascii_lowercase();
        let hash = hash.trim_start_matches("0x").to_string();
        if hash.is_empty() {
            continue;
        }
        out.push(HashBinding {
            hash,
            section: section.clone(),
        });
    }
    out
}

fn strip_comment(line: &str) -> &str {
    match line.find(';') {
        Some(i) => &line[..i],
        None => line,
    }
}

fn is_override_section(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("textureoverride") || lower.starts_with("resourceoverride")
}

/// Aggregated conflict report. Empty when no hash is bound by two or
/// more enabled Mods.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConflictReport {
    pub conflicts: Vec<Conflict>,
    /// `mod_id -> conflict_count` so the UI doesn't have to count.
    pub per_mod_count: HashMap<String, usize>,
}

/// One Conflict: a hash bound by two or more Mods. `sections` is the
/// union of section names each Mod used to bind this hash; useful UI
/// context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Conflict {
    pub hash: String,
    pub mod_ids: Vec<String>,
    pub sections: Vec<String>,
}

/// Build a report from a list of `(mod_id, bindings)` tuples. The
/// pure function. Core hands it the per-mod bindings it collected
/// from disk.
pub fn build_report(per_mod_bindings: &[(String, Vec<HashBinding>)]) -> ConflictReport {
    let mut by_hash: HashMap<String, (Vec<String>, Vec<String>)> = HashMap::new();
    for (mod_id, bindings) in per_mod_bindings {
        // Dedup bindings within a single mod — re-binding the same
        // hash twice in two of your own INIs is not a Conflict.
        let mut seen_in_mod: HashMap<String, ()> = HashMap::new();
        for b in bindings {
            if seen_in_mod.insert(b.hash.clone(), ()).is_some() {
                continue;
            }
            let entry = by_hash
                .entry(b.hash.clone())
                .or_insert_with(|| (Vec::new(), Vec::new()));
            entry.0.push(mod_id.clone());
            if !entry.1.iter().any(|s| s == &b.section) {
                entry.1.push(b.section.clone());
            }
        }
    }

    let mut conflicts: Vec<Conflict> = by_hash
        .into_iter()
        .filter(|(_, (mods, _))| mods.len() >= 2)
        .map(|(hash, (mod_ids, sections))| Conflict {
            hash,
            mod_ids,
            sections,
        })
        .collect();
    conflicts.sort_by(|a, b| a.hash.cmp(&b.hash));

    let mut per_mod_count: HashMap<String, usize> = HashMap::new();
    for c in &conflicts {
        for m in &c.mod_ids {
            *per_mod_count.entry(m.clone()).or_insert(0) += 1;
        }
    }
    ConflictReport {
        conflicts,
        per_mod_count,
    }
}
