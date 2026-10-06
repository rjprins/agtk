import createDOMPurify from 'dompurify';

// Keep Monaco's temporary hooks separate from the Markdown preview sanitizer.
export default createDOMPurify();
