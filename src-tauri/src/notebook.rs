//! Jupyter notebook (`.ipynb`) read and cell-level edit.
//!
//! A notebook is JSON, and treating it as text is how an edit corrupts it:
//! `edit_file` on the raw JSON can replace a string that occurs in a base64
//! output as easily as one in a source cell. These functions keep the
//! structure — parse, address a cell by id, change only its `source`, and
//! write the whole document back with `nbformat` preserved.
//!
//! ## Cell identity
//!
//! `nbformat` 4.5 added an `id` to each cell; older notebooks have none. This
//! module never invents an id *in the file* — that would be a silent upgrade
//! of a document the user did not ask to migrate. Instead [`read`] reports a
//! stable handle for every cell (`cell-<index>` for an id-less one) and
//! [`edit`] resolves that same handle back to the cell it named. The handle is
//! derived from position, so it stays correct as long as the document is not
//! reordered between the read and the edit — which is the same assumption a
//! line-addressed edit already makes.

use std::path::Path;

/// One cell, as `notebook_read` reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// The notebook's own `id`, or a positional handle for an id-less cell.
    pub id: String,
    pub cell_type: String,
    pub source: String,
    /// How many output entries the cell carries; the content is not returned.
    pub outputs: usize,
}

/// A parsed notebook, reduced to what a reader needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Notebook {
    /// `metadata.kernelspec.name`, when present.
    pub kernel: Option<String>,
    pub cells: Vec<Cell>,
}

/// Why a notebook could not be read or edited.
#[derive(Debug)]
pub enum NotebookError {
    Io(String),
    /// Not JSON, or JSON that is not an `nbformat` document.
    Invalid(String),
    /// `notebook_edit` named a cell that is not there.
    NoSuchCell(String),
}

impl std::fmt::Display for NotebookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotebookError::Io(e) => write!(f, "{e}"),
            NotebookError::Invalid(e) => write!(f, "{e}"),
            NotebookError::NoSuchCell(id) => write!(f, "no cell with id '{id}'"),
        }
    }
}

/// A cell's `source` is either a list of lines or a single string, and both
/// are legal. Joining the list without a separator is correct: each element
/// already carries its own newline.
fn source_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(lines) => lines
            .iter()
            .filter_map(|l| l.as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// The public handle for the cell at `index`.
fn handle(cell: &serde_json::Value, index: usize) -> String {
    cell.get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("cell-{index}"))
}

/// Resolve a handle to a positional index.
///
/// A notebook `id` wins over the positional form, so a real cell whose id
/// happens to look like `cell-7` is still found by its id rather than by
/// position 7.
fn index_of(cells: &[serde_json::Value], handle: &str) -> Option<usize> {
    if let Some(i) = cells
        .iter()
        .position(|c| c.get("id").and_then(|v| v.as_str()) == Some(handle))
    {
        return Some(i);
    }
    let n = handle.strip_prefix("cell-")?.parse::<usize>().ok()?;
    (n < cells.len()).then_some(n)
}

fn parse(raw: &str) -> Result<serde_json::Value, NotebookError> {
    let doc: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
        NotebookError::Invalid(format!("not valid JSON: {e}"))
    })?;
    if !doc.is_object() {
        return Err(NotebookError::Invalid("notebook is not a JSON object".into()));
    }
    if doc.get("cells").and_then(|c| c.as_array()).is_none() {
        return Err(NotebookError::Invalid(
            "no `cells` array: not an nbformat notebook".into(),
        ));
    }
    Ok(doc)
}

