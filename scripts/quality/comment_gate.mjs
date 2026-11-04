import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(scriptDir, '..', '..');

const EXT_LANG = new Map([
  ['.rs', 'rust'],
  ['.mjs', 'js'],
  ['.sh', 'shell'],
]);

const isIdent = (c) => /[A-Za-z0-9_]/.test(c);

function skipCookedString(src, start) {
  const n = src.length;
  let j = start + 1;
  while (j < n) {
    if (src[j] === '\\') { j += 2; continue; }
    if (src[j] === '"') return j + 1;
    j += 1;
  }
  return n;
}

function tryRustStringAt(src, idx) {
  if (idx > 0 && isIdent(src[idx - 1])) return -1;
  let j = idx;
  if (src[j] === 'b') j += 1;
  if (src[j] !== 'r') {
    return src[j] === '"' ? skipCookedString(src, j) : -1;
  }
  j += 1;
  let hashes = 0;
  while (src[j] === '#') { hashes += 1; j += 1; }
  if (src[j] !== '"') return -1;
  j += 1;
  const closing = '"' + '#'.repeat(hashes);
  const k = src.indexOf(closing, j);
  return k === -1 ? src.length : k + closing.length;
}

export function lexRust(src) {
  const segs = [];
  const n = src.length;
  let i = 0;
  let codeStart = 0;
  const code = (to) => { if (to > codeStart) segs.push({ type: 'code', start: codeStart, end: to }); };
  const comment = (from, to, newline) => { code(from); segs.push({ type: 'comment', start: from, end: to, newline }); codeStart = to; };

  while (i < n) {
    const c = src[i];
    if (c === '/' && src[i + 1] === '/') {
      let j = src.indexOf('\n', i);
      if (j === -1) j = n;
      comment(i, j, true);
      i = j;
      continue;
    }
    if (c === '/' && src[i + 1] === '*') {
      let depth = 1;
      let j = i + 2;
      while (j < n && depth > 0) {
        if (src[j] === '/' && src[j + 1] === '*') { depth += 1; j += 2; continue; }
        if (src[j] === '*' && src[j + 1] === '/') { depth -= 1; j += 2; continue; }
        j += 1;
      }
      comment(i, j, false);
      i = j;
      continue;
    }
    if (c === '"' || c === 'b' || c === 'r') {
      const end = tryRustStringAt(src, i);
      if (end !== -1) { i = end; continue; }
    }
    if (c === "'") {
      if (src[i + 1] === '\\') {
        let j = i + 2;
        if (src[j] === 'u' && src[j + 1] === '{') {
          const close = src.indexOf('}', j);
          j = close === -1 ? n : close + 1;
        } else if (src[j] === 'x') {
          j += 3;
        } else {
          j += 1;
        }
        if (src[j] === "'") { i = j + 1; continue; }
      } else if (src[i + 2] === "'" && src[i + 1] !== "'") {
        i += 3;
        continue;
      }
      i += 1;
      continue;
    }
    i += 1;
  }
  code(n);
  return segs;
}

const JS_REGEX_ALLOWED_WORDS = new Set([
  'return', 'typeof', 'instanceof', 'in', 'of', 'new', 'delete', 'void',
  'case', 'do', 'else', 'yield', 'await', 'throw',
]);

function scanJsRegex(src, start) {
  const n = src.length;
  let j = start + 1;
  let inClass = false;
  while (j < n) {
    const c = src[j];
    if (c === '\\') { j += 2; continue; }
    if (c === '\n') return -1;
    if (c === '[') { inClass = true; j += 1; continue; }
    if (c === ']') { inClass = false; j += 1; continue; }
    if (c === '/' && !inClass) {
      j += 1;
      while (j < n && /[a-z]/.test(src[j])) j += 1;
      return j;
    }
    j += 1;
  }
  return -1;
}

