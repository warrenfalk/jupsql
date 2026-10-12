import { ArrowDown, ArrowUp, Braces, ChevronDown, ChevronRight, GitBranch, Play, Trash2 } from 'lucide-react';
import { useState } from 'react';
import type { Analysis, Cell, DisplayResult } from './model';
import { Results } from './Results';
import { SqlEditor } from './SqlEditor';

export function CellCard({
  cell,
  index,
  count,
  names,
  analysis,
  result,
  canRun,
  running,
  onChange,
  onRun,
  onRefresh,
  onDelete,
  onMove,
  onFocus,
}: {
  cell: Cell;
  index: number;
  count: number;
  names: string[];
  analysis: Analysis | undefined;
  result: DisplayResult | undefined;
  canRun: boolean;
  running: boolean;
  onChange: (cell: Cell) => void;
  onRun: () => void;
  onRefresh: () => void;
  onDelete: () => void;
  onMove: (direction: -1 | 1) => void;
  onFocus: () => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const [collapsed, setCollapsed] = useState(false);
  const stale = !!result && (result.stale || result.compiledSql !== analysis?.compiledSql || !!analysis?.error);
  return (
    <article id={`cell-${cell.id}`} className={`cell-card ${running ? 'cell-running' : ''}`} onFocusCapture={onFocus}>
      <header className="cell-header">
        <div className="flex min-w-0 items-center gap-3">
          <button
            className="icon-button small"
            aria-label={`${collapsed ? 'Expand' : 'Collapse'} ${cell.name}`}
            onClick={() => setCollapsed(!collapsed)}
          >
            {collapsed ? <ChevronRight size={15} /> : <ChevronDown size={15} />}
          </button>
          <span className="cell-number">{String(index + 1).padStart(2, '0')}</span>
          <input
            className="cell-name"
            aria-label={`Cell ${index + 1} name`}
            value={cell.name}
            spellCheck={false}
            onChange={(e) => onChange({ ...cell, name: e.target.value })}
          />
          <select
            aria-label={`${cell.name} type`}
            className={`cell-kind ${cell.kind}`}
            value={cell.kind}
            onChange={(e) => onChange({ ...cell, kind: e.target.value === 'mutation' ? 'mutation' : 'query' })}
          >
            <option value="query">Query</option>
            <option value="mutation">Mutation</option>
          </select>
          {running ? (
            <span className="state-badge running">Running</span>
          ) : stale ? (
            <span className="state-badge stale">Stale</span>
          ) : null}
        </div>
        <div className="cell-actions">
          <button
            className="icon-button small"
            aria-label={`Move ${cell.name} up`}
            disabled={index === 0}
            onClick={() => onMove(-1)}
          >
            <ArrowUp size={14} />
          </button>
          <button
            className="icon-button small"
            aria-label={`Move ${cell.name} down`}
            disabled={index === count - 1}
            onClick={() => onMove(1)}
          >
            <ArrowDown size={14} />
          </button>
          <button className="icon-button small" aria-label={`Delete ${cell.name}`} onClick={onDelete}>
            <Trash2 size={14} />
          </button>
          <span className="action-divider" />
          <button
            className="button compact"
            title="Refresh this cell and its downstream queries in one snapshot; mutations are skipped"
            disabled={!canRun || cell.kind !== 'query'}
            onClick={onRefresh}
          >
            <GitBranch size={13} />
            <span>Rerun dependents</span>
          </button>
          <button
            className={`button compact ${cell.kind === 'mutation' ? 'mutation-button' : 'run-button'}`}
            disabled={!canRun || !!analysis?.error}
            onClick={onRun}
          >
            <Play size={13} fill="currentColor" />
            {cell.kind === 'mutation' ? 'Run mutation' : 'Run'}
          </button>
        </div>
      </header>
      {!collapsed && (
        <>
          <textarea
            className="cell-notes"
            aria-label={`${cell.name} notes`}
            placeholder="Add a note about this step…"
            value={cell.notes}
            rows={1}
            onChange={(e) => onChange({ ...cell, notes: e.target.value })}
          />
          <SqlEditor
            value={cell.sql}
            onChange={(sql) => onChange({ ...cell, sql })}
            onRun={onRun}
            names={names}
            label={`${cell.name} SQL`}
          />
          <div className="cell-footer">
            <div className="flex items-center gap-2">
              <GitBranch size={12} />
              {analysis?.dependencies.length ? (
                <span>
                  Uses{' '}
                  {analysis.dependencies.map((name) => (
                    <code key={name}>{name}</code>
                  ))}
                </span>
              ) : (
                <span>Independent cell</span>
              )}
            </div>
            <button className="text-button" onClick={() => setExpanded(!expanded)}>
              <Braces size={12} />
              {expanded ? 'Hide expanded SQL' : 'Expanded SQL'}
            </button>
          </div>
          {analysis?.error && (
            <p className="validation-error" role="alert">
              {analysis.error}
            </p>
          )}
          {expanded && (
            <pre className="expanded-sql">{analysis?.compiledSql ?? 'Fix the query to see its expanded SQL.'}</pre>
          )}
          {result && <Results result={result} stale={stale} name={cell.name} />}
        </>
      )}
    </article>
  );
}
