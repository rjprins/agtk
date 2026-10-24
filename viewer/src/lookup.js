// Helpers for go to definition and find references, which agtk answers.

function isPosition(value) {
  return Number.isInteger(value) && value > 0;
}

// Turns agtk's answer into Monaco locations. Locations in the shown text use its
// model; the others get `fileUri(path, text)`, which may create a preview model.
export function answerLocations(answer, { shownUri, fileUri, range }) {
  if (!answer || !Array.isArray(answer.locations)) return [];
  const files = answer.files && typeof answer.files === 'object' ? answer.files : {};
  return answer.locations
    .filter((location) => location
      && (location.inShownText || typeof location.path === 'string')
      && [location.startLine, location.startColumn, location.endLine, location.endColumn]
        .every(isPosition))
    .map((location) => ({
      uri: location.inShownText
        ? shownUri
        : fileUri(location.path, typeof files[location.path] === 'string' ? files[location.path] : null),
      range: range(location.startLine, location.startColumn, location.endLine, location.endColumn),
    }));
}

// Alt+Left and Alt+Right, or the keyboard's own back and forward keys.
export function historyDirection(event) {
  if (event.key === 'BrowserBack') return 'back';
  if (event.key === 'BrowserForward') return 'forward';
  if (!event.altKey || event.ctrlKey || event.shiftKey || event.metaKey) return null;
  if (event.key === 'ArrowLeft') return 'back';
  if (event.key === 'ArrowRight') return 'forward';
  return null;
}
