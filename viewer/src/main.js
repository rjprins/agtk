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
// The diff markers use the icon font. Without this it only loads with the JSON chunk.
import 'monaco-editor/features/codicon/register';

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
const fileEditorElement = document.querySelector('#file-editor');
const fileNameElement = document.querySelector('#file-name');
const versionLabelsElement = document.querySelector('#version-labels');
const metadataElement = document.querySelector('#metadata');
const emptyStateElement = document.querySelector('#empty-state');

const sharedOptions = {
  automaticLayout: true,
  lineNumbers: 'on',
  minimap: { enabled: false },
  readOnly: true,
  scrollBeyondLastLine: false,
  wordWrap: 'off',
};

const diffEditor = monaco.editor.createDiffEditor(editorElement, {
  ...sharedOptions,
  diffAlgorithm: 'advanced',
  hideUnchangedRegions: { enabled: false },
  ignoreTrimWhitespace: false,
  originalEditable: false,
  overviewRulerLanes: 3,
  renderIndicators: true,
  renderMarginRevertIcon: false,
  renderOverviewRuler: true,
  renderSideBySide: false,
});

// The plain editor for whole files is created on first use.
let fileEditor = null;
let appearance = { theme: 'vs', fontFamily: null, fontSize: null };

let modelSequence = 0;
// 'diff', 'file' or null while nothing is shown.
let mode = null;
let currentTabId = null;
let currentRequestId = null;
let activeChangeIndex = -1;
let findState = { query: '', index: -1 };

function sendEvent(event) {
  window.webkit?.messageHandlers?.agtk?.postMessage(event);
}

function ensureFileEditor() {
  if (fileEditor) return fileEditor;
  fileEditor = monaco.editor.create(fileEditorElement, {
    ...sharedOptions,
    renderLineHighlight: 'line',
    occurrencesHighlight: 'singleFile',
  });
  applyAppearance(fileEditor);
  return fileEditor;
}

// The editor that currently shows text, for find, copy and view state.
function activeEditor() {
  if (mode === 'diff') return diffEditor.getModifiedEditor();
  if (mode === 'file') return fileEditor;
  return null;
}

function clear() {
  const models = diffEditor.getModel();
  if (models) {
    diffEditor.setModel(null);
    models.original.dispose();
    models.modified.dispose();
  }
  if (fileEditor) {
    const model = fileEditor.getModel();
    fileEditor.setModel(null);
    model?.dispose();
  }

  mode = null;
  currentTabId = null;
  currentRequestId = null;
  activeChangeIndex = -1;
  findState = { query: '', index: -1 };
  fileNameElement.textContent = 'No file selected';
  versionLabelsElement.textContent = '';
  metadataElement.textContent = '';
  editorElement.classList.remove('visible');
  fileEditorElement.classList.remove('visible');
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
  metadata,
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

  clear();
  modelSequence += 1;
  const uriRoot = `inmemory://agtk/diff/${modelSequence}`;
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

  mode = 'diff';
  currentTabId = tabId;
  currentRequestId = typeof requestId === 'string' ? requestId : tabId;
  activeChangeIndex = -1;
  findState = { query: '', index: -1 };
  diffEditor.setModel({ original: originalModel, modified: modifiedModel });
  fileNameElement.textContent = path;
  versionLabelsElement.textContent = `${originalLabel || 'Original'} → ${modifiedLabel || 'Current'}`;
  metadataElement.textContent = Array.isArray(metadata) ? metadata.join(' · ') : '';
  editorElement.classList.add('visible');
  emptyStateElement.hidden = true;
  if (viewState) diffEditor.restoreViewState(viewState);
  diffEditor.getModifiedEditor().focus();
}

function showFile({
  requestId,
  tabId,
  path,
  text,
  language,
  label,
  metadata,
  viewState,
  line,
  column,
}) {
  if (typeof tabId !== 'string' || typeof path !== 'string' || typeof text !== 'string') {
    throw new TypeError('File data must include text, a path, and a tab ID');
  }

  clear();
  const editor = ensureFileEditor();
  modelSequence += 1;
  const selectedLanguage = language || languageForPath(path) || 'plaintext';
  const model = monaco.editor.createModel(
    text,
    selectedLanguage,
    monaco.Uri.parse(`inmemory://agtk/file/${modelSequence}`),
  );

  mode = 'file';
  currentTabId = tabId;
  currentRequestId = typeof requestId === 'string' ? requestId : tabId;
  findState = { query: '', index: -1 };
  editor.setModel(model);
  fileNameElement.textContent = path;
  versionLabelsElement.textContent = typeof label === 'string' ? label : '';
  metadataElement.textContent = Array.isArray(metadata) ? metadata.join(' · ') : '';
  fileEditorElement.classList.add('visible');
  emptyStateElement.hidden = true;
  if (viewState) editor.restoreViewState(viewState);
  if (Number.isFinite(line) && line > 0) reveal(line, column);
  editor.focus();
  sendEvent({
    type: 'fileRendered',
    requestId: currentRequestId,
    tabId: currentTabId,
    lineCount: model.getLineCount(),
  });
}

