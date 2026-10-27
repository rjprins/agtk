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

// The box's elements. `onSend(comment, sessionId)` sends; `onClose()` gives focus back.
export function createCommentBox({ onSend, onClose }) {
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
    if (!onSend(comment, session.value)) finish('Nothing is selected');
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
  element.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' && event.target === input && !event.shiftKey && !event.isComposing) {
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
  // Clicks here are not the editor's either, so they keep its selection.
  element.addEventListener('mousedown', (event) => event.stopPropagation());

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
