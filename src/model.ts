export type Cell = {
  id: string;
  name: string;
  kind: 'query' | 'mutation';
  sql: string;
  notes: string;
};

export type Notebook = { version: 1; title: string; cells: Cell[] };
export type Analysis = { id: string; dependencies: string[]; compiledSql: string | null; error: string | null };
export type ConnectionInput = {
  host: string;
  port: number;
  database: string;
  user: string;
  password: string;
  tls: boolean;
};
export type ConnectionDefaults = { input: ConnectionInput; autoConnect: boolean; error: string | null };
export type Bootstrap = {
  notebook: Notebook | null;
  sessionId: string;
  logPath: string;
  dataDirectory: string;
  connectionDefaults: ConnectionDefaults;
};
export type CellResult = {
  cellId: string;
  compiledSql: string;
  columns: string[];
  rows: (string | null)[][];
  affectedRows: number;
  truncated: boolean;
  elapsedMs: number;
  error: string | null;
};
export type RunResult = {
  runId: string;
  snapshot: string;
  startedAt: number;
  results: CellResult[];
  skipped: string[];
  cancelled: boolean;
  loggingError: string | null;
};
export type DisplayResult = CellResult & { runId: string; snapshot: string; startedAt: number; stale: boolean };
export type Progress = { runId: string; cellId: string; index: number; total: number };

export function newCell(index: number): Cell {
  return { id: crypto.randomUUID(), name: `query_${index}`, kind: 'query', sql: '', notes: '' };
}

export function newNotebook(): Notebook {
  return {
    version: 1,
    title: 'Untitled notebook',
    cells: [newCell(1)],
  };
}

export function affectedIds(notebook: Notebook, analyses: Analysis[], changedId: string): Set<string> {
  const affected = new Set([changedId]);
  let previousSize = 0;
  while (previousSize !== affected.size) {
    previousSize = affected.size;
    const names = new Set(notebook.cells.filter((cell) => affected.has(cell.id)).map((cell) => cell.name));
    for (const analysis of analyses) {
      if (analysis.dependencies.some((name) => names.has(name))) affected.add(analysis.id);
    }
  }
  return affected;
}

export function acceptResults(
  previous: Record<string, DisplayResult>,
  run: RunResult,
  changedDuringRun: Set<string>,
): Record<string, DisplayResult> {
  return {
    ...previous,
    ...Object.fromEntries(
      run.results.map((result) => [
        result.cellId,
        {
          ...result,
          runId: run.runId,
          snapshot: run.snapshot,
          startedAt: run.startedAt,
          stale: changedDuringRun.has(result.cellId) || run.cancelled || result.error !== null,
        },
      ]),
    ),
  };
}

export function parseNotebook(text: string): Notebook {
  const value: unknown = JSON.parse(text);
  if (
    !value ||
    typeof value !== 'object' ||
    !('version' in value) ||
    value.version !== 1 ||
    !('title' in value) ||
    typeof value.title !== 'string' ||
    !('cells' in value) ||
    !Array.isArray(value.cells) ||
    value.cells.length > 100
  ) {
    throw new Error('Choose a JupSQL notebook with version 1 and up to 100 cells.');
  }
  const cells: Cell[] = value.cells.map((cell: unknown) => {
    if (
      !cell ||
      typeof cell !== 'object' ||
      !('id' in cell) ||
      typeof cell.id !== 'string' ||
      !('name' in cell) ||
      typeof cell.name !== 'string' ||
      !('sql' in cell) ||
      typeof cell.sql !== 'string' ||
      !('notes' in cell) ||
      typeof cell.notes !== 'string' ||
      !('kind' in cell) ||
      (cell.kind !== 'query' && cell.kind !== 'mutation')
    ) {
      throw new Error('A cell in this file has an invalid format.');
    }
    return { id: cell.id, name: cell.name, sql: cell.sql, notes: cell.notes, kind: cell.kind };
  });
  if (new Set(cells.map((cell) => cell.id)).size !== cells.length) throw new Error('Cell IDs must be unique.');
  return { version: 1, title: value.title, cells };
}
