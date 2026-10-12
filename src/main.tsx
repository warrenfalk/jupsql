import { createRoot } from 'react-dom/client';
import { App } from './App';
import { bootstrap } from './bridge';
import './styles.css';

const root = createRoot(document.getElementById('root')!);
root.render(<div className="startup">Opening your notebook…</div>);
void bootstrap()
  .then((initial) => root.render(<App initial={initial} />))
  .catch((error: unknown) => {
    root.render(
      <div className="startup">
        <h1>Couldn’t open the notebook</h1>
        <p>{String(error)}</p>
        <p>Your existing file has been preserved.</p>
      </div>,
    );
  });
