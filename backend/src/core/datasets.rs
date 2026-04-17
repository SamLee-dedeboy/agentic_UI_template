//! Uploaded-dataset store.
//!
//! Two maps:
//!  - `datasets`: cookie-owned records of uploaded files (path, schema,
//!    row count, sample rows). One entry per upload.
//!  - `session_bindings`: which dataset, if any, is active for a given
//!    chat session. Set by `POST /api/datasets/bind`; consulted by the
//!    Claude spawn code to decide whether to attach the Python MCP
//!    sidecar.
//!
//! Both live in memory; a process restart loses everything. Good enough
//! for a template and for single-instance deploys. A fork that wants
//! persistence should wrap `DatasetStore` with a disk-backed layer or
//! upgrade to an object store.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    /// Inferred schema type: "string" | "integer" | "number" | "boolean" | "null" | "mixed".
    pub dtype: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DatasetRecord {
    pub dataset_id: String,
    #[serde(skip)]
    pub path: PathBuf,
    pub format: String,
    pub filename: String,
    pub columns: Vec<ColumnInfo>,
    pub row_count: usize,
    pub sample_rows: Vec<serde_json::Value>,
    #[serde(skip)]
    pub cookie_id: String,
}

/// The subset of a `DatasetRecord` that the Claude-spawn path cares
/// about. Cheap to clone (the sample rows aren't needed once the Python
/// server has the file on disk).
#[derive(Debug, Clone)]
pub struct DatasetBinding {
    pub dataset_id: String,
    pub path: PathBuf,
    pub format: String,
    pub filename: String,
    pub columns: Vec<ColumnInfo>,
    pub row_count: usize,
}

#[derive(Clone, Default)]
pub struct DatasetStore {
    datasets: Arc<Mutex<HashMap<String, DatasetRecord>>>,
    session_bindings: Arc<Mutex<HashMap<String, DatasetBinding>>>,
}

impl DatasetStore {
    pub async fn insert(&self, record: DatasetRecord) {
        self.datasets
            .lock()
            .await
            .insert(record.dataset_id.clone(), record);
    }

    /// Returns the record only if the presented `cookie_id` owns it.
    /// Prevents cross-cookie binds.
    pub async fn get_owned(&self, dataset_id: &str, cookie_id: &str) -> Option<DatasetRecord> {
        self.datasets
            .lock()
            .await
            .get(dataset_id)
            .filter(|r| r.cookie_id == cookie_id)
            .cloned()
    }

    pub async fn bind(&self, session_id: &str, binding: DatasetBinding) {
        self.session_bindings
            .lock()
            .await
            .insert(session_id.to_string(), binding);
    }

    pub async fn binding(&self, session_id: &str) -> Option<DatasetBinding> {
        self.session_bindings.lock().await.get(session_id).cloned()
    }

    /// Drop the binding (but not the underlying dataset — the user may
    /// rebind it in a new chat). Called from WebSocket teardown.
    pub async fn unbind(&self, session_id: &str) {
        self.session_bindings.lock().await.remove(session_id);
    }
}

// ---------------------------------------------------------------------------
// Schema inference.
// ---------------------------------------------------------------------------

/// Parse a CSV file into (columns, row_count, up-to-5 sample rows).
/// Errors are best-effort — malformed rows abort the read and return an
/// error string suitable for a 400 response.
pub fn parse_csv(
    path: &Path,
) -> Result<(Vec<ColumnInfo>, usize, Vec<serde_json::Value>), String> {
    let mut rdr = csv::Reader::from_path(path).map_err(|e| format!("opening CSV: {e}"))?;
    let headers: Vec<String> = rdr
        .headers()
        .map_err(|e| format!("reading CSV headers: {e}"))?
        .iter()
        .map(String::from)
        .collect();

    let mut sample_rows: Vec<serde_json::Value> = Vec::new();
    let mut row_count: usize = 0;
    // Store up to 200 rows for type inference, but only return 5 as the
    // sample. Inference on 200 is typically enough to catch mixed types.
    let mut infer_rows: Vec<serde_json::Value> = Vec::new();

    for rec_res in rdr.records() {
        let rec = rec_res.map_err(|e| format!("parsing CSV row {}: {e}", row_count + 1))?;
        if infer_rows.len() < 200 {
            let mut obj = serde_json::Map::new();
            for (i, h) in headers.iter().enumerate() {
                let cell = rec.get(i).unwrap_or("");
                obj.insert(h.clone(), infer_cell_value(cell));
            }
            let row_val = serde_json::Value::Object(obj);
            if sample_rows.len() < 5 {
                sample_rows.push(row_val.clone());
            }
            infer_rows.push(row_val);
        }
        row_count += 1;
    }

    let columns = infer_columns(&headers, &infer_rows);
    Ok((columns, row_count, sample_rows))
}

