import MarkdownIt from 'markdown-it';

// Raw HTML stays on because READMEs use it for layout. The page sanitizes the result.
const markdown = new MarkdownIt({ html: true, linkify: true });
markdown.core.ruler.push('agtk_task_lists', renderTaskLists);
markdown.core.ruler.push('agtk_source_lines', markSourceLines);

// Markdown text as HTML. Each block names its first source line in `data-source-line`.
export function renderMarkdown(text) {
  return markdown.render(text);
}

function markSourceLines(state) {
  for (const token of state.tokens) {
    if (token.map && token.nesting !== -1 && token.type !== 'inline') {
      token.attrSet('data-source-line', String(token.map[0] + 1));
    }
  }
}

// GitHub task list items: `- [ ] todo` and `- [x] done`.
function renderTaskLists(state) {
  const tokens = state.tokens;
  for (let index = 2; index < tokens.length; index += 1) {
    const inline = tokens[index];
    if (
      inline.type !== 'inline'
      || tokens[index - 1].type !== 'paragraph_open'
      || tokens[index - 2].type !== 'list_item_open'
    ) {
      continue;
    }
    const first = inline.children?.[0];
    const match = first?.type === 'text' ? /^\[([ xX])\](?: |$)/.exec(first.content) : null;
    if (!match) continue;
    first.content = first.content.slice(match[0].length);
    const checkbox = new state.Token('html_inline', '', 0);
    checkbox.content = `<input type="checkbox" disabled${match[1] === ' ' ? '' : ' checked'}> `;
    inline.children.unshift(checkbox);
    tokens[index - 2].attrJoin('class', 'task-list-item');
  }
}

// The anchor GitHub gives a heading, without the numeric suffix for repeats.
export function headingSlug(text) {
  return text
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{M}\p{N}\s_-]/gu, '')
    .replace(/\s/g, '-');
}
