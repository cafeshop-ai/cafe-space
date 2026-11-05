import { Component, lazy, Suspense, useState, type ComponentProps, type ReactNode } from 'react';
import type { SessionView } from '../components/SessionView';

let loading: Promise<typeof import('../components/SessionView')> | null = null;
/** Fetches the session page chunk once; a failed fetch can be tried again, since loading has no side effects. */
export function loadSessionPage() {
  loading ??= import('../components/SessionView').catch(error => { loading = null; throw error; });
  return loading;
}
const makePage = () => lazy(() => loadSessionPage().then(module => ({ default: module.SessionView })));
let Page = makePage();

class LoadFailed extends Component<{ onRetry: () => void; children: ReactNode }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() { return { failed: true }; }
  render() {
    return this.state.failed ? <div className="empty-state"><h2>会话页没能加载</h2>
      <button className="secondary-button" onClick={this.props.onRetry}>重试</button></div> : this.props.children;
  }
}

/** The session page, assistant-ui included, is its own chunk (web/README size budget). */
export function SessionPage(props: ComponentProps<typeof SessionView>) {
  const [attempt, setAttempt] = useState(0);
  const retry = () => { Page = makePage(); setAttempt(value => value + 1); };
  return <LoadFailed key={attempt} onRetry={retry}>
    <Suspense fallback={<div className="skeleton-list" aria-label="正在读取会话" aria-busy="true"><div /><div /></div>}><Page {...props} /></Suspense>
  </LoadFailed>;
}
