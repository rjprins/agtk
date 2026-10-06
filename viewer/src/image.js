// Only bytes supplied by the host can become an image. In particular SVG stays
// in an img element, where it cannot execute scripts or load external resources.
export function isImageDataUrl(value) {
  return typeof value === 'string'
    && /^data:image\/(?:png|jpeg|gif|webp|svg\+xml|bmp|x-icon|avif);base64,[A-Za-z0-9+/]*={0,2}$/.test(value);
}

export function createImageView(root) {
  const viewport = root.querySelector('#image-viewport');
  const fit = root.querySelector('#image-fit');
  const actual = root.querySelector('#image-actual');
  const status = root.querySelector('#image-status');
  let generation = 0;

  function setActualSize(value) {
    viewport.classList.toggle('actual-size', value);
    fit.setAttribute('aria-pressed', String(!value));
    actual.setAttribute('aria-pressed', String(value));
  }
  fit.addEventListener('click', () => setActualSize(false));
  actual.addEventListener('click', () => setActualSize(true));

  return {
    clear() {
      generation += 1;
      root.hidden = true;
      viewport.replaceChildren();
    },
    show({ dataUrl, path, viewState, onLoad, onError }) {
      const current = ++generation;
      root.hidden = false;
      viewport.replaceChildren();
      fit.disabled = actual.disabled = true;
      status.textContent = 'Loading image…';
      setActualSize(viewState?.actualSize === true);
      const image = document.createElement('img');
      image.alt = path;
      image.hidden = true;
      image.addEventListener('load', () => {
        if (current !== generation) return;
        // WebKit can report an SVG's fitted size as naturalWidth/naturalHeight
        // once it participates in layout. Capture its dimensions while hidden.
        const width = image.naturalWidth;
        const height = image.naturalHeight;
        image.width = width;
        image.height = height;
        image.hidden = false;
        fit.disabled = actual.disabled = false;
        status.textContent = `${width} × ${height} px`;
        viewport.scrollLeft = Number.isFinite(viewState?.left) ? viewState.left : 0;
        viewport.scrollTop = Number.isFinite(viewState?.top) ? viewState.top : 0;
        onLoad(width, height);
      });
      image.addEventListener('error', () => {
        if (current !== generation) return;
        const message = 'Could not display this image. The file may be damaged or use an unsupported format.';
        status.textContent = message;
        image.remove();
        onError(message);
      });
      // Do not insert SVG or any other file content as HTML.
      image.src = dataUrl;
      viewport.append(image);
      viewport.focus({ preventScroll: true });
    },
    saveViewState() {
      return {
        actualSize: viewport.classList.contains('actual-size'),
        left: viewport.scrollLeft,
        top: viewport.scrollTop,
      };
    },
  };
}
