import { describe, expect, it } from 'vitest';
import { acceptResults, affectedIds, exampleNotebook, parseNotebook } from './model';
import type { Analysis, RunResult } from './model';

describe('notebook state', () => {
  it('invalidates every dependent and leaves unrelated cells alone', () => {
    const notebook = exampleNotebook();
    const [a, b, c] = notebook.cells;
    if (!a || !b || !c) throw new Error('Invalid fixture');
    const analyses: Analysis[] = [
      { id: a.id, dependencies: [], compiledSql: a.sql, error: null },
      { id: b.id, dependencies: [a.name], compiledSql: b.sql, error: null },
      { id: c.id, dependencies: [b.name], compiledSql: c.sql, error: null },
      { id: 'unrelated', dependencies: [], compiledSql: 'select 1', error: null },
    ];
    expect(affectedIds(notebook, analyses, a.id)).toEqual(new Set([a.id, b.id, c.id]));
  });

  it('keeps late results stale when definitions changed during the run', () => {
    const run: RunResult = {
      runId: 'run',
      snapshot: '1:2:',
      startedAt: 1,
      cancelled: false,
      loggingError: null,
      skipped: [],
      results: [
        {
          cellId: 'a',
          compiledSql: 'SELECT 1',
          rows: [['1']],
          columns: ['n'],
          affectedRows: 1,
          truncated: false,
          elapsedMs: 2,
          error: null,
        },
      ],
    };
    expect(acceptResults({}, run, new Set(['a'])).a?.stale).toBe(true);
    expect(acceptResults({}, run, new Set()).a?.stale).toBe(false);
    expect(acceptResults({}, { ...run, cancelled: true }, new Set()).a?.stale).toBe(true);
  });

  it('round trips notebooks and rejects malformed or duplicate identities', () => {
    const notebook = exampleNotebook();
    expect(parseNotebook(JSON.stringify(notebook))).toEqual(notebook);
    expect(() => parseNotebook('{"version":2}')).toThrow();
    expect(() => parseNotebook(JSON.stringify({ ...notebook, cells: [notebook.cells[0], notebook.cells[0]] }))).toThrow(
      'unique',
    );
  });
});