export function lexJs(src) {
  const segs = [];
  const n = src.length;
  let i = 0;
  let codeStart = 0;
  let lastWord = '';
  let lastCh = '';
  const code = (to) => { if (to > codeStart) segs.push({ type: 'code', start: codeStart, end: to }); };
  const comment = (from, to, newline) => { code(from); segs.push({ type: 'comment', start: from, end: to, newline }); codeStart = to; };
  const note = (text) => {
    if (!text) return;
    const wordMatch = text.match(/[A-Za-z_$][A-Za-z0-9_$]*$/);
    lastWord = wordMatch ? wordMatch[0] : '';
    lastCh = text[text.length - 1];
  };
  const regexAllowed = () =>
    lastCh === '' ||
    '([{,;=:?!&|+-*/%^~<>'.includes(lastCh) ||
    JS_REGEX_ALLOWED_WORDS.has(lastWord);

  const stack = [];
  while (i < n) {
    const c = src[i];
    if (stack.length > 0 && stack[stack.length - 1] === 'tpl-text') {
      if (c === '`') { stack.pop(); i += 1; continue; }
      if (c === '$' && src[i + 1] === '{') { stack[stack.length - 1] = 'tpl-expr'; stack.push('brace'); i += 2; code(i); codeStart = i; continue; }
      i += 1;
      continue;
    }
    if (c === '/' && src[i + 1] === '/') {
      let j = src.indexOf('\n', i);
      if (j === -1) j = n;
      comment(i, j, true);
      i = j;
      continue;
    }
    if (c === '/' && src[i + 1] === '*') {
      const j = src.indexOf('*/', i + 2);
      const end = j === -1 ? n : j + 2;
      comment(i, end, false);
      i = end;
      continue;
    }
    if (c === '/' && regexAllowed()) {
      const end = scanJsRegex(src, i);
      if (end !== -1) { note(src.slice(i, end)); i = end; code(i); codeStart = i; continue; }
    }
    if (c === '"' || c === "'") {
      let j = i + 1;
      while (j < n) {
        if (src[j] === '\\') { j += 2; continue; }
        if (src[j] === c) { j += 1; break; }
        if (src[j] === '\n') { j += 1; break; }
        j += 1;
      }
      note(src.slice(i, j));
      i = j;
      code(i);
      codeStart = i;
      continue;
    }
    if (c === '`') {
      stack.push('tpl-text');
      i += 1;
      code(i);
      codeStart = i;
      continue;
    }
    if (c === '{') { stack.push('brace'); note('{'); i += 1; continue; }
    if (c === '}') {
      const top = stack.pop();
      if (top === 'brace' && stack.length > 0 && stack[stack.length - 1] === 'tpl-expr') {
        stack[stack.length - 1] = 'tpl-text';
      }
      note('}');
      i += 1;
      continue;
    }
    if (!/\s/.test(c)) note(c);
    i += 1;
  }
  code(n);
  return segs;
}

