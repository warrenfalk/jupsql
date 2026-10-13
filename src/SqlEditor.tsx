import { PostgreSQL, sql } from '@codemirror/lang-sql';
import { Prec } from '@codemirror/state';
import { EditorView, keymap } from '@codemirror/view';
import CodeMirror from '@uiw/react-codemirror';
import { useMemo } from 'react';

const editorTheme = EditorView.theme({
  '&': { fontSize: '13px', backgroundColor: '#fcfcfd' },
  '.cm-content': {
    fontFamily: "'SFMono-Regular', Consolas, 'Liberation Mono', monospace",
    padding: '14px 0',
    minHeight: '94px',
  },
  '.cm-line': { padding: '0 18px' },
  '.cm-gutters': { backgroundColor: '#fcfcfd', color: '#a4adbc', border: 'none', paddingLeft: '12px' },
  '.cm-activeLine, .cm-activeLineGutter': { backgroundColor: '#f2f5fa' },
  '&.cm-focused': { outline: 'none' },
  '.cm-scroller': { overflow: 'auto' },
});

export function SqlEditor({
  value,
  onChange,
  onRun,
  onRefresh,
  names,
  label,
}: {
  value: string;
  onChange: (value: string) => void;
  onRun: () => void;
  onRefresh: () => void;
  names: string[];
  label: string;
}) {
  const extensions = useMemo(
    () => [
      sql({ dialect: PostgreSQL, schema: { notebook: Object.fromEntries(names.map((name) => [name, []])) } }),
      editorTheme,
      // Override the default Mod-Enter binding, which inserts a blank line.
      Prec.high(
        keymap.of([
          {
            key: 'Mod-Enter',
            run: () => {
              onRun();
              return true;
            },
          },
          {
            key: 'Mod-Shift-Enter',
            run: () => {
              onRefresh();
              return true;
            },
          },
        ]),
      ),
      EditorView.contentAttributes.of({ 'aria-label': label }),
    ],
    [names, onRun, onRefresh, label],
  );
  return (
    <CodeMirror
      value={value}
      onChange={onChange}
      extensions={extensions}
      basicSetup={{ foldGutter: false, highlightActiveLine: true }}
    />
  );
}