/// Read a notebook, following the same path policy as `read_file` only in the
/// sense that the caller has already resolved the path.
pub fn read(path: &Path) -> Result<Notebook, NotebookError> {
    let raw = std::fs::read_to_string(path).map_err(|e| NotebookError::Io(e.to_string()))?;
    let doc = parse(&raw)?;
    let kernel = doc
        .get("metadata")
        .and_then(|m| m.get("kernelspec"))
        .and_then(|k| k.get("name"))
        .and_then(|n| n.as_str())
        .map(str::to_string);
    let cells = doc["cells"]
        .as_array()
        .map(|cells| {
            cells
                .iter()
                .enumerate()
                .map(|(i, c)| Cell {
                    id: handle(c, i),
                    cell_type: c
                        .get("cell_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("code")
                        .to_string(),
                    source: source_string(&c["source"]),
                    outputs: c
                        .get("outputs")
                        .and_then(|v| v.as_array())
                        .map(Vec::len)
                        .unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Notebook { kernel, cells })
}

/// One cell's new state after an edit.
#[derive(Debug, Clone, PartialEq)]
pub struct EditOutcome {
    pub cell_id: String,
    pub cell_type: String,
    pub bytes_written: usize,
}

/// Replace one cell's source, and optionally its type.
///
/// The `source` is written as a **list of lines** with their newlines kept,
/// which is the canonical `nbformat` shape and byte-identical to what Jupyter
/// itself writes. `nbformat` / `nbformat_minor` / every other cell are left
/// exactly as they were.
pub fn edit(
    path: &Path,
    cell_id: &str,
    new_source: &str,
    cell_type: Option<&str>,
) -> Result<EditOutcome, NotebookError> {
    let raw = std::fs::read_to_string(path).map_err(|e| NotebookError::Io(e.to_string()))?;
    let mut doc = parse(&raw)?;

    let cells = doc["cells"]
        .as_array_mut()
        .ok_or_else(|| NotebookError::Invalid("no `cells` array".into()))?;
    let idx = index_of(cells, cell_id).ok_or_else(|| NotebookError::NoSuchCell(cell_id.to_string()))?;

    // A new notebook-type cell type is validated here, not silently written.
    let resolved_type = match cell_type {
        None => cells[idx]
            .get("cell_type")
            .and_then(|v| v.as_str())
            .unwrap_or("code")
            .to_string(),
        Some(t) if t == "code" || t == "markdown" || t == "raw" => t.to_string(),
        Some(other) => {
            return Err(NotebookError::Invalid(format!(
                "unknown cell_type '{other}' (code, markdown, or raw)"
            )))
        }
    };

    // `split_inclusive` keeps the trailing newline on each line, so the
    // joined list is the source back, and a source with no final newline
    // stays without one.
    let lines: Vec<serde_json::Value> = new_source
        .split_inclusive('\n')
        .map(|line| serde_json::Value::String(line.to_string()))
        .collect();
    let stored = if new_source.is_empty() {
        // Jupyter spells "no source" and "one empty line" differently; an
        // empty string is the empty list, which is what it writes.
        Vec::new()
    } else {
        lines
    };

    cells[idx]["source"] = serde_json::Value::Array(stored);
    cells[idx]["cell_type"] = serde_json::Value::String(resolved_type.clone());
    // A cell that became markdown must not keep code-only keys, or the file
    // is one Jupyter will refuse to run.
    if resolved_type != "code" {
        if let Some(obj) = cells[idx].as_object_mut() {
            obj.remove("outputs");
            obj.remove("execution_count");
        }
    }

    let text = serde_json::to_string_pretty(&doc)
        .map_err(|e| NotebookError::Io(format!("could not serialise notebook: {e}")))?;
    std::fs::write(path, text.as_bytes()).map_err(|e| NotebookError::Io(e.to_string()))?;
    Ok(EditOutcome {
        cell_id: cell_id.to_string(),
        cell_type: resolved_type,
        bytes_written: text.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        serde_json::json!({
            "cells": [
                {"cell_type": "markdown", "source": ["# Title\n", "\n"], "metadata": {}},
                {
                    "cell_type": "code",
                    "id": "abc123",
                    "source": ["print(1)\n"],
                    "outputs": [{"output_type": "stream", "text": ["1\n"]}],
                    "execution_count": 1,
                    "metadata": {}
                }
            ],
            "metadata": {"kernelspec": {"name": "python3"}},
            "nbformat": 4,
            "nbformat_minor": 5
        })
        .to_string()
    }

    #[test]
    fn read_reports_ids_sources_and_kernel() {
        let dir = std::env::temp_dir().join(format!("nb-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.ipynb");
        std::fs::write(&path, fixture()).unwrap();
        let nb = read(&path).unwrap();
        assert_eq!(nb.kernel.as_deref(), Some("python3"));
        assert_eq!(nb.cells.len(), 2);
        assert_eq!(nb.cells[0].id, "cell-0");
        assert_eq!(nb.cells[0].cell_type, "markdown");
        assert_eq!(nb.cells[0].source, "# Title\n\n");
        assert_eq!(nb.cells[1].id, "abc123");
        assert_eq!(nb.cells[1].outputs, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_by_positional_handle_changes_only_the_source() {
        let dir = std::env::temp_dir().join(format!("nb-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.ipynb");
        std::fs::write(&path, fixture()).unwrap();
        let outcome = edit(&path, "cell-0", "new text", Some("markdown")).unwrap();
        assert_eq!(outcome.cell_type, "markdown");
        let after = read(&path).unwrap();
        assert_eq!(after.cells[0].source, "new text");
        // The code cell is untouched, id and all.
        assert_eq!(after.cells[1].id, "abc123");
        assert_eq!(after.cells[1].source, "print(1)\n");
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["nbformat"], 4);
        assert_eq!(doc["nbformat_minor"], 5);

        // A code → markdown conversion drops the code-only keys.
        edit(&path, "abc123", "prose", Some("markdown")).unwrap();
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(doc["cells"][1].get("outputs").is_none());
        assert!(doc["cells"][1].get("execution_count").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_real_id_wins_over_a_positional_looking_one() {
        let dir = std::env::temp_dir().join(format!("nb-id-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.ipynb");
        let doc = serde_json::json!({
            "cells": [
                {"cell_type": "code", "id": "cell-1", "source": ["a\n"]},
                {"cell_type": "code", "source": ["b\n"]}
            ],
            "metadata": {},
            "nbformat": 4,
            "nbformat_minor": 5
        });
        std::fs::write(&path, doc.to_string()).unwrap();
        assert_eq!(read(&path).unwrap().cells[0].id, "cell-1");
        // Editing "cell-1" edits the cell whose *id* is cell-1, not index 1.
        edit(&path, "cell-1", "changed", None).unwrap();
        let after = read(&path).unwrap();
        assert_eq!(after.cells[0].source, "changed");
        assert_eq!(after.cells[1].source, "b\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_documents_are_refused_not_rewritten() {
        let dir = std::env::temp_dir().join(format!("nb-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("d.ipynb");
        std::fs::write(&path, "{not json").unwrap();
        assert!(matches!(read(&path), Err(NotebookError::Invalid(_))));
        std::fs::write(&path, r#"{"foo": 1}"#).unwrap();
        assert!(matches!(read(&path), Err(NotebookError::Invalid(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
