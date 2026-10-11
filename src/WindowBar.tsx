/** Application identity and the two global entry points. */
export function WindowBar({ onSettings, onSetup, setupPending }: {
  onSettings: () => void;
  onSetup: () => void;
  setupPending: boolean;
}) {
  return <header className="app__header window-bar row row--between" role="banner" aria-label="GMM">
    <strong>GMM</strong>
    <div className="row">
      <button data-settings-arrival="global" onClick={onSettings}>Settings</button>
      <button className="app__help" onClick={onSetup} disabled={setupPending}
        title="Help → Run setup again">Run setup again</button>
    </div>
  </header>;
}
