const EXTENSION_LANGUAGE = new Map([
  ['.bash', 'shell'],
  ['.c', 'cpp'],
  ['.cc', 'cpp'],
  ['.cfg', 'ini'],
  ['.cjs', 'javascript'],
  ['.conf', 'ini'],
  ['.cpp', 'cpp'],
  ['.cs', 'csharp'],
  ['.csproj', 'xml'],
  ['.css', 'css'],
  ['.cts', 'typescript'],
  ['.cxx', 'cpp'],
  ['.env', 'ini'],
  ['.go', 'go'],
  ['.h', 'cpp'],
  ['.hpp', 'cpp'],
  ['.htm', 'html'],
  ['.html', 'html'],
  ['.hxx', 'cpp'],
  ['.ini', 'ini'],
  ['.java', 'java'],
  ['.js', 'javascript'],
  ['.json', 'json'],
  ['.json5', 'json'],
  ['.jsonc', 'json'],
  ['.jsx', 'javascript'],
  ['.md', 'markdown'],
  ['.mdx', 'mdx'],
  ['.mjs', 'javascript'],
  ['.mts', 'typescript'],
  ['.php', 'php'],
  ['.props', 'xml'],
  ['.py', 'python'],
  ['.pyi', 'python'],
  ['.rb', 'ruby'],
  ['.rs', 'rust'],
  ['.scss', 'scss'],
  ['.sh', 'shell'],
  ['.sql', 'sql'],
  ['.svg', 'xml'],
  ['.targets', 'xml'],
  ['.toml', 'ini'],
  ['.ts', 'typescript'],
  ['.tsx', 'typescript'],
  ['.xml', 'xml'],
  ['.xsl', 'xml'],
  ['.yaml', 'yaml'],
  ['.yml', 'yaml'],
  ['.zsh', 'shell'],
]);

// Lock files are TOML, which the INI grammar colors well enough.
const FILENAME_LANGUAGE = new Map([
  ['.bashrc', 'shell'],
  ['.profile', 'shell'],
  ['.zshenv', 'shell'],
  ['.zshrc', 'shell'],
  ['cargo.lock', 'ini'],
  ['containerfile', 'dockerfile'],
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
