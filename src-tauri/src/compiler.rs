use crate::model::{Analysis, CellKind, Notebook};
use sqlparser::{
    ast::{
        Ident, ObjectName, Query, SetExpr, Statement, TableAlias, TableFactor, Visit, VisitMut,
        Visitor, VisitorMut,
    },
    dialect::PostgreSqlDialect,
    parser::Parser,
};
use std::{collections::BTreeSet, ops::ControlFlow};

pub fn validate_notebook(notebook: &Notebook) -> Result<(), String> {
    if notebook.version != 1 {
        return Err("This notebook version is not supported.".into());
    }
    if notebook.cells.len() > 100 {
        return Err("The prototype supports up to 100 cells per notebook.".into());
    }
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    for cell in &notebook.cells {
        if cell.id.is_empty() || !ids.insert(&cell.id) {
            return Err("Every cell needs a unique ID.".into());
        }
        if !valid_name(&cell.name) {
            return Err(format!(
                "Cell name '{}' must start with a lowercase letter and contain only lowercase letters, numbers, or underscores.",
                cell.name
            ));
        }
        if !names.insert(&cell.name) {
            return Err(format!("Cell name '{}' is used more than once.", cell.name));
        }
        if cell.sql.len() > 100_000 {
            return Err(format!(
                "Cell '{}' exceeds the 100 KB SQL limit.",
                cell.name
            ));
        }
    }
    Ok(())
}

fn valid_name(name: &str) -> bool {
    name.len() <= 63
        && name.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
}

fn parse(sql: &str) -> Result<Statement, String> {
    let mut statements = Parser::new(&PostgreSqlDialect {})
        .with_recursion_limit(64)
        .try_with_sql(sql)
        .map_err(|e| e.to_string())?
        .parse_statements()
        .map_err(|e| e.to_string())?;
    if statements.len() != 1 {
        return Err("Write exactly one SQL statement per cell.".into());
    }
    Ok(statements.remove(0))
}

fn reference(name: &ObjectName) -> Option<String> {
    let parts = name
        .0
        .iter()
        .map(|p| p.as_ident())
        .collect::<Option<Vec<_>>>()?;
    if parts.len() != 2 {
        return None;
    }
    let normalize = |id: &Ident| {
        if id.quote_style.is_some() {
            id.value.clone()
        } else {
            id.value.to_lowercase()
        }
    };
    (normalize(parts[0]) == "notebook").then(|| normalize(parts[1]))
}

struct Dependencies(BTreeSet<String>);
impl Visitor for Dependencies {
    type Break = ();
    fn pre_visit_relation(&mut self, name: &ObjectName) -> ControlFlow<()> {
        if let Some(name) = reference(name) {
            self.0.insert(name);
        }
        ControlFlow::Continue(())
    }
}

pub fn dependencies(sql: &str) -> Result<Vec<String>, String> {
    let statement = parse(sql)?;
    let mut visitor = Dependencies(BTreeSet::new());
    let _ = statement.visit(&mut visitor);
    Ok(visitor.0.into_iter().collect())
}

struct ReadOnly;
impl Visitor for ReadOnly {
    type Break = String;
    fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<String> {
        if !matches!(statement, Statement::Query(_)) {
            return ControlFlow::Break("Query cells cannot contain writes, including writes inside WITH. Use a mutation cell.".into());
        }
        ControlFlow::Continue(())
    }
    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<String> {
        if !query.locks.is_empty() || contains_into(&query.body) {
            return ControlFlow::Break(
                "Query cells cannot use SELECT INTO or row-locking clauses.".into(),
            );
        }
        ControlFlow::Continue(())
    }
}

fn contains_into(body: &SetExpr) -> bool {
    match body {
        SetExpr::Select(select) => select.into.is_some(),
        SetExpr::SetOperation { left, right, .. } => contains_into(left) || contains_into(right),
        _ => false,
    }
}

struct Expand<'a> {
    notebook: &'a Notebook,
    stack: &'a mut Vec<String>,
    remaining: &'a mut usize,
}

