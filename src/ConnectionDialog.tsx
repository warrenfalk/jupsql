import { Database, X } from 'lucide-react';
import { useState } from 'react';
import * as bridge from './bridge';
import type { ConnectionInput } from './model';

function initialConnection(): ConnectionInput {
  const fallback = {
    host: '127.0.0.1',
    port: 55432,
    database: 'jupsql_demo',
    user: 'jupsql',
    password: '',
    tls: false,
  };
  try {
    const saved = localStorage.getItem('jupsql.connection');
    if (!saved) return fallback;
    const value = JSON.parse(saved) as Partial<ConnectionInput>;
    return {
      host: typeof value.host === 'string' ? value.host : fallback.host,
      port: typeof value.port === 'number' ? value.port : fallback.port,
      database: typeof value.database === 'string' ? value.database : fallback.database,
      user: typeof value.user === 'string' ? value.user : fallback.user,
      tls: typeof value.tls === 'boolean' ? value.tls : fallback.tls,
      password: '',
    };
  } catch {
    return fallback;
  }
}

export function ConnectionDialog({
  onClose,
  onConnected,
}: {
  onClose: () => void;
  onConnected: (label: string) => void;
}) {
  const [value, setValue] = useState(initialConnection);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function submit() {
    setBusy(true);
    setError(null);
    try {
      const label = await bridge.connect(value);
      const { password: _password, ...profile } = value;
      localStorage.setItem('jupsql.connection', JSON.stringify(profile));
      onConnected(label);
      onClose();
    } catch (error) {
      setError(String(error));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div className="modal-backdrop">
      <section role="dialog" aria-modal="true" aria-labelledby="connect-title" className="modal">
        <div className="flex items-center justify-between">
          <span className="modal-icon">
            <Database size={21} />
          </span>
          <button className="icon-button" aria-label="Close connection dialog" onClick={onClose} disabled={busy}>
            <X size={18} />
          </button>
        </div>
        <h2 id="connect-title">Connect to PostgreSQL</h2>
        <p className="muted mb-6">Your queries run directly from this desktop app.</p>
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <fieldset disabled={busy} className="grid grid-cols-3 gap-4">
            <label className="col-span-2">
              Host
              <input
                autoFocus
                required
                value={value.host}
                onChange={(e) => setValue({ ...value, host: e.target.value })}
              />
            </label>
            <label>
              Port
              <input
                required
                type="number"
                min="1"
                max="65535"
                value={value.port}
                onChange={(e) => setValue({ ...value, port: Number(e.target.value) })}
              />
            </label>
            <label className="col-span-3">
              Database
              <input
                required
                value={value.database}
                onChange={(e) => setValue({ ...value, database: e.target.value })}
              />
            </label>
            <label className="col-span-3">
              Username
              <input required value={value.user} onChange={(e) => setValue({ ...value, user: e.target.value })} />
            </label>
            <label className="col-span-3">
              Password
              <input
                type="password"
                autoComplete="off"
                value={value.password}
                onChange={(e) => setValue({ ...value, password: e.target.value })}
                placeholder="Kept in memory for this session"
              />
            </label>
            <label className="col-span-3 flex items-center gap-2 font-normal">
              <input
                type="checkbox"
                checked={value.tls}
                onChange={(e) => setValue({ ...value, tls: e.target.checked })}
              />
              Require TLS with certificate verification
            </label>
          </fieldset>
          {error && (
            <p role="alert" className="error-box mt-4">
              {error}
            </p>
          )}
          <div className="mt-6 flex items-center justify-between gap-4">
            <span className="text-xs muted">Passwords aren’t saved or logged.</span>
            <button className="button primary" disabled={busy}>
              {busy ? 'Connecting…' : 'Connect'}
            </button>
          </div>
        </form>
      </section>
    </div>
  );
}
