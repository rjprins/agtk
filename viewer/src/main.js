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
// Go to definition, peek and find references, with Ctrl+click and the context menu.
import 'monaco-editor/editor/contrib/clipboard/browser/clipboard';
import 'monaco-editor/editor/contrib/contextmenu/browser/contextmenu';
import 'monaco-editor/editor/contrib/gotoSymbol/browser/goToCommands';
import 'monaco-editor/editor/contrib/gotoSymbol/browser/link/goToDefinitionAtPosition';
import 'monaco-editor/editor/contrib/hover/browser/hoverContribution';
import 'monaco-editor/editor/standalone/browser/referenceSearch/standaloneReferenceSearch';

import DOMPurify from 'dompurify';

import { languageForFence, languageForPath } from './language.js';
import { headingSlug, renderMarkdown } from './markdown.js';
import { createImageView, isImageDataUrl } from './image.js';
import { answerLocations, historyDirection } from './lookup.js';
import { commentEvent, commentSessions, createCommentBox, selectedLines } from './comment.js';
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
const markdownElement = document.querySelector('#markdown');
const markdownBodyElement = document.querySelector('#markdown-body');
const imageView = createImageView(document.querySelector('#image-view'));

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
// 'diff', 'file', 'image' or null while nothing is shown.
let mode = null;
let currentTabId = null;
let currentRequestId = null;
let activeChangeIndex = -1;
let findState = { query: '', index: -1 };
// 'source' shows the text in Monaco, 'preview' renders it as Markdown.
let presentation = 'source';
// The Markdown text that preview renders, and whether it is rendered yet.
let previewText = null;
let previewIsOriginal = false;
let previewRendered = false;
let renderGeneration = 0;
let lookupSequence = 0;
// Lookups waiting for agtk, by request ID.
const pendingLookups = new Map();
// Other files that answers pointed into, so peek can preview them.
const lookupModels = new Map();
// The agent sessions agtk offers for comments on the selection.
let commentTargets = { sessions: [], preferred: null };
let commentSequence = 0;
// The comment waiting for agtk's answer.
let pendingComment = null;
// What the comment box is about: a Monaco selection, or `{ text, range }` for
// words selected in the Markdown preview. The host is the editor or the preview.
let commentRange = null;
let commentHost = null;
// Whether a mouse button went down in an editor and is not up yet.
let selecting = false;

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
  offerComments(fileEditor);
  return fileEditor;
}

// The editor that currently shows text, for find, copy and view state.
function activeEditor() {
  if (mode === 'diff') return diffEditor.getModifiedEditor();
  if (mode === 'file') return fileEditor;
  return null;
}

// The shown text's model is named after its file when agtk sends the full path,
// so peek labels it like the other files it lists.
function shownModelUri(sourcePath, fallback) {
  return typeof sourcePath === 'string' && sourcePath.startsWith('/')
    ? monaco.Uri.from({ scheme: 'agtk', path: sourcePath, query: String(modelSequence) })
    : monaco.Uri.parse(fallback);
}

function clear() {
  hideCommentBox();
  commentBox.reset();
  for (const note of markdownElement.querySelectorAll('.comment-note')) note.remove();
  if (pendingComment) clearTimeout(pendingComment.timeout);
  pendingComment = null;
  imageView.clear();
  for (const resolve of pendingLookups.values()) resolve(null);
  pendingLookups.clear();
  for (const model of lookupModels.values()) model.dispose();
  lookupModels.clear();
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
  presentation = 'source';
  previewText = null;
  previewIsOriginal = false;
  previewRendered = false;
  renderGeneration += 1;
  markdownBodyElement.replaceChildren();
  fileNameElement.textContent = 'No file selected';
  versionLabelsElement.textContent = '';
  metadataElement.textContent = '';
  editorElement.classList.remove('visible');
  fileEditorElement.classList.remove('visible');
  markdownElement.classList.remove('visible');
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
  line,
  column,
  sourcePath,
  presentation: requestedPresentation,
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
    shownModelUri(sourcePath, `${uriRoot}/modified`),
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
  emptyStateElement.hidden = true;
  if (viewState) diffEditor.restoreViewState(viewState);
  // A deleted file has only its old text to preview.
  previewIsOriginal = modified.length === 0 && original.length > 0;
  previewText = previewIsOriginal ? original : modified;
  if (requestedPresentation === 'preview') {
    showPreview(viewState?.agtkPreviewScroll);
  } else {
    editorElement.classList.add('visible');
    diffEditor.getModifiedEditor().focus();
  }
  if (Number.isFinite(line) && line > 0) reveal(line, column);
}

