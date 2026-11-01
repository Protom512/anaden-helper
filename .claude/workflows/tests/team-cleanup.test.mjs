import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const src = readFileSync(join(here, '..', 'feature-pipeline.js'), 'utf8');

test('finally-block passes team_name to TeamDelete (not argument-less)', () => {
  const m = src.match(/finally\s*\{[\s\S]*?TeamDelete[\s\S]*?\}/);
  assert.ok(m, 'finally block containing TeamDelete should exist');
  assert.match(m[0], /TeamDelete\(\s*\{\s*team_name:\s*\w+/);
  assert.doesNotMatch(m[0], /TeamDelete\(\s*\)/);
});