impl VisitorMut for Expand<'_> {
    type Break = String;
    fn post_visit_table_factor(&mut self, factor: &mut TableFactor) -> ControlFlow<String> {
        let TableFactor::Table {
            name,
            alias,
            args,
            sample,
            ..
        } = factor
        else {
            return ControlFlow::Continue(());
        };
        let Some(name) = reference(name) else {
            return ControlFlow::Continue(());
        };
        if args.is_some() || sample.is_some() {
            return ControlFlow::Break(
                "Notebook references cannot have function arguments or TABLESAMPLE.".into(),
            );
        }
        let Some(cell) = self.notebook.cells.iter().find(|cell| cell.name == name) else {
            return ControlFlow::Break(format!("Referenced cell '{name}' does not exist."));
        };
        if cell.kind != CellKind::Query {
            return ControlFlow::Break(format!(
                "'{name}' is a mutation. Only query definitions can be referenced."
            ));
        }
        let query = match compile_inner(self.notebook, &cell.id, self.stack, self.remaining) {
            Ok(Statement::Query(query)) => query,
            Ok(_) => return ControlFlow::Break(format!("'{name}' is not a query.")),
            Err(error) => return ControlFlow::Break(error),
        };
        *factor = TableFactor::Derived {
            lateral: false,
            subquery: query,
            alias: Some(alias.clone().unwrap_or(TableAlias {
                explicit: true,
                name: Ident::new(name),
                columns: vec![],
            })),
            sample: None,
        };
        ControlFlow::Continue(())
    }
}

fn compile_inner(
    notebook: &Notebook,
    id: &str,
    stack: &mut Vec<String>,
    remaining: &mut usize,
) -> Result<Statement, String> {
    let cell = notebook
        .cells
        .iter()
        .find(|cell| cell.id == id)
        .ok_or("Cell does not exist.")?;
    if stack.contains(&cell.name) {
        return Err(format!(
            "Circular reference: {} → {}",
            stack.join(" → "),
            cell.name
        ));
    }
    if stack.len() >= 32 || *remaining == 0 {
        return Err(
            "Dependency expansion is too large. Reduce nesting or repeated references.".into(),
        );
    }
    *remaining -= 1;
    let mut statement = parse(&cell.sql).map_err(|e| format!("{}: {e}", cell.name))?;
    match cell.kind {
        CellKind::Query => {
            if let ControlFlow::Break(error) = statement.visit(&mut ReadOnly) {
                return Err(format!("{}: {error}", cell.name));
            }
        }
        CellKind::Mutation => {
            if !matches!(
                statement,
                Statement::Insert(_)
                    | Statement::Update { .. }
                    | Statement::Delete(_)
                    | Statement::Query(_)
            ) {
                return Err("Mutation cells support INSERT, UPDATE, DELETE, and WITH/SELECT statements. Transaction control and schema changes are outside this prototype.".into());
            }
        }
    }
    stack.push(cell.name.clone());
    let result = VisitMut::visit(
        &mut statement,
        &mut Expand {
            notebook,
            stack,
            remaining,
        },
    );
    stack.pop();
    if let ControlFlow::Break(error) = result {
        return Err(error);
    }
    let mut remaining_refs = Dependencies(BTreeSet::new());
    let _ = statement.visit(&mut remaining_refs);
    if !remaining_refs.0.is_empty() {
        return Err("Use notebook references in FROM/JOIN expressions, never as mutation targets or TABLE shorthand.".into());
    }
    if statement.to_string().len() > 1_000_000 {
        return Err("Expanded SQL exceeds 1 MB. Reduce repeated references.".into());
    }
    Ok(statement)
}

pub fn compile(notebook: &Notebook, id: &str) -> Result<String, String> {
    validate_notebook(notebook)?;
    compile_inner(notebook, id, &mut vec![], &mut 500).map(|statement| statement.to_string())
}

pub fn analyze(notebook: &Notebook) -> Vec<Analysis> {
    notebook
        .cells
        .iter()
        .map(|cell| {
            let dependencies = dependencies(&cell.sql).unwrap_or_default();
            match compile(notebook, &cell.id) {
                Ok(sql) => Analysis {
                    id: cell.id.clone(),
                    dependencies,
                    compiled_sql: Some(sql),
                    error: None,
                },
                Err(error) => Analysis {
                    id: cell.id.clone(),
                    dependencies,
                    compiled_sql: None,
                    error: Some(error),
                },
            }
        })
        .collect()
}