function showFile({
  requestId,
  tabId,
  path,
  text,
  dataUrl,
  language,
  label,
  metadata,
  viewState,
  line,
  column,
  sourcePath,
  presentation: requestedPresentation,
}) {
  if (dataUrl !== undefined) {
    if (typeof tabId !== 'string' || typeof path !== 'string' || !isImageDataUrl(dataUrl)) {
      throw new TypeError('Image data must include an encoded image, a path, and a tab ID');
    }
    clear();
    mode = 'image';
    currentTabId = tabId;
    currentRequestId = typeof requestId === 'string' ? requestId : tabId;
    fileNameElement.textContent = path;
    versionLabelsElement.textContent = typeof label === 'string' ? label : '';
    metadataElement.textContent = Array.isArray(metadata) ? metadata.join(' · ') : '';
    emptyStateElement.hidden = true;
    imageView.show({
      dataUrl, path, viewState: viewState?.agtkImage,
      onLoad: (width, height) => sendEvent({
        type: 'imageRendered', requestId: currentRequestId, tabId, width, height,
      }),
      onError: (message) => sendEvent({ type: 'error', requestId: currentRequestId, message }),
    });
    return;
  }
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
    shownModelUri(sourcePath, `inmemory://agtk/file/${modelSequence}`),
  );

  mode = 'file';
  currentTabId = tabId;
  currentRequestId = typeof requestId === 'string' ? requestId : tabId;
  findState = { query: '', index: -1 };
  editor.setModel(model);
  fileNameElement.textContent = path;
  versionLabelsElement.textContent = typeof label === 'string' ? label : '';
  metadataElement.textContent = Array.isArray(metadata) ? metadata.join(' · ') : '';
  emptyStateElement.hidden = true;
  if (viewState) editor.restoreViewState(viewState);
  previewText = text;
  if (requestedPresentation === 'preview') {
    showPreview(viewState?.agtkPreviewScroll);
  } else {
    fileEditorElement.classList.add('visible');
    editor.focus();
  }
  if (Number.isFinite(line) && line > 0) reveal(line, column);
  sendEvent({
    type: 'fileRendered',
    requestId: currentRequestId,
    tabId: currentTabId,
    lineCount: model.getLineCount(),
  });
}

// Puts the cursor at a line and optional column and centers it.
function reveal(line, column) {
  if (presentation === 'preview') {
    scrollPreviewToLine(line);
    return;
  }
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
  let state = mode === 'image'
    ? { agtkImage: imageView.saveViewState() }
    : mode === 'diff' ? diffEditor.saveViewState() : fileEditor?.saveViewState();
  if (state && presentation === 'preview') {
    state = { ...state, agtkPreviewScroll: markdownElement.scrollTop };
  }
  sendEvent({ type: 'viewState', tabId: currentTabId, state: state ?? null });
  return state;
}

