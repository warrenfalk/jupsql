import { Check, Clock3, Download, TriangleAlert } from 'lucide-react';
import { useState } from 'react';
import * as bridge from './bridge';
import type { DisplayResult } from './model';

export async function download(name: string, text: string, type: string) {
  if (bridge.desktop) {
    await bridge.exportFile(name, text);
    return;
  }
  const url = URL.createObjectURL(new Blob([text], { type }));
  const link = document.createElement('a');
  link.href = url;
  link.download = name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export function Results({ result, stale, name }: { result: DisplayResult; stale: boolean; name: string }) {
  const [page, setPage] = useState(0);
  const [exportError, setExportError] = useState<string | null>(null);
  const pages = Math.max(1, Math.ceil(result.rows.length / 50));
  const current = Math.min(page, pages - 1);
  return (
    <div className="results">
      <div className={`result-bar ${stale ? 'result-stale' : ''}`}>
        <div className="flex items-center gap-2">
          {result.error ? <TriangleAlert size={13} /> : stale ? <Clock3 size={13} /> : <Check size={13} />}
          <strong>{result.error ? 'Query failed' : stale ? 'Stale result' : 'Result'}</strong>
          {!result.error && (
            <span>
              {result.rows.length.toLocaleString()} row{result.rows.length === 1 ? '' : 's'}
              {result.truncated ? ' · preview limited' : ''}
            </span>
          )}
        </div>
        <div className="flex items-center gap-3">
          <span>{result.elapsedMs.toLocaleString()} ms</span>
          <time title={`Run ${result.runId}\nSnapshot ${result.snapshot}`}>
            {new Date(result.startedAt).toLocaleTimeString([], {
              hour: '2-digit',
              minute: '2-digit',
              second: '2-digit',
            })}
          </time>
          {!result.error && result.columns.length > 0 && (
            <button
              className="icon-button small"
              aria-label={`Export ${name} displayed rows as CSV`}
              title="Export displayed rows"
              onClick={() => {
                const escape = (value: string | null) => (value === null ? '' : `"${value.replaceAll('"', '""')}"`);
                void download(
                  `${name}.csv`,
                  [result.columns, ...result.rows].map((row) => row.map(escape).join(',')).join('\n'),
                  'text/csv',
                ).catch((error: unknown) => setExportError(String(error)));
              }}
            >
              <Download size={13} />
            </button>
          )}
        </div>
      </div>
      {exportError && (
        <p className="query-error" role="alert">
          Export failed: {exportError}
        </p>
      )}
      {result.error ? (
        <pre role="alert" className="query-error">
          {result.error}
        </pre>
      ) : result.columns.length ? (
        <>
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th className="row-number">#</th>
                  {result.columns.map((column, index) => (
                    <th key={index}>{column}</th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {result.rows.slice(current * 50, (current + 1) * 50).map((row, index) => (
                  <tr key={index}>
                    <td className="row-number">{current * 50 + index + 1}</td>
                    {row.map((value, column) => (
                      <td key={column} title={value ?? 'NULL'}>
                        {value === null ? <span className="null-value">NULL</span> : value}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
            {result.rows.length === 0 && <div className="empty-result">The query returned no rows.</div>}
          </div>
          {(pages > 1 || result.truncated) && (
            <div className="pagination">
              <span>
                {result.truncated
                  ? 'Showing a bounded preview. References use the full query.'
                  : `${result.rows.length} rows`}
              </span>
              {pages > 1 && (
                <div className="flex items-center gap-3">
                  <button disabled={current === 0} onClick={() => setPage(current - 1)}>
                    Previous
                  </button>
                  <span>
                    {current + 1} / {pages}
                  </span>
                  <button disabled={current + 1 === pages} onClick={() => setPage(current + 1)}>
                    Next
                  </button>
                </div>
              )}
            </div>
          )}
        </>
      ) : (
        <div className="empty-result">Statement completed · {result.affectedRows} rows affected</div>
      )}
    </div>
  );
}