// Puts the cursor at a line and optional column and centers it.
function reveal(line, column) {
  const editor = activeEditor();
  const model = editor?.getModel();
  if (!editor || !model || !Number.isFinite(line) || line < 1) return;
  const lineNumber = Math.min(Math.trunc(line), model.getLineCount());
  const maxColumn = model.getLineMaxColumn(lineNumber);
  const columnNumber = Number.isFinite(column) && column > 0
    ? Math.min(Math.trunc(column), maxColumn)
    : 1;
  editor.setPosition({ lineNumber, column: columnNumber });
  editor.revealLineInCenter(lineNumber);
  editor.focus();
}

function sendViewState() {
  if (!currentTabId) return null;
  const state = mode === 'diff' ? diffEditor.saveViewState() : fileEditor?.saveViewState();
  sendEvent({ type: 'viewState', tabId: currentTabId, state: state ?? null });
  return state;
}

function moveToChange(next) {
  if (mode !== 'diff') return;
  const changes = diffEditor.getLineChanges() || [];
  if (changes.length === 0) return;

  activeChangeIndex = next
    ? (activeChangeIndex + 1) % changes.length
    : (activeChangeIndex - 1 + changes.length) % changes.length;
  const change = changes[activeChangeIndex];
  const line = Math.max(
    1,
    change.modifiedStartLineNumber || change.modifiedEndLineNumber || 1,
  );
  const editor = diffEditor.getModifiedEditor();
  editor.revealLineInCenter(line);
  editor.setPosition({ lineNumber: line, column: 1 });
  editor.focus();
}

function find(query, next) {
  if (typeof query !== 'string' || query.length === 0) return;
  const editor = activeEditor();
  const model = editor?.getModel();
  if (!editor || !model) return;

  const matches = model.findMatches(query, false, false, false, null, false, 10000);
  if (matches.length === 0) return;
  const sameSearch = query === findState.query;
  const currentLine = editor.getPosition()?.lineNumber || 1;
  if (!sameSearch) {
    const position = matches.findIndex((match) => match.range.startLineNumber >= currentLine);
    findState = {
      query,
      index: next
        ? (position < 0 ? 0 : position)
        : (position < 0 ? matches.length - 1 : (position - 1 + matches.length) % matches.length),
    };
  } else {
    findState.index = next
      ? (findState.index + 1) % matches.length
      : (findState.index - 1 + matches.length) % matches.length;
  }

  const range = matches[findState.index].range;
  editor.setSelection(range);
  editor.revealRangeInCenter(range);
  editor.focus();
}

function copySelection() {
  activeEditor()?.getAction('editor.action.clipboardCopyAction')?.run();
}

function applyAppearance(editor) {
  const options = {};
  if (appearance.fontFamily) options.fontFamily = appearance.fontFamily;
  if (appearance.fontSize) options.fontSize = appearance.fontSize;
  editor.updateOptions(options);
}

function setAppearance(theme, fontFamily, fontSize) {
  const isDark = theme === 'dark';
  document.body.dataset.theme = isDark ? 'dark' : 'light';
  appearance.theme = isDark ? 'vs-dark' : 'vs';
  monaco.editor.setTheme(appearance.theme);
  if (typeof fontFamily === 'string' && fontFamily.length > 0) {
    appearance.fontFamily = fontFamily;
  }
  if (Number.isFinite(fontSize) && fontSize >= 8 && fontSize <= 48) {
    appearance.fontSize = Math.round(fontSize);
  }
  applyAppearance(diffEditor);
  if (fileEditor) applyAppearance(fileEditor);
}

window.agtkViewer = Object.freeze({
  clear,
  copySelection,
  find,
  moveToChange,
  reveal,
  saveViewState: sendViewState,
  setAppearance,
  showDiff,
  showFile,
});

diffEditor.onDidUpdateDiff(() => {
  if (mode !== 'diff' || !currentTabId) return;
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

sendEvent({ type: 'ready', version: '0.2.0' });

const demo = new URLSearchParams(window.location.search).get('demo');
if (demo === '1') {
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
} else if (demo === 'file') {
  showFile({
    tabId: 'demo-file',
    path: 'restly/api/views.py',
    label: 'Working tree',
    text: [
      'class InvoiceView(fr.AsyncRestView):',
      '    async def authorize(self, action, obj=None, data=None):',
      '        user: Annotated[User, Depends(get_current_user)]',
      '        if action in ("create", "update", "delete") and not self.current_user.is_staff:',
      '            raise fr.Forbidden()',
    ].join('\n'),
    line: 3,
    column: 9,
  });
}
