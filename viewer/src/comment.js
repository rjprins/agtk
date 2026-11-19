// A box under selected text that sends the text with a comment to an agent session.

// Page events must stay under agtk's 32 KiB limit.
const MAX_EVENT_BYTES = 30 * 1024;
export const MAX_COMMENT_LENGTH = 4000;

// The first of `wanted` that is still offered, else the first session.
export function chooseSession(sessions, wanted) {
  const ids = sessions.map((session) => session.id);
  return wanted.find((id) => ids.includes(id)) ?? ids[0] ?? null;
}

// Keeps only well-formed sessions from agtk's list.
export function commentSessions(value) {
  const sessions = Array.isArray(value?.sessions) ? value.sessions : [];
  return {
    sessions: sessions.filter((session) => typeof session?.id === 'string'
      && session.id.length > 0 && typeof session.name === 'string'),
    preferred: typeof value?.preferred === 'string' ? value.preferred : null,
  };
}

// The lines a selection covers. A selection that ends at the start of a line
// does not cover that line.
export function selectedLines({ startLineNumber, endLineNumber, endColumn }) {
  const end = endColumn === 1 && endLineNumber > startLineNumber ? endLineNumber - 1 : endLineNumber;
  return { startLine: startLineNumber, endLine: end };
}

// The comment event, without the selected text when it would be too large to send.
export function commentEvent(fields) {
  const event = { type: 'comment', ...fields };
  if (new TextEncoder().encode(JSON.stringify(event)).length <= MAX_EVENT_BYTES) return event;
  const { text: _text, ...withoutText } = event;
  return withoutText;
}

// The Emacs motion or edit a key asks for in the comment, as GTK's Emacs key theme
// has them, or null.
export function emacsCommand({ key, ctrlKey, altKey, metaKey, shiftKey }) {
  if (metaKey || shiftKey || ctrlKey === altKey) return null;
  const keys = ctrlKey
    ? { a: 'lineStart', e: 'lineEnd', b: 'back', f: 'forward', p: 'up', n: 'down',
      d: 'deleteForward', h: 'deleteBack', k: 'killLine' }
    : { b: 'wordBack', f: 'wordForward', d: 'killWord', Backspace: 'killWordBack' };
  return keys[key.length === 1 ? key.toLowerCase() : key] ?? null;
}

function wordEnd(value, at) {
  while (at < value.length && !/\w/.test(value[at])) at += 1;
  while (at < value.length && /\w/.test(value[at])) at += 1;
  return at;
}

function wordStart(value, at) {
  while (at > 0 && !/\w/.test(value[at - 1])) at -= 1;
  while (at > 0 && /\w/.test(value[at - 1])) at -= 1;
  return at;
}

// Where the cursor goes, as `{ cursor }`, or the text to delete, as `{ from, to }`,
// for `command` with the cursor at `at` in `value`.
export function applyEmacs(command, value, at) {
  const lineStart = value.lastIndexOf('\n', at - 1) + 1;
  const newline = value.indexOf('\n', at);
  const lineEnd = newline === -1 ? value.length : newline;
  const column = at - lineStart;
  switch (command) {
    case 'lineStart': return { cursor: lineStart };
    case 'lineEnd': return { cursor: lineEnd };
    case 'back': return { cursor: Math.max(0, at - 1) };
    case 'forward': return { cursor: Math.min(value.length, at + 1) };
    case 'wordBack': return { cursor: wordStart(value, at) };
    case 'wordForward': return { cursor: wordEnd(value, at) };
    case 'up': {
      if (lineStart === 0) return { cursor: 0 };
      const previousStart = value.lastIndexOf('\n', lineStart - 2) + 1;
      return { cursor: Math.min(previousStart + column, lineStart - 1) };
    }
    case 'down': {
      if (newline === -1) return { cursor: value.length };
      const nextEnd = value.indexOf('\n', newline + 1);
      return { cursor: Math.min(newline + 1 + column, nextEnd === -1 ? value.length : nextEnd) };
    }
    case 'deleteForward': return { from: at, to: Math.min(value.length, at + 1) };
    case 'deleteBack': return { from: Math.max(0, at - 1), to: at };
    // At the end of a line, joins the next line, as Emacs does.
    case 'killLine': return { from: at, to: at === lineEnd ? Math.min(value.length, at + 1) : lineEnd };
    case 'killWord': return { from: at, to: wordEnd(value, at) };
    case 'killWordBack': return { from: wordStart(value, at), to: at };
    default: return null;
  }
}