export function lexShell(src) {
  const segs = [];
  const n = src.length;
  let i = 0;
  let codeStart = 0;
  let heredocDelim = null;
  let heredocDash = false;
  const contexts = [{ q: null, paren: 0 }];
  const code = (to) => { if (to > codeStart) segs.push({ type: 'code', start: codeStart, end: to }); };
  const comment = (from, to) => { code(from); segs.push({ type: 'comment', start: from, end: to, newline: true }); codeStart = to; };

  while (i < n) {
    const c = src[i];
    if (heredocDelim !== null) {
      if (c === '\n') { i += 1; continue; }
      const lineEnd = src.indexOf('\n', i);
      const line = src.slice(i, lineEnd === -1 ? n : lineEnd).replace(/\r$/, '');
      const stripped = heredocDash ? line.replace(/^\t+/, '') : line;
      if (stripped === heredocDelim) heredocDelim = null;
      i = lineEnd === -1 ? n : lineEnd;
      continue;
    }
    const ctx = contexts[contexts.length - 1];
    if (ctx.q === "'") {
      if (c === "'") ctx.q = null;
      i += 1;
      continue;
    }
    if (ctx.q === '"') {
      if (c === '\\') { i += 2; continue; }
      if (c === '"') ctx.q = null;
      if ((c === '$' && src[i + 1] === '(') || c === '`') {
        contexts.push({ q: null, paren: 0 });
        i += c === '`' ? 1 : 2;
        continue;
      }
      i += 1;
      continue;
    }
    if (c === "'") { ctx.q = "'"; i += 1; continue; }
    if (c === '"') { ctx.q = '"'; i += 1; continue; }
    if (c === '#' && (i === 0 || /\s/.test(src[i - 1])) && !(i === 0 && src.startsWith('#!'))) {
      let j = src.indexOf('\n', i);
      if (j === -1) j = n;
      comment(i, j);
      i = j;
      continue;
    }
    if (c === '$' && src[i + 1] === '(') {
      contexts.push({ q: null, paren: 0 });
      i += 2;
      continue;
    }
    if (c === '`') {
      contexts.push({ q: null, paren: 0 });
      i += 1;
      continue;
    }
    if (c === '(') { ctx.paren += 1; i += 1; continue; }
    if (c === ')') {
      if (ctx.paren > 0) ctx.paren -= 1;
      else if (contexts.length > 1) contexts.pop();
      i += 1;
      continue;
    }
    if (c === '<' && src[i + 1] === '<') {
      const nl = src.indexOf('\n', i);
      const rest = src.slice(i, nl === -1 ? n : nl);
      const m = rest.match(/^<<(-?)\s*(["']?)([^\s"';&|)]+)\2/);
      if (m && !/^\d+$/.test(m[3])) {
        heredocDelim = m[3];
        heredocDash = m[1] === '-';
        const eol = src.indexOf('\n', i + m[0].length);
        i = eol === -1 ? n : eol + 1;
        continue;
      }
    }
    i += 1;
  }
  code(n);
  return segs;
}

const LEXERS = { rust: lexRust, js: lexJs, shell: lexShell };

export function findComments(src, lang) {
  const lex = LEXERS[lang];
  if (!lex) throw new Error(`comment-gate: unknown lang ${lang}`);
  const segs = lex(src).filter((s) => s.type === 'comment');
  const out = [];
  for (const s of segs) {
    if (lang === 'shell' && s.start === 0) continue;
    out.push(s);
  }
  return out;
}

export function codeFingerprint(src, lang) {
  const segs = LEXERS[lang](src);
  let out = '';
  for (const s of segs) {
    if (s.type !== 'code') continue;
    out += src.slice(s.start, s.end);
  }
  return out.replace(/\s+/g, '');
}

export function scopeFiles(root = ROOT) {
  const dirs = ['crates', 'scripts'];
  const out = [];
  const walk = (dir) => {
    let entries;
    try { entries = fs.readdirSync(dir, { withFileTypes: true }); } catch { return; }
    for (const e of entries) {
      const p = path.join(dir, e.name);
      if (e.isDirectory()) {
        if (e.name === 'target' || e.name === 'node_modules') continue;
        walk(p);
        continue;
      }
      const lang = EXT_LANG.get(path.extname(e.name));
      if (lang) out.push({ path: p, lang });
    }
  };
  for (const d of dirs) walk(path.join(root, d));
  out.sort((a, b) => a.path.localeCompare(b.path));
  return out;
}

export function check(root = ROOT) {
  const findings = [];
  let filesChecked = 0;
  for (const f of scopeFiles(root)) {
    filesChecked += 1;
    const src = fs.readFileSync(f.path, 'utf8');
    for (const c of findComments(src, f.lang)) {
      const line = src.slice(0, c.start).split('\n').length;
      findings.push({ file: f.path, line, start: c.start, end: c.end });
    }
  }
  return { ok: findings.length === 0, filesChecked, findings };
}

export function main() {
  const result = check();
  if (!result.ok) {
    console.error(`comment-gate: ${result.findings.length} comment(s) in ${result.findings.length > 0 ? result.findings[0].file.split(path.sep).slice(0, 2).join(path.sep) : 'scope'} code (crates/ + scripts/):`);
    for (const f of result.findings.slice(0, 200)) {
      console.error(`  ${path.relative(ROOT, f.file).replace(/\\/g, '/')}:${f.line}`);
    }
    if (result.findings.length > 200) console.error(`  ... ${result.findings.length - 200} more`);
    return 1;
  }
  console.log(`comment-gate: OK (${result.filesChecked} files, no comments)`);
  return 0;
}

const invokedDirectly = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href;
if (invokedDirectly) process.exit(main());