function moveToChange(next) {
  if (mode !== 'diff' || presentation === 'preview') return;
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
  if (presentation === 'preview') {
    // WebKit's find selects the match and scrolls to it.
    window.find(query, false, !next, true, false, false, false);
    return;
  }
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
  // The host copies a preview selection with WebKit's own copy command.
  if (presentation === 'preview') return;
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
  if (typeof fontFamily === 'string' && fontFamily.length > 0) {
    document.body.style.setProperty('--code-font-family', `"${fontFamily.replaceAll('"', '')}", monospace`);
  }
  if (Number.isFinite(fontSize) && fontSize >= 8 && fontSize <= 48) {
    document.body.style.setProperty('--code-font-size', `${Math.round(fontSize)}px`);
  }
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

// Markdown preview

// Switches between Monaco and the rendered Markdown, keeping roughly the same place.
function setPresentation(next) {
  if (mode === null || previewText === null) return;
  const wanted = next === 'preview' ? 'preview' : 'source';
  if (wanted === presentation) return;
  if (wanted === 'preview') {
    const line = previewEditor()?.getVisibleRanges()[0]?.startLineNumber;
    showPreview();
    if (line > 1) scrollPreviewToLine(line, 'start');
    return;
  }
  const line = firstVisiblePreviewLine();
  hideCommentBox();
  presentation = 'source';
  markdownElement.classList.remove('visible');
  (mode === 'diff' ? editorElement : fileEditorElement).classList.add('visible');
  const editor = previewEditor();
  if (editor) {
    editor.layout();
    if (line) editor.setScrollTop(editor.getTopForLineNumber(line));
    activeEditor()?.focus();
  }
}

// The Monaco editor whose lines the preview's source lines refer to.
function previewEditor() {
  if (mode === 'diff') {
    return previewIsOriginal ? diffEditor.getOriginalEditor() : diffEditor.getModifiedEditor();
  }
  return fileEditor;
}

function showPreview(scrollTop) {
  presentation = 'preview';
  hideCommentBox();
  editorElement.classList.remove('visible');
  fileEditorElement.classList.remove('visible');
  if (!previewRendered) renderPreview();
  markdownElement.classList.add('visible');
  markdownElement.scrollTop = Number.isFinite(scrollTop) ? scrollTop : 0;
  markdownElement.focus({ preventScroll: true });
}

function renderPreview() {
  renderGeneration += 1;
  const generation = renderGeneration;
  markdownBodyElement.innerHTML = DOMPurify.sanitize(renderMarkdown(previewText), {
    FORBID_TAGS: ['style', 'form'],
    FORBID_ATTR: ['style'],
  });
  previewRendered = true;

  const slugCounts = new Map();
  for (const heading of markdownBodyElement.querySelectorAll('h1, h2, h3, h4, h5, h6')) {
    const slug = headingSlug(heading.textContent);
    const count = slugCounts.get(slug) ?? 0;
    slugCounts.set(slug, count + 1);
    heading.dataset.anchor = count === 0 ? slug : `${slug}-${count}`;
  }
  for (const link of markdownBodyElement.querySelectorAll('a[href]')) {
    if (!link.title) link.title = link.getAttribute('href');
  }
  // Only embedded images can load: the page has no access to files or the network.
  for (const image of markdownBodyElement.querySelectorAll('img')) {
    if (image.getAttribute('src')?.startsWith('data:')) continue;
    const placeholder = document.createElement('span');
    placeholder.className = 'image-placeholder';
    placeholder.textContent = image.alt || image.getAttribute('src') || 'image';
    placeholder.title = image.getAttribute('src') || '';
    image.replaceWith(placeholder);
  }
  for (const code of markdownBodyElement.querySelectorAll('pre > code')) {
    const languageClass = [...code.classList].find((name) => name.startsWith('language-'));
    const language = languageClass ? languageForFence(languageClass.slice('language-'.length)) : null;
    if (!language) continue;
    monaco.editor
      .colorize(code.textContent.replace(/\n$/, ''), language, { tabSize: 4 })
      .then((html) => {
        if (generation === renderGeneration && code.isConnected) code.innerHTML = html;
      })
      .catch(() => {});
  }
}

// The block that starts at or before a source line.
function blockForLine(line) {
  let found = null;
  for (const element of markdownBodyElement.querySelectorAll('[data-source-line]')) {
    if (Number(element.dataset.sourceLine) > line) break;
    found = element;
  }
  return found;
}

function scrollPreviewToLine(line, position = 'center') {
  if (!Number.isFinite(line) || line < 1) return;
  const block = blockForLine(Math.trunc(line));
  if (block) block.scrollIntoView({ block: position });
  else markdownElement.scrollTop = 0;
}

function firstVisiblePreviewLine() {
  const top = markdownElement.getBoundingClientRect().top;
  for (const element of markdownBodyElement.querySelectorAll('[data-source-line]')) {
    if (element.getBoundingClientRect().bottom > top) return Number(element.dataset.sourceLine);
  }
  return null;
}

function scrollToAnchor(anchor) {
  const target = [...markdownBodyElement.querySelectorAll('[data-anchor]')]
    .find((heading) => heading.dataset.anchor === anchor.toLowerCase());
  target?.scrollIntoView({ block: 'start' });
}

markdownElement.addEventListener('click', (event) => {
  const link = event.target instanceof Element ? event.target.closest('a[href]') : null;
  if (!link || !markdownElement.contains(link)) return;
  event.preventDefault();
  const href = link.getAttribute('href');
  if (href.startsWith('#')) {
    let anchor = href.slice(1);
    try {
      anchor = decodeURIComponent(anchor);
    } catch {
      // Keep the raw fragment.
    }
    scrollToAnchor(anchor);
    return;
  }
  if (currentTabId && href.length <= 4096) {
    sendEvent({ type: 'openLink', tabId: currentTabId, href });
  }
});

// Go to definition and find references

// agtk has language servers for these. It checks the file type itself as well.
const LOOKUP_LANGUAGES = ['python', 'typescript', 'javascript'];

function showEditorMessage(editor, message, position) {
  editor.getContribution('editor.contrib.messageController')?.showMessage(message, position);
}

function lookupModel(path, text) {
  const uri = monaco.Uri.file(path);
  const model = lookupModels.get(path);
  if (model) {
    if (text !== null && model.getValue() !== text) model.setValue(text);
  } else if (text !== null) {
    lookupModels.set(path, monaco.editor.createModel(text, languageForPath(path) || 'plaintext', uri));
  }
  return uri;
}

function lookUp(lookup, model, position, token) {
  const editor = activeEditor();
  if (!editor || model !== editor.getModel() || !currentRequestId || presentation === 'preview') {
    return null;
  }
  lookupSequence += 1;
  const requestId = lookupSequence;
  const documentId = currentRequestId;
  const answered = new Promise((resolve) => {
    pendingLookups.set(requestId, resolve);
    token.onCancellationRequested(() => {
      pendingLookups.delete(requestId);
      resolve(null);
    });
  });
  sendEvent({
    type: 'lookUp', requestId, documentId, lookup, line: position.lineNumber, column: position.column,
  });
  // Holding Ctrl over a word looks up its definition too: only talk about lookups at the cursor.
  const atCursor = () => editor.getPosition()?.equals(position) ?? false;
  const waiting = setTimeout(() => {
    if (atCursor()) showEditorMessage(editor, 'Asking the language server…', position);
  }, 800);
  return answered.then((answer) => {
    clearTimeout(waiting);
    editor.getContribution('editor.contrib.messageController')?.closeMessage();
    if (!answer || documentId !== currentRequestId) return null;
    if (typeof answer.error === 'string') {
      // Replaces Monaco's own message that nothing was found, which shows first.
      if (atCursor()) setTimeout(() => showEditorMessage(editor, answer.error, position), 0);
      return null;
    }
    return answerLocations(answer, {
      shownUri: model.uri,
      fileUri: lookupModel,
      range: (...bounds) => new monaco.Range(...bounds),
    });
  });
}

monaco.languages.registerDefinitionProvider(LOOKUP_LANGUAGES, {
  provideDefinition: (model, position, token) => lookUp('definition', model, position, token),
});
monaco.languages.registerReferenceProvider(LOOKUP_LANGUAGES, {
  provideReferences: (model, position, _context, token) => lookUp('references', model, position, token),
});

// Monaco opens every definition and reference through here, in this file or another.
monaco.editor.registerEditorOpener({
  openCodeEditor(source, resource, selectionOrPosition) {
    const editor = activeEditor();
    const model = editor?.getModel();
    if (!model || source !== editor || !currentRequestId) return false;
    const target = selectionOrPosition && 'startLineNumber' in selectionOrPosition
      ? { lineNumber: selectionOrPosition.startLineNumber, column: selectionOrPosition.startColumn }
      : selectionOrPosition;
    const from = editor.getPosition();
    const jump = {
      type: 'jump',
      documentId: currentRequestId,
      line: target?.lineNumber || 1,
      column: target?.column || 1,
      fromLine: from?.lineNumber || 1,
      fromColumn: from?.column || 1,
    };
    if (resource.toString() === model.uri.toString()) {
      sendEvent(jump);
      // Monaco moves the cursor itself.
      return false;
    }
    if (resource.scheme !== 'file') return false;
    editor.getContribution('editor.contrib.referencesController')?.closeWidget(false);
    sendEvent({ ...jump, path: resource.path });
    return true;
  },
});

function navigateHistory(forward) {
  const position = activeEditor()?.getPosition();
  if (!position || !currentRequestId) return false;
  sendEvent({
    type: 'history',
    documentId: currentRequestId,
    forward,
    line: position.lineNumber,
    column: position.column,
  });
  return true;
}

window.addEventListener('keydown', (event) => {
  if (commentBox.element.contains(event.target)) return;
  const direction = historyDirection(event);
  if (direction && navigateHistory(direction === 'forward')) {
    event.preventDefault();
    event.stopPropagation();
  }
}, true);

// The mouse's back and forward buttons.
window.addEventListener('mouseup', (event) => {
  if ((event.button === 3 || event.button === 4) && navigateHistory(event.button === 4)) {
    event.preventDefault();
  }
});

function answerLookup(requestId, answer) {
  const resolve = pendingLookups.get(requestId);
  if (!resolve) return;
  pendingLookups.delete(requestId);
  resolve(answer && typeof answer === 'object' ? answer : null);
}

// Comments on the selection

const commentBox = createCommentBox({
  onSend: sendComment,
  onClose: () => {
    const host = commentHost;
    hideCommentBox();
    host?.focus({ preventScroll: true });
  },
});

const commentWidget = {
  getId: () => 'agtk.comment',
  getDomNode: () => commentBox.element,
  getPosition() {
    if (!commentRange || commentHost === markdownElement) return null;
    const { startLine, endLine } = selectedLines(commentRange);
    return {
      position: { lineNumber: endLine, column: startLine === endLine ? commentRange.startColumn : 1 },
      preference: [
        monaco.editor.ContentWidgetPositionPreference.BELOW,
        monaco.editor.ContentWidgetPositionPreference.ABOVE,
      ],
    };
  },
};

// Keeps the commented words marked in the preview once the focus moves to the box.
const previewHighlight = typeof Highlight === 'function' && CSS.highlights ? new Highlight() : null;
if (previewHighlight) CSS.highlights.set('agtk-comment', previewHighlight);

function hideCommentBox() {
  if (commentHost === markdownElement) {
    commentBox.element.remove();
    previewHighlight?.clear();
  } else {
    commentHost?.removeContentWidget(commentWidget);
  }
  commentHost = null;
  commentRange = null;
}

// Puts the box under the selection, or hides it when nothing is selected. While
// the mouse is still selecting it only hides, so it does not follow the drag.
function updateCommentBox(onlyHide = false) {
  if (commentBox.sending) return;
  if (presentation === 'preview') {
    updatePreviewCommentBox(onlyHide);
    return;
  }
  const editor = activeEditor();
  const selection = editor?.getSelection();
  if (!editor || !selection || selection.isEmpty() || commentTargets.sessions.length === 0) {
    hideCommentBox();
    return;
  }
  if (onlyHide) return;
  commentRange = selection;
  if (commentHost === editor) {
    editor.layoutContentWidget(commentWidget);
  } else {
    hideCommentBox();
    commentRange = selection;
    commentHost = editor;
    editor.addContentWidget(commentWidget);
  }
  const { startLine, endLine } = selectedLines(selection);
  commentBox.setPlaceholder(startLine === endLine
    ? `Comment on line ${startLine}…`
    : `Comment on lines ${startLine}–${endLine}…`);
}

// The preview sends the selected words without lines: they are what the comment is about.
function updatePreviewCommentBox(onlyHide) {
  // Focus in the box moves the page's selection into it.
  if (commentBox.hasFocus) return;
  const selection = window.getSelection();
  const range = selection.rangeCount > 0 ? selection.getRangeAt(0) : null;
  const text = range && !range.collapsed && markdownBodyElement.contains(range.commonAncestorContainer)
    ? selection.toString().trim()
    : '';
  // A deleted file's preview shows text that is no longer there.
  if (!text || previewIsOriginal || commentTargets.sessions.length === 0) {
    hideCommentBox();
    return;
  }
  if (onlyHide) return;
  if (commentHost !== markdownElement) {
    hideCommentBox();
    commentHost = markdownElement;
    markdownElement.append(commentBox.element);
  }
  commentRange = { text, range: range.cloneRange() };
  placeInPreview(commentBox.element, commentRange.range);
  previewHighlight?.clear();
  previewHighlight?.add(commentRange.range);
  commentBox.setPlaceholder('Comment on the selection…');
}

// Puts `element` under the end of `range`, within the preview's width.
function placeInPreview(element, range) {
  const rects = range.getClientRects();
  const last = rects[rects.length - 1] ?? range.getBoundingClientRect();
  const area = markdownElement.getBoundingClientRect();
  element.style.position = 'absolute';
  element.style.visibility = '';
  element.style.top = `${last.bottom - area.top + markdownElement.scrollTop}px`;
  const room = markdownElement.clientWidth - element.offsetWidth - 8;
  const left = Math.min(last.left - area.left + markdownElement.scrollLeft, room);
  element.style.left = `${Math.max(8, left)}px`;
}

function showPreviewNote(text, range) {
  const note = document.createElement('div');
  note.className = 'comment-note';
  note.setAttribute('role', 'status');
  note.textContent = text;
  markdownElement.append(note);
  placeInPreview(note, range);
  setTimeout(() => note.remove(), 2500);
}

function offerComments(editor) {
  editor.onMouseDown(() => {
    selecting = true;
  });
  editor.onDidChangeCursorSelection(() => {
    if (editor === activeEditor()) updateCommentBox(selecting);
  });
  // Typing over the selection starts the comment, since the text itself is read only.
  editor.onKeyDown((event) => {
    if (commentHost !== editor) return;
    const { key } = event.browserEvent;
    const typed = key.length === 1 && !event.ctrlKey && !event.altKey && !event.metaKey;
    if (!typed && key !== 'Enter') return;
    event.preventDefault();
    event.stopPropagation();
    commentBox.type(typed ? key : '');
  });
}

markdownElement.addEventListener('mousedown', () => {
  selecting = true;
});
document.addEventListener('selectionchange', () => {
  if (presentation === 'preview') updateCommentBox(selecting);
});
// Typing over selected words in the preview starts the comment too.
document.addEventListener('keydown', (event) => {
  if (commentHost !== markdownElement || commentBox.element.contains(event.target)) return;
  const typed = event.key.length === 1 && !event.ctrlKey && !event.altKey && !event.metaKey;
  if (!typed && event.key !== 'Enter') return;
  event.preventDefault();
  commentBox.type(typed ? event.key : '');
});

window.addEventListener('mouseup', () => {
  if (!selecting) return;
  selecting = false;
  updateCommentBox();
});

// Sends the comment, or returns why it cannot.
function sendComment(comment, sessionId) {
  const host = commentHost;
  if (!commentRange || !host || !currentRequestId) return 'Nothing is selected';
  let place;
  if (host === markdownElement) {
    place = { text: commentRange.text };
  } else {
    const model = host.getModel();
    if (!model) return 'Nothing is selected';
    place = {
      ...selectedLines(commentRange),
      text: model.getValueInRange(commentRange, monaco.editor.EndOfLinePreference.LF),
    };
  }
  commentSequence += 1;
  const commentId = commentSequence;
  const event = commentEvent({
    commentId, documentId: currentRequestId, sessionId, comment, ...place,
  });
  if (!('text' in event) && !('startLine' in event)) return 'The selection is too long to send';
  sendEvent(event);
  pendingComment = {
    commentId,
    sessionId,
    host,
    range: commentRange,
    timeout: setTimeout(() => answerComment(commentId, 'agtk did not answer'), 5000),
  };
  return '';
}

// agtk's answer to a comment: an empty error when the session got it.
function answerComment(commentId, error) {
  if (pendingComment?.commentId !== commentId) return;
  const { sessionId, host, range, timeout } = pendingComment;
  clearTimeout(timeout);
  pendingComment = null;
  const name = commentBox.sessionName(sessionId);
  commentBox.finish(typeof error === 'string' ? error : '');
  if (error) return;
  hideCommentBox();
  host.focus({ preventScroll: true });
  const message = name ? `Sent to ${name}` : 'Sent';
  if (host === markdownElement) showPreviewNote(message, range.range);
  else showEditorMessage(host, message, range.getEndPosition());
}

function setCommentSessions(value) {
  commentTargets = commentSessions(value);
  commentBox.setSessions(commentTargets.sessions, commentTargets.preferred);
  if (commentTargets.sessions.length === 0 && !commentBox.sending) hideCommentBox();
}

offerComments(diffEditor.getModifiedEditor());

window.agtkViewer = Object.freeze({
  answerComment,
  answerLookup,
  clear,
  copySelection,
  find,
  moveToChange,
  reveal,
  saveViewState: sendViewState,
  setAppearance,
  setCommentSessions,
  setPresentation,
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
} else if (demo === 'markdown') {
  showFile({
    tabId: 'demo-markdown',
    path: 'docs/guide.md',
    label: 'Working tree',
    presentation: 'preview',
    text: [
      '# Guide',
      '',
      'Read the [setup notes](setup.md#L3), see [Usage](#usage) or visit https://example.com.',
      '',
      '## Usage',
      '',
      '- [x] Install',
      '- [ ] Configure `agtk`',
      '',
      '| Key | Action |',
      '| --- | --- |',
      '| `Ctrl+F` | Find |',
      '',
      '> A quoted note.',
      '',
      '```rust',
      'fn main() {',
      '    println!("hello");',
      '}',
      '```',
      '',
      '![Screenshot](screenshot.png)',
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
