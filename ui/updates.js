/* Pure updater view-state helpers; release notes are always rendered as text. */
(function (root) {
  'use strict';
  function normalize(status) {
    if (!status) return { state: 'unavailable', reason: 'Update status is not available yet.' };
    const state = ['idle','checking','current','available','pr_candidate','downloading','ready_to_install','installed','installing','unavailable','failed'].includes(status.state) ? status.state : 'failed';
    const out = { state, currentVersion: String(status.current_version || (state === 'current' ? status.version : '')), availableVersion: String(status.available_version || (state === 'available' ? status.version : '')), progress: null, notes: String(status.release_notes || status.notes || ''), error: String(status.error || status.message || status.reason || '') };
    const p = Number(status.progress != null ? status.progress : (status.content_length > 0 ? 100 * status.downloaded / status.content_length : NaN));
    out.downloaded = Number(status.downloaded) || 0;
    out.contentLength = Number(status.content_length) || null;
    out.channel = String(status.channel || 'stable');
    out.installedVersion = String(status.installed_version || status.meta_version || status.meta?.version || '');
    if (state === 'pr_candidate') {
      const pr = Number(status.pr_number), run = Number(status.run_id), sha = String(status.source_sha || '');
      const expected = Number.isSafeInteger(run) && run > 0 ? 'https://github.com/iarnvidr/muninn/actions/runs/' + run : '';
      if (!Number.isSafeInteger(pr) || pr < 1 || !/^[0-9a-f]{40}$/.test(sha) || status.url !== expected) {
        return { state: 'unavailable', reason: 'PR build metadata could not be verified.' };
      }
      out.prNumber = pr; out.runId = run; out.sourceSha = sha; out.buildUrl = expected;
    }
    if (Number.isFinite(p)) out.progress = Math.max(0, Math.min(100, p));
    if (state === 'available' && (!out.availableVersion || out.availableVersion === 'undefined')) return { state: 'unavailable', reason: 'No release information is available.' };
    return out;
  }
  function view(status) {
    const s = normalize(status);
    const labels = { idle: 'Check for updates when you’re ready.', checking: 'Checking for updates…', current: 'You’re up to date.', available: 'An update is available.', pr_candidate: 'A tested PR build is available separately.', downloading: 'Downloading update…', ready_to_install: 'Update downloaded and verified. Ready to install.', installed: 'Update installed.', installing: 'Installing update…', unavailable: 'Update service is unavailable.', failed: 'Update failed.' };
    return Object.assign({}, s, { label: s.reason || s.error || labels[s.state], canCheck: !['checking','downloading','installing'].includes(s.state), canDownload: s.state === 'available', canOpenPrBuild: s.state === 'pr_candidate', canInstall: s.state === 'ready_to_install', canCancel: ['checking','downloading'].includes(s.state) });
  }
  const api = { normalize, view };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  root.MuninnUpdates = api;
})(typeof window !== 'undefined' ? window : globalThis);
