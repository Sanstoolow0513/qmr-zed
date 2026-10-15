import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const catalogs = ['en', 'zh-CN'].map(locale => JSON.parse(fs.readFileSync(
  path.join(root, 'crates/i18n/locales', `${locale}.json`), 'utf8',
)));
const [english, chinese] = catalogs;
const missing = [];
const verbatim = new Set([
  'const value = 42;', 'fn main() {}', 'git commit --no-verify',
  'zed', 'zed-cloud, zed, edit-prediction-bench, zed.dev', '~/projects/my-project',
  'zed/crates/ui/src/components/label/truncate/label/label.rs',
  'ssh user@example -p 2222', 'zed://...',
]);

function files(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const filename = path.join(directory, entry.name);
    return entry.isDirectory() ? files(filename) : [filename];
  });
}

function decode(literal) {
  return JSON.parse(literal.replace(/\\\r?\n\s*/g, '').replace(/\\u\{([\da-f]+)\}/gi,
    (_, value) => JSON.stringify(String.fromCodePoint(Number.parseInt(value, 16))).slice(1, -1)));
}

if (JSON.stringify(Object.keys(english).sort()) !== JSON.stringify(Object.keys(chinese).sort())) {
  missing.push('English and Chinese catalog keys differ');
}
for (const filename of files(path.join(root, 'crates'))) {
  if (!filename.endsWith('.rs') || filename.includes(`${path.sep}i18n${path.sep}`)) continue;
  const source = fs.readFileSync(filename, 'utf8');
  const expressions = [
    /\b(?:Label|Headline|ListHeader|ListSubHeader|ContextMenuEntry)::localized\(\s*("(?:[^"\\]|\\.)*")/g,
    /\bTooltip::localized_\w+\(\s*("(?:[^"\\]|\\.)*")/g,
    /\bi18n::(?:text|static_text|format|Message::new)\(\s*("(?:[^"\\]|\\.)*")/g,
    /\bInputField::localized\(\s*window\s*,\s*cx\s*,\s*("(?:[^"\\]|\\.)*")/g,
    /\bButton::localized\(\s*(?:"(?:[^"\\]|\\.)*"|\w+)\s*,\s*("(?:[^"\\]|\\.)*")/g,
  ];
  for (const expression of expressions) {
    for (const match of source.matchAll(expression)) {
      const message = decode(match[1]);
      if (!/[a-z]{2}/i.test(message) || verbatim.has(message) || Object.hasOwn(chinese, message)) continue;
      const line = source.slice(0, match.index).split('\n').length;
      missing.push(`${path.relative(root, filename)}:${line}: ${JSON.stringify(message)}`);
    }
  }
}

if (missing.length) {
  console.error([...new Set(missing)].join('\n'));
  process.exitCode = 1;
} else {
  console.log(`Validated ${Object.keys(chinese).length} bilingual messages and explicit UI translation calls.`);
}
