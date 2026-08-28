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
use super::filesystem::resolve_enumerated_entry;

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
    visit_with_entry_lookups(
        dir,
        out,
        &mut |entry| entry.file_type(),
        &mut |_| Ok(()),
        &mut |_| Ok(()),
    )
}

#[cfg(test)]
fn visit_with_file_type<F>(
    dir: &Path,
    out: &mut Vec<HashBinding>,
    classify_file_type: &mut F,
) -> Result<()>
where
    F: FnMut(&fs::DirEntry) -> std::io::Result<fs::FileType>,
{
    visit_with_entry_lookups(dir, out, classify_file_type, &mut |_| Ok(()), &mut |_| {
        Ok(())
    })
}

fn visit_with_entry_lookups<F, D, R>(
    dir: &Path,
    out: &mut Vec<HashBinding>,
    classify_file_type: &mut F,
    before_descend: &mut D,
    before_read_ini: &mut R,
) -> Result<()>
where
    F: FnMut(&fs::DirEntry) -> std::io::Result<fs::FileType>,
    D: FnMut(&Path) -> Result<()>,
    R: FnMut(&Path) -> Result<()>,
{
    let entries = fs::read_dir(dir).map_err(|source| Error::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let Some(file_type) =
            resolve_enumerated_entry(classify_file_type(&entry).map_err(|source| Error::Io {
                path: path.clone(),
                source,
            }))?
        else {
            continue;
        };
        if file_type.is_dir() {
            if resolve_enumerated_entry(before_descend(&path))?.is_none() {
                continue;
            }
            visit_with_entry_lookups(
                &path,
                out,
                classify_file_type,
                before_descend,
                before_read_ini,
            )?;
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("ini"))
        {
            let read_ini = before_read_ini(&path).and_then(|()| extract_hashes_from_file(&path));
            let Some(bindings) = resolve_enumerated_entry(read_ini)? else {
                continue;
            };
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

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn dir_scan_propagates_entry_type_uncertainty() {
        let temp = tempfile::tempdir().expect("temporary mod directory");
        let root = temp.path().join("mod");
        fs::create_dir_all(&root).expect("create mod directory");
        let ini = root.join("unreadable.ini");
        fs::write(&ini, b"[TextureOverrideA]\nhash = 0x1\n").expect("write ini");
        let mut out = Vec::new();

        let result = visit_with_file_type(&root, &mut out, &mut |_| {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "test entry-type obstruction",
            ))
        });

        assert!(
            matches!(result, Err(Error::Io { ref path, ref source })
                if path == &ini && source.kind() == io::ErrorKind::PermissionDenied),
            "an unreadable directory entry type must not produce a partial conflict scan, got {result:?}",
        );
    }

    #[test]
    fn dir_scan_skips_entry_that_vanishes_before_type_lookup() {
        let temp = tempfile::tempdir().expect("temporary mod directory");
        let root = temp.path().join("mod");
        fs::create_dir_all(&root).expect("create mod directory");
        let vanished = root.join("vanished.ini");
        let remaining = root.join("remaining.ini");
        fs::write(&vanished, b"[TextureOverrideGone]\nhash = 0x1\n").expect("write vanishing ini");
        fs::write(&remaining, b"[TextureOverrideHere]\nhash = 0x2\n").expect("write remaining ini");
        let mut out = Vec::new();

        let result = visit_with_file_type(&root, &mut out, &mut |entry| {
            if entry.path() == vanished {
                fs::remove_file(&vanished).expect("make directory entry vanish");
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "test entry vanished",
                ));
            }
            entry.file_type()
        });

        assert!(result.is_ok(), "a vanished directory entry must be skipped");
        assert_eq!(
            out,
            vec![HashBinding {
                hash: "2".to_string(),
                section: "TextureOverrideHere".to_string(),
            }],
            "a vanished entry must not hide readable conflict evidence",
        );
    }

    #[test]
    fn dir_scan_skips_directory_that_vanishes_before_descending() {
        let temp = tempfile::tempdir().expect("temporary mod directory");
        let root = temp.path().join("mod");
        let vanished = root.join("vanished");
        fs::create_dir_all(&vanished).expect("create vanishing directory");
        fs::write(
            vanished.join("gone.ini"),
            b"[TextureOverrideGone]\nhash = 0x1\n",
        )
        .expect("write vanishing ini");
        fs::write(
            root.join("remaining.ini"),
            b"[TextureOverrideHere]\nhash = 0x2\n",
        )
        .expect("write remaining ini");
        let mut out = Vec::new();

        let result = visit_with_entry_lookups(
            &root,
            &mut out,
            &mut |entry| entry.file_type(),
            &mut |path| {
                if path == vanished {
                    fs::remove_dir_all(&vanished).expect("make directory vanish");
                    return Err(Error::Io {
                        path: vanished.clone(),
                        source: io::Error::new(io::ErrorKind::NotFound, "test directory vanished"),
                    });
                }
                Ok(())
            },
            &mut |_| Ok(()),
        );

        assert!(
            result.is_ok(),
            "a directory that vanishes after enumeration must be skipped"
        );
        assert_eq!(
            out,
            vec![HashBinding {
                hash: "2".to_string(),
                section: "TextureOverrideHere".to_string(),
            }],
            "a vanished directory must not hide readable conflict evidence",
        );
    }

    #[test]
    fn dir_scan_propagates_unreadable_directory_before_descending() {
        let temp = tempfile::tempdir().expect("temporary mod directory");
        let root = temp.path().join("mod");
        let unreadable = root.join("unreadable");
        fs::create_dir_all(&unreadable).expect("create unreadable directory");
        let mut out = Vec::new();

        let result = visit_with_entry_lookups(
            &root,
            &mut out,
            &mut |entry| entry.file_type(),
            &mut |path| {
                Err(Error::Io {
                    path: path.to_path_buf(),
                    source: io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "test directory obstruction",
                    ),
                })
            },
            &mut |_| Ok(()),
        );

        assert!(
            matches!(result, Err(Error::Io { ref path, ref source })
                if path == &unreadable && source.kind() == io::ErrorKind::PermissionDenied),
            "an unreadable enumerated directory must still fail the conflict scan, got {result:?}",
        );
    }

    fn scan_with_not_found_inside_child_traversal() -> (std::path::PathBuf, Result<Vec<HashBinding>>)
    {
        let temp = tempfile::tempdir().expect("temporary mod directory");
        let root = temp.path().join("mod");
        let child = root.join("child");
        let vanished = child.join("vanished");
        fs::create_dir_all(&vanished).expect("create nested vanishing directory");
        fs::write(
            child.join("partial.ini"),
            b"[TextureOverridePartial]\nhash = 0x2\n",
        )
        .expect("write readable sibling ini");
        fs::write(
            vanished.join("gone.ini"),
            b"[TextureOverrideGone]\nhash = 0x1\n",
        )
        .expect("write vanishing ini");
        let mut out = Vec::new();

        let result = visit_with_entry_lookups(
            &root,
            &mut out,
            &mut |entry| entry.file_type(),
            &mut |path| {
                if path == vanished {
                    fs::remove_dir_all(&vanished).expect("make nested directory vanish");
                }
                Ok(())
            },
            &mut |_| Ok(()),
        )
        .map(|()| out);

        (vanished, result)
    }

    #[test]
    fn dir_scan_propagates_not_found_from_inside_child_traversal() {
        let (vanished, result) = scan_with_not_found_inside_child_traversal();

        assert!(
            matches!(&result, Err(Error::Io { path, source })
                if path == &vanished && source.kind() == io::ErrorKind::NotFound),
            "NotFound after entering a child traversal must fail the conflict scan, got {result:?}",
        );
    }

    #[test]
    fn dir_scan_does_not_return_partial_bindings_after_nested_not_found() {
        let (_, result) = scan_with_not_found_inside_child_traversal();
        let partial = vec![HashBinding {
            hash: "2".to_string(),
            section: "TextureOverridePartial".to_string(),
        }];

        assert!(
            !matches!(&result, Ok(bindings) if bindings == &partial),
            "a nested traversal failure must not return partial bindings, got {result:?}",
        );
        assert!(
            result.is_err(),
            "the nested traversal fixture must fail rather than return any confident result",
        );
    }

    #[test]
    fn dir_scan_skips_ini_that_vanishes_before_read() {
        let temp = tempfile::tempdir().expect("temporary mod directory");
        let root = temp.path().join("mod");
        fs::create_dir_all(&root).expect("create mod directory");
        let vanished = root.join("vanished.ini");
        fs::write(&vanished, b"[TextureOverrideGone]\nhash = 0x1\n").expect("write vanishing ini");
        fs::write(
            root.join("remaining.ini"),
            b"[TextureOverrideHere]\nhash = 0x2\n",
        )
        .expect("write remaining ini");
        let mut out = Vec::new();

        let result = visit_with_entry_lookups(
            &root,
            &mut out,
            &mut |entry| entry.file_type(),
            &mut |_| Ok(()),
            &mut |path| {
                if path == vanished {
                    fs::remove_file(&vanished).expect("make ini vanish");
                    return Err(Error::Io {
                        path: vanished.clone(),
                        source: io::Error::new(io::ErrorKind::NotFound, "test ini vanished"),
                    });
                }
                Ok(())
            },
        );

        assert!(
            result.is_ok(),
            "an ini that vanishes after enumeration must be skipped"
        );
        assert_eq!(
            out,
            vec![HashBinding {
                hash: "2".to_string(),
                section: "TextureOverrideHere".to_string(),
            }],
            "a vanished ini must not hide readable conflict evidence",
        );
    }

    #[test]
    fn dir_scan_propagates_unreadable_ini_before_read() {
        let temp = tempfile::tempdir().expect("temporary mod directory");
        let root = temp.path().join("mod");
        fs::create_dir_all(&root).expect("create mod directory");
        let unreadable = root.join("unreadable.ini");
        fs::write(&unreadable, b"[TextureOverrideA]\nhash = 0x1\n").expect("write ini");
        let mut out = Vec::new();

        let result = visit_with_entry_lookups(
            &root,
            &mut out,
            &mut |entry| entry.file_type(),
            &mut |_| Ok(()),
            &mut |path| {
                Err(Error::Io {
                    path: path.to_path_buf(),
                    source: io::Error::new(io::ErrorKind::PermissionDenied, "test ini obstruction"),
                })
            },
        );

        assert!(
            matches!(result, Err(Error::Io { ref path, ref source })
                if path == &unreadable && source.kind() == io::ErrorKind::PermissionDenied),
            "an unreadable enumerated ini must still fail the conflict scan, got {result:?}",
        );
    }
}
