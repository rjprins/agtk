import * as monaco from 'monaco-editor/editor/editor.api';
import EditorWorker from 'monaco-editor/editor/editor.worker?worker&inline';
import JsonWorker from 'monaco-editor/language/json/json.worker?worker&inline';

import 'monaco-editor/languages/definitions/cpp/register';
import 'monaco-editor/languages/definitions/csharp/register';
import 'monaco-editor/languages/definitions/css/register';
import 'monaco-editor/languages/definitions/dockerfile/register';
import 'monaco-editor/languages/definitions/go/register';
import 'monaco-editor/languages/definitions/html/register';
import 'monaco-editor/languages/definitions/ini/register';
import 'monaco-editor/languages/definitions/java/register';
import 'monaco-editor/languages/definitions/javascript/register';
import 'monaco-editor/languages/definitions/markdown/register';
import 'monaco-editor/languages/definitions/mdx/register';
import 'monaco-editor/languages/definitions/php/register';
import 'monaco-editor/languages/definitions/python/register';
import 'monaco-editor/languages/definitions/ruby/register';
import 'monaco-editor/languages/definitions/rust/register';
import 'monaco-editor/languages/definitions/scss/register';
import 'monaco-editor/languages/definitions/shell/register';
import 'monaco-editor/languages/definitions/sql/register';
import 'monaco-editor/languages/definitions/typescript/register';
import 'monaco-editor/languages/definitions/xml/register';
import 'monaco-editor/languages/definitions/yaml/register';
import 'monaco-editor/languages/features/json/register';

import { languageForPath } from './language.js';
import './style.css';

self.MonacoEnvironment = {
  getWorker(_moduleId, label) {
    switch (label) {
      case 'json':
        return new JsonWorker();
      default:
        return new EditorWorker();
    }
  },
};

const editorElement = document.querySelector('#editor');
const fileNameElement = document.querySelector('#file-name');
const versionLabelsElement = document.querySelector('#version-labels');
const emptyStateElement = document.querySelector('#empty-state');

const diffEditor = monaco.editor.createDiffEditor(editorElement, {
  automaticLayout: true,
  diffAlgorithm: 'advanced',
  hideUnchangedRegions: { enabled: false },
  ignoreTrimWhitespace: false,
  lineNumbers: 'on',
  minimap: { enabled: false },
  originalEditable: false,
  overviewRulerLanes: 3,
  readOnly: true,
  renderIndicators: true,
  renderMarginRevertIcon: false,
  renderOverviewRuler: true,
  renderSideBySide: false,
  scrollBeyondLastLine: false,
  wordWrap: 'off',
});

let modelSequence = 0;
let currentTabId = null;
let currentRequestId = null;

function sendEvent(event) {
  window.webkit?.messageHandlers?.agmux?.postMessage(event);
}

function clearDiff() {
  const models = diffEditor.getModel();
  if (models) {
    diffEditor.setModel(null);
    models.original.dispose();
    models.modified.dispose();
  }

  currentTabId = null;
  currentRequestId = null;
  fileNameElement.textContent = 'No file selected';
  versionLabelsElement.textContent = '';
  editorElement.classList.remove('visible');
  emptyStateElement.hidden = false;
}

function showDiff({
  requestId,
  tabId,
  path,
  original,
  modified,
  originalLabel,
  modifiedLabel,
  language,
  viewState,
}) {
  if (
    typeof tabId !== 'string'
    || typeof path !== 'string'
    || typeof original !== 'string'
    || typeof modified !== 'string'
  ) {
    throw new TypeError('Diff data must include text versions, a path, and a tab ID');
  }

  clearDiff();
  modelSequence += 1;
  const uriRoot = `inmemory://agmux/diff/${modelSequence}`;
  const selectedLanguage = language || languageForPath(path) || 'plaintext';
  const originalModel = monaco.editor.createModel(
    original,
    selectedLanguage,
    monaco.Uri.parse(`${uriRoot}/original`),
  );
  const modifiedModel = monaco.editor.createModel(
    modified,
    selectedLanguage,
    monaco.Uri.parse(`${uriRoot}/modified`),
  );

  currentTabId = tabId;
  currentRequestId = typeof requestId === 'string' ? requestId : tabId;
  diffEditor.setModel({ original: originalModel, modified: modifiedModel });
  fileNameElement.textContent = path;
  versionLabelsElement.textContent = `${originalLabel || 'Original'} → ${modifiedLabel || 'Current'}`;
  editorElement.classList.add('visible');
  emptyStateElement.hidden = true;
  if (viewState) diffEditor.restoreViewState(viewState);
  diffEditor.getModifiedEditor().focus();
}

function sendViewState() {
  if (!currentTabId) return null;
  const state = diffEditor.saveViewState();
  sendEvent({ type: 'viewState', tabId: currentTabId, state });
  return state;
}

function setAppearance(theme, fontFamily, fontSize) {
  const isDark = theme === 'dark';
  document.body.dataset.theme = isDark ? 'dark' : 'light';
  monaco.editor.setTheme(isDark ? 'vs-dark' : 'vs');
  if (typeof fontFamily === 'string' && fontFamily.length > 0) {
    diffEditor.updateOptions({ fontFamily });
  }
  if (Number.isFinite(fontSize) && fontSize >= 8 && fontSize <= 48) {
    diffEditor.updateOptions({ fontSize: Math.round(fontSize) });
  }
}

window.agmuxDiffViewer = Object.freeze({
  clearDiff,
  saveViewState: sendViewState,
  setAppearance,
  showDiff,
});

diffEditor.onDidUpdateDiff(() => {
  if (!currentTabId) return;
  const lineChanges = diffEditor.getLineChanges() || [];
  const characterChanges = lineChanges.reduce(
    (count, change) => count + (change.charChanges?.length || 0),
    0,
  );
  sendEvent({
    type: 'diffRendered',
    requestId: currentRequestId,
    tabId: currentTabId,
    lineChanges: lineChanges.length,
    characterChanges,
  });
});

sendEvent({ type: 'ready', version: '0.1.0' });

if (new URLSearchParams(window.location.search).get('demo') === '1') {
  showDiff({
    tabId: 'demo-python-change',
    path: 'restly/api/views.py',
    originalLabel: 'main',
    modifiedLabel: 'Working tree',
    original: [
      'class InvoiceView(fr.AsyncRestView):',
      '    async def authorize(self, action, obj=None, data=None):',
      '        user = self.request.user  # populated by the auth middleware',
      '        if action in ("create", "update", "delete") and not user.is_staff:',
      '            raise fr.Forbidden()',
    ].join('\n'),
    modified: [
      'class InvoiceView(fr.AsyncRestView):',
      '    async def authorize(self, action, obj=None, data=None):',
      '        user: Annotated[User, Depends(get_current_user)]',
      '        if action in ("create", "update", "delete") and not self.current_user.is_staff:',
      '            raise fr.Forbidden()',
    ].join('\n'),
  });
}
