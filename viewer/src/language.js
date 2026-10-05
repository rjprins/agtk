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

const KNOWN_LANGUAGES = new Set([...EXTENSION_LANGUAGE.values(), ...FILENAME_LANGUAGE.values()]);

const FENCE_LANGUAGE = new Map([
  ['bash', 'shell'],
  ['c++', 'cpp'],
  ['console', 'shell'],
  ['docker', 'dockerfile'],
  ['golang', 'go'],
  ['shell-session', 'shell'],
  ['zsh', 'shell'],
]);

// The info string of a code fence, like `rust` or `rs`, as a Monaco language ID.
export function languageForFence(info) {
  if (typeof info !== 'string') return null;
  const name = info.trim().split(/\s+/)[0].toLowerCase();
  if (name.length === 0) return null;
  if (KNOWN_LANGUAGES.has(name)) return name;
  return FENCE_LANGUAGE.get(name) ?? EXTENSION_LANGUAGE.get(`.${name}`) ?? null;
}