// The box's elements. `onSend(comment, sessionId)` sends, or returns why it cannot.
// `onClose()` gives focus back. `onFocus(focused)` tells when the box gains or loses focus.
export function createCommentBox({ onSend, onClose, onFocus = () => {} }) {
  const element = document.createElement('form');
  element.className = 'comment-box';
  element.setAttribute('aria-label', 'Comment on the selection');
  const input = document.createElement('textarea');
  input.rows = 1;
  input.maxLength = MAX_COMMENT_LENGTH;
  input.setAttribute('aria-label', 'Comment');
  const footer = document.createElement('div');
  footer.className = 'comment-box-footer';
  const session = document.createElement('select');
  session.setAttribute('aria-label', 'Session');
  session.title = 'The agent session that gets the comment';
  const status = document.createElement('span');
  status.className = 'comment-box-status';
  status.setAttribute('role', 'status');
  const send = document.createElement('button');
  send.type = 'submit';
  send.textContent = 'Send';
  footer.append(session, status, send);
  element.append(input, footer);

  let lastUsed = null;
  let sending = false;

  // Grows the comment with its lines. Detached, it has no height to measure.
  function fit() {
    input.style.height = 'auto';
    if (!input.isConnected || !input.value) return;
    const borders = input.offsetHeight - input.clientHeight;
    input.style.height = `${input.scrollHeight + borders}px`;
  }

  function submit() {
    const comment = input.value.trim();
    if (sending || comment.length === 0 || !session.value) return;
    sending = true;
    input.readOnly = true;
    send.disabled = true;
    status.textContent = 'Sending…';
    const error = onSend(comment, session.value);
    if (error) finish(error);
  }

  // Ends sending: clears the comment when it went out, or shows why not.
  function finish(error) {
    sending = false;
    input.readOnly = false;
    send.disabled = false;
    status.textContent = error || '';
    if (error) return;
    lastUsed = session.value;
    input.value = '';
    fit();
  }

  element.addEventListener('submit', (event) => {
    event.preventDefault();
    submit();
  });
  input.addEventListener('input', () => {
    status.textContent = '';
    fit();
  });
  // Emacs keys in the comment. A selection collapses first, as GTK does.
  function emacsKey(event) {
    const command = event.isComposing ? null : emacsCommand(event);
    if (!command) return false;
    event.preventDefault();
    if (input.readOnly && !['lineStart', 'lineEnd', 'back', 'forward', 'wordBack',
      'wordForward', 'up', 'down'].includes(command)) return true;
    const at = input.selectionDirection === 'backward' ? input.selectionStart : input.selectionEnd;
    const change = applyEmacs(command, input.value, at);
    if (!change) return true;
    if ('cursor' in change) {
      input.setSelectionRange(change.cursor, change.cursor);
    } else if (change.to > change.from) {
      input.setRangeText('', change.from, change.to, 'start');
      input.dispatchEvent(new Event('input'));
    }
    return true;
  }

  element.addEventListener('focusin', () => onFocus(true));
  element.addEventListener('focusout', (event) => {
    if (!element.contains(event.relatedTarget)) onFocus(false);
  });
  element.addEventListener('keydown', (event) => {
    if (event.target === input && emacsKey(event)) {
      // Handled.
    } else if (event.key === 'Enter' && event.target === input && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      submit();
    } else if (event.key === 'Escape') {
      event.preventDefault();
      input.value = '';
      fit();
      onClose();
    }
    // Keys typed here are not the editor's.
    event.stopPropagation();
  });
  // Clicks here are not the editor's either, so they keep its selection. Only the
  // comment and the session list take focus, so the selected words stay selected.
  element.addEventListener('mousedown', (event) => {
    event.stopPropagation();
    if (event.target !== input && event.target !== session) event.preventDefault();
  });

  return {
    element,
    get sending() {
      return sending;
    },
    get hasFocus() {
      return element.contains(document.activeElement);
    },
    setSessions(sessions, preferred) {
      // The choice stays while it is offered, and the last session used comes next.
      const chosen = chooseSession(sessions, [session.value, lastUsed, preferred]);
      session.replaceChildren(...sessions.map(({ id, name }) => {
        const option = document.createElement('option');
        option.value = id;
        option.textContent = name;
        return option;
      }));
      if (chosen) session.value = chosen;
    },
    sessionName(id) {
      return [...session.options].find((option) => option.value === id)?.textContent ?? '';
    },
    setPlaceholder(text) {
      input.placeholder = text;
    },
    // Puts the focus in the comment, adding `text` as if typed there.
    type(text) {
      input.focus();
      if (text) {
        input.setRangeText(text, input.selectionStart, input.selectionEnd, 'end');
        input.dispatchEvent(new Event('input'));
      }
    },
    finish,
    // Drops the comment and any sending, for a new document.
    reset() {
      sending = false;
      input.readOnly = false;
      send.disabled = false;
      status.textContent = '';
      input.value = '';
      fit();
    },
  };
}
