use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CellKind {
    Query,
    Mutation,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Cell {
    pub id: String,
    pub name: String,
    pub kind: CellKind,
    pub sql: String,
    pub notes: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Notebook {
    pub version: u32,
    pub title: String,
    pub cells: Vec<Cell>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    pub id: String,
    pub dependencies: Vec<String>,
    pub compiled_sql: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInput {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub user: String,
    pub password: String,
    pub tls: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CellResult {
    pub cell_id: String,
    pub compiled_sql: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub affected_rows: u64,
    pub truncated: bool,
    pub elapsed_ms: u128,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResult {
    pub run_id: String,
    pub snapshot: String,
    pub started_at: u128,
    pub results: Vec<CellResult>,
    pub skipped: Vec<String>,
    pub cancelled: bool,
    pub logging_error: Option<String>,
}

pub fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