/// The source is included so its visible results and its dependents share a snapshot.
pub fn refresh_targets(
    notebook: &Notebook,
    source: Option<&str>,
) -> Result<(Vec<String>, Vec<String>), String> {
    validate_notebook(notebook)?;
    let mut names = BTreeSet::new();
    if let Some(id) = source {
        let cell = notebook
            .cells
            .iter()
            .find(|c| c.id == id)
            .ok_or("Cell does not exist.")?;
        names.insert(cell.name.clone());
        loop {
            let before = names.len();
            for cell in &notebook.cells {
                if dependencies(&cell.sql)
                    .unwrap_or_default()
                    .iter()
                    .any(|name| names.contains(name))
                {
                    names.insert(cell.name.clone());
                }
            }
            if names.len() == before {
                break;
            }
        }
    }
    let included = notebook
        .cells
        .iter()
        .filter(|cell| source.is_none() || names.contains(&cell.name));
    let (queries, mutations): (Vec<_>, Vec<_>) =
        included.partition(|cell| cell.kind == CellKind::Query);
    Ok((
        queries.iter().map(|cell| cell.id.clone()).collect(),
        mutations.iter().map(|cell| cell.id.clone()).collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Cell;

    fn book(queries: &[(&str, &str)]) -> Notebook {
        Notebook {
            version: 1,
            title: "Test".into(),
            cells: queries
                .iter()
                .map(|(name, sql)| Cell {
                    id: name.to_string(),
                    name: name.to_string(),
                    kind: CellKind::Query,
                    sql: sql.to_string(),
                    notes: String::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn expands_current_sql_transitively_and_preserves_aliases() {
        let mut notebook = book(&[
            ("base", "select 1 as n"),
            ("middle", "select b.n from notebook.base b"),
            ("last", "select * from notebook.middle"),
        ]);
        notebook.cells[0].sql = "select 42 as n".into();
        let sql = compile(&notebook, "last").unwrap();
        assert!(sql.contains("SELECT 42 AS n"));
        assert!(sql.contains(") b"));
        assert!(!sql.contains("notebook."));
    }

    #[test]
    fn ignores_strings_and_comments_and_handles_quoted_identifiers() {
        let notebook = book(&[
            ("base", "select 'notebook.missing' as n -- notebook.other\n"),
            ("last", "select * from \"notebook\".\"base\""),
        ]);
        assert_eq!(
            dependencies(&notebook.cells[0].sql).unwrap(),
            Vec::<String>::new()
        );
        assert!(
            compile(&notebook, "last")
                .unwrap()
                .contains("'notebook.missing'")
        );
    }

    #[test]
    fn rejects_cycles_missing_cells_and_multiple_statements() {
        assert!(
            compile(
                &book(&[
                    ("a", "select * from notebook.b"),
                    ("b", "select * from notebook.a")
                ]),
                "a"
            )
            .unwrap_err()
            .contains("Circular")
        );
        assert!(
            compile(&book(&[("a", "select * from notebook.missing")]), "a")
                .unwrap_err()
                .contains("does not exist")
        );
        assert!(
            compile(&book(&[("a", "select 1; delete from t")]), "a")
                .unwrap_err()
                .contains("one SQL statement")
        );
    }

    #[test]
    fn rejects_writes_hidden_in_queries() {
        for sql in [
            "delete from t returning *",
            "with x as (delete from t returning *) select * from x",
            "select * into new_table from t",
            "select * from t for update",
        ] {
            assert!(compile(&book(&[("a", sql)]), "a").is_err(), "{sql}");
        }
    }

    #[test]
    fn mutations_can_consume_queries_but_cannot_be_references_or_bulk_targets() {
        let mut notebook = book(&[
            ("base", "select 1 as id"),
            (
                "write",
                "delete from t where id in (select id from notebook.base) returning *",
            ),
            ("summary", "select count(*) from notebook.base"),
        ]);
        notebook.cells[1].kind = CellKind::Mutation;
        assert!(compile(&notebook, "write").is_ok());
        let (targets, skipped) = refresh_targets(&notebook, Some("base")).unwrap();
        assert_eq!(targets, ["base", "summary"]);
        assert_eq!(skipped, ["write"]);
        notebook.cells[2].sql = "select * from notebook.write".into();
        assert!(
            compile(&notebook, "summary")
                .unwrap_err()
                .contains("mutation")
        );
    }
}
