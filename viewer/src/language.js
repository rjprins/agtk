const EXTENSION_LANGUAGE = new Map([
  ['.c', 'cpp'],
  ['.cc', 'cpp'],
  ['.cpp', 'cpp'],
  ['.cs', 'csharp'],
  ['.css', 'css'],
  ['.go', 'go'],
  ['.h', 'cpp'],
  ['.html', 'html'],
  ['.ini', 'ini'],
  ['.java', 'java'],
  ['.js', 'javascript'],
  ['.jsx', 'javascript'],
  ['.json', 'json'],
  ['.md', 'markdown'],
  ['.mdx', 'mdx'],
  ['.php', 'php'],
  ['.py', 'python'],
  ['.rb', 'ruby'],
  ['.rs', 'rust'],
  ['.scss', 'scss'],
  ['.sh', 'shell'],
  ['.sql', 'sql'],
  ['.svg', 'xml'],
  ['.ts', 'typescript'],
  ['.tsx', 'typescript'],
  ['.xml', 'xml'],
  ['.yaml', 'yaml'],
  ['.yml', 'yaml'],
  ['.zsh', 'shell'],
]);

const FILENAME_LANGUAGE = new Map([
  ['.bashrc', 'shell'],
  ['.zshrc', 'shell'],
  ['dockerfile', 'dockerfile'],
]);

export function languageForPath(path) {
  if (typeof path !== 'string' || path.length === 0) return null;

  const filename = path.split(/[\\/]/).at(-1).toLowerCase();
  const knownFilename = FILENAME_LANGUAGE.get(filename);
  if (knownFilename) return knownFilename;

  const dot = filename.lastIndexOf('.');
  return dot < 0 ? null : (EXTENSION_LANGUAGE.get(filename.slice(dot)) ?? null);
}
