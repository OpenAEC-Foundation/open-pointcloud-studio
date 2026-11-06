import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import '@openaec/ui/css/tokens.css';
import '@openaec/ui/css/components.css';
import './styles/openaec-overrides.css';   // <-- nieuw
import './styles/globals.css';

class ErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null }
> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    console.error('[ErrorBoundary]', error, info.componentStack);
  }

  render() {
    if (this.state.error) {
      return (
        <div style={{ padding: 40, color: '#f87171', background: '#36363E', minHeight: '100vh', fontFamily: 'monospace' }}>
          <h1 style={{ color: '#f87171' }}>Runtime Error</h1>
          <pre style={{ whiteSpace: 'pre-wrap', fontSize: 14, color: '#FAFAF9' }}>
            {this.state.error.message}
          </pre>
          <pre style={{ whiteSpace: 'pre-wrap', fontSize: 12, color: 'rgba(250, 250, 249, 0.6)', marginTop: 16 }}>
            {this.state.error.stack}
          </pre>
          <button
            onClick={() => this.setState({ error: null })}
            style={{ marginTop: 20, padding: '8px 16px', background: '#D97706', color: 'white', border: 'none', borderRadius: 4, cursor: 'pointer' }}
          >
            Try Again
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>
);