/// Parse a JSON array-of-objects file. The root must be a JSON array;
/// each element must be an object. Heterogeneous keys are unioned.
pub fn parse_json_array(
    path: &Path,
) -> Result<(Vec<ColumnInfo>, usize, Vec<serde_json::Value>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("reading JSON: {e}"))?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("parsing JSON: {e}"))?;
    let array = value
        .as_array()
        .ok_or_else(|| "JSON root must be an array of objects".to_string())?;

    // Union of keys seen across the first 200 rows, preserving
    // first-seen order so the column ordering is stable and intuitive.
    let mut seen_keys: Vec<String> = Vec::new();
    let mut key_set = std::collections::HashSet::new();
    let infer_sample: Vec<&serde_json::Value> = array.iter().take(200).collect();
    for row in &infer_sample {
        if let Some(obj) = row.as_object() {
            for k in obj.keys() {
                if key_set.insert(k.clone()) {
                    seen_keys.push(k.clone());
                }
            }
        }
    }

    let sample_rows: Vec<serde_json::Value> = array.iter().take(5).cloned().collect();
    let row_count = array.len();

    let infer_rows: Vec<serde_json::Value> = infer_sample.into_iter().cloned().collect();
    let columns = infer_columns(&seen_keys, &infer_rows);
    Ok((columns, row_count, sample_rows))
}

/// CSV cells arrive as strings; try to JSON-ify them so downstream
/// tooling sees numbers as numbers.
fn infer_cell_value(raw: &str) -> serde_json::Value {
    if raw.is_empty() {
        return serde_json::Value::Null;
    }
    if let Ok(n) = raw.parse::<i64>() {
        return serde_json::Value::Number(n.into());
    }
    if let Ok(f) = raw.parse::<f64>() {
        if let Some(num) = serde_json::Number::from_f64(f) {
            return serde_json::Value::Number(num);
        }
    }
    match raw.to_ascii_lowercase().as_str() {
        "true" => serde_json::Value::Bool(true),
        "false" => serde_json::Value::Bool(false),
        _ => serde_json::Value::String(raw.to_string()),
    }
}

fn infer_columns(keys: &[String], rows: &[serde_json::Value]) -> Vec<ColumnInfo> {
    keys.iter()
        .map(|k| ColumnInfo {
            name: k.clone(),
            dtype: infer_column_type(rows, k),
        })
        .collect()
}

fn infer_column_type(rows: &[serde_json::Value], col: &str) -> String {
    let mut seen: std::collections::HashSet<&'static str> = std::collections::HashSet::new();
    for row in rows {
        let val = row.get(col);
        let kind = match val {
            None | Some(serde_json::Value::Null) => continue,
            Some(serde_json::Value::Bool(_)) => "boolean",
            Some(serde_json::Value::Number(n)) => {
                if n.is_i64() || n.is_u64() {
                    "integer"
                } else {
                    "number"
                }
            }
            Some(serde_json::Value::String(_)) => "string",
            Some(_) => "mixed",
        };
        seen.insert(kind);
    }
    if seen.is_empty() {
        return "null".into();
    }
    if seen.len() == 1 {
        return seen.into_iter().next().unwrap().into();
    }
    // integer + number → number (both numeric).
    if seen.len() == 2 && seen.contains("integer") && seen.contains("number") {
        return "number".into();
    }
    "mixed".into()
}
