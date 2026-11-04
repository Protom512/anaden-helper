import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {
  parseCcccToml,
  computeTestRegions,
  flattenFunctions,
  evaluateGate,
  classifyPath,
  main,
} from './complexity_gate.mjs';


test('workspace convention: cfg(test) + allow x3 + blank + mod tests -> region from mod line to closing brace (attrs excluded)', () => {
  const src = [
    'use super::*;',
    '',
    '#[cfg(test)]',
    '#[allow(clippy::unwrap_used)]',
    '#[allow(clippy::panic)]',
    '#[allow(clippy::expect_used)]',
    '',
    'mod tests {',
    '    use super::*;',
    '',
    '    #[test]',
    '    fn foo() {',
    '        assert_eq!(1, 1);',
    '    }',
    '}',
  ].join('\n');
  const regions = computeTestRegions(src);
  assert.equal(regions.length, 1);
  assert.equal(regions[0].start, 8);
  assert.equal(regions[0].end, 15);
  assert.equal(regions[0].kind, 'mod');
});

test('cfg(all(test, windows)) + mod tests -> region', () => {
  const src = [
    '#[cfg(all(test, windows))]',
    'mod tests {',
    '}',
  ].join('\n');
  const regions = computeTestRegions(src);
  assert.equal(regions.length, 1);
  assert.equal(regions[0].start, 2);
  assert.equal(regions[0].end, 3);
  assert.equal(regions[0].kind, 'mod');
});

test('cfg(test) fn helper mid-file -> region covers only that fn; following production fn is outside', () => {
  const src = [
    'pub fn alpha() -> u32 { 1 }',
    '',
    '#[cfg(test)]',
    'fn unresolved() -> Option<u32> { None }',
    '',
    'pub fn beta() -> u32 { 2 }',
  ].join('\n');
  const regions = computeTestRegions(src);
  assert.equal(regions.length, 1);
  assert.equal(regions[0].kind, 'fn');
  assert.equal(regions[0].start, 4);
  assert.equal(regions[0].end, 4);
  assert.ok(regions[0].end < 6);
});

test('pub(crate) mod test_util with visibility modifier -> region', () => {
  const src = [
    '#[cfg(test)]',
    'pub(crate) mod test_util {',
    '    pub fn helper() {}',
    '}',
  ].join('\n');
  const regions = computeTestRegions(src);
  assert.equal(regions.length, 1);
  assert.equal(regions[0].kind, 'mod');
  assert.equal(regions[0].start, 2);
  assert.equal(regions[0].end, 4);
});

test('cfg(test) mod tests nested inside another mod -> region closes at its own brace', () => {
  const src = [
    'mod outer {',
    '    pub fn prod() {}',
    '',
    '    #[cfg(test)]',
    '    mod tests {',
    '        #[test]',
    '        fn inner() {}',
    '    }',
    '',
    '    pub fn prod2() {}',
    '}',
  ].join('\n');
  const regions = computeTestRegions(src);
  assert.equal(regions.length, 1);
  assert.equal(regions[0].start, 5);
  assert.equal(regions[0].end, 8);
  assert.equal(regions[0].kind, 'mod');
});

test('mod tests; without body -> single-line region', () => {
  const src = [
    '#[cfg(test)]',
    'mod tests;',
    '',
    'pub fn prod() {}',
  ].join('\n');
  const regions = computeTestRegions(src);
  assert.equal(regions.length, 1);
  assert.equal(regions[0].start, 2);
  assert.equal(regions[0].end, 2);
  assert.equal(regions[0].kind, 'mod');
});

test('doc comments (///), line comments (//) and blank lines between cfg and item are skipped', () => {
  const src = [
    '#[cfg(test)]',
    '/// doc for the module',
    '// plain comment',
    '',
    'mod tests {',
    '}',
  ].join('\n');
  const regions = computeTestRegions(src);
  assert.equal(regions.length, 1);
  assert.equal(regions[0].start, 5);
  assert.equal(regions[0].end, 6);
});


test('brace inside ordinary string literal does not shift region end', () => {
  const src = ['#[cfg(test)]', 'mod tests {', '    let s = "}";', '}'].join('\n');
  const regions = computeTestRegions(src);
  assert.equal(regions[0].end, 4);
});

test('brace inside raw string r#"} {"# does not shift region end', () => {
  const src = ['#[cfg(test)]', 'mod tests {', '    let s = r#"} {"#;', '    let t = r"\\{";', '}'].join('\n');
  const regions = computeTestRegions(src);
  assert.equal(regions[0].end, 5);
});

test('backslash + newline line-continuation inside string does not drift line numbers', () => {
  const src = [
    '#[cfg(test)]',
    'mod tests {',
    '    fn a() {',
    '        let s = "x\\',
    'y"; let t = 1;',
    '    }',
    '}',
  ].join('\n');
  const regions = computeTestRegions(src);
  assert.equal(regions[0].start, 2);
  assert.equal(regions[0].end, 7);
});

test('char literal brace, escaped char and lifetime mixed -> brace depth not broken', () => {
  const src = [
    '#[cfg(test)]',
    'mod tests {',
    "    let c = '{';",
    "    let n = '\\n';",
    "    let b = b'{';",
    "    fn f<'a>(x: &'a str) -> &'a str { x }",
    '}',
  ].join('\n');
  const regions = computeTestRegions(src);
  assert.equal(regions[0].end, 7);
});

test('braces inside nested block comments are ignored', () => {
  const src = [
    '#[cfg(test)]',
    'mod tests {',
    '    /* { /* } */ let x = 1; /* } */',
    '}',
  ].join('\n');
  const regions = computeTestRegions(src);
  assert.equal(regions[0].end, 4);
});

test('region reports attrStart at the cfg(test) attribute line (for tests-untouched guard)', () => {
  const src = [
    'pub fn prod() {}',
    '',
    '#[cfg(test)]',
    '#[allow(clippy::unwrap_used)]',
    'mod tests {',
    '}',
  ].join('\n');
  const regions = computeTestRegions(src);
  assert.equal(regions.length, 1);
  assert.equal(regions[0].attrStart, 3);
  assert.equal(regions[0].start, 5);
  assert.equal(regions[0].end, 6);
});

test('non-cfg attribute alone does not create a region', () => {
  const src = [
    '#[allow(dead_code)]',
    'pub fn unused() {}',
    '',
    '#[cfg(not(test))]',
    'pub fn other() {}',
  ].join('\n');
  const regions = computeTestRegions(src);
  assert.equal(regions.length, 0);
});


test('parseCcccToml: defaults when keys are missing', () => {
  const parsed = parseCcccToml('');
  assert.equal(parsed.maxCyclomatic, 15);
  assert.equal(parsed.maxCognitive, 20);
});

test('parseCcccToml: parses values and strips # comments', () => {
  const parsed = parseCcccToml(
    ['# header comment', 'max-cyclomatic = 12  # inline', 'max-cognitive=25', 'cache = true', 'cache-file = ".cccc.cache"'].join('\n'),
  );
  assert.equal(parsed.maxCyclomatic, 12);
  assert.equal(parsed.maxCognitive, 25);
  assert.equal(parsed.cache, true);
  assert.equal(parsed.cacheFile, '.cccc.cache');
});

test('parseCcccToml: invalid lines are ignored, non-numeric threshold falls back', () => {
  const parsed = parseCcccToml(['this is not toml', 'max-cyclomatic = abc'].join('\n'));
  assert.equal(parsed.maxCyclomatic, 15);
  assert.equal(parsed.maxCognitive, 20);
});


test('classifyPath: tests-dir / bench / scan', () => {
  assert.equal(classifyPath('crates/anaden-cli/tests/e2e.rs'), 'tests-dir');
  assert.equal(classifyPath('crates/anaden-studio/benches/log_snapshot_bench.rs'), 'bench');
  assert.equal(classifyPath('crates/anaden-engine/src/pipeline_driver.rs'), 'scan');
  assert.equal(classifyPath('crates/anaden-vision/examples/bench_match.rs'), 'scan');
});


test('flattenFunctions: children recursion with parentPath labels and path normalization', () => {
  const files = [
    {
      path: 'crates\\anaden-studio\\src\\app_ui_authoring.rs',
      cognitive: 35,
      cyclomatic: 25,
      functions: [
        {
          name: 'render_authoring',
          kind: 'method',
          line: 20,
          cognitive: 1,
          cyclomatic: 2,
          children: [
            {
              name: '<closure>',
              kind: 'closure',
              line: 54,
              cognitive: 35,
              cyclomatic: 25,
              children: [{ name: '<closure>', kind: 'closure', line: 60, cognitive: 2, cyclomatic: 3 }],
            },
          ],
        },
      ],
    },
  ];
  const flat = flattenFunctions(files);
  assert.equal(flat.length, 3);
  assert.equal(flat[0].file, 'crates/anaden-studio/src/app_ui_authoring.rs');
  assert.equal(flat[0].parentPath, '');
  assert.equal(flat[1].name, '<closure>');
  assert.equal(flat[1].parentPath, 'render_authoring');
  assert.equal(flat[1].line, 54);
  assert.equal(flat[2].parentPath, 'render_authoring::<closure@54>');
  assert.equal(flat[2].line, 60);
});

test('flattenFunctions: no children still yields every top-level function', () => {
  const files = [
    {
      path: 'crates/a/src/lib.rs',
      cognitive: 0,
      cyclomatic: 0,
      functions: [
        { name: 'f', kind: 'function', line: 1, cognitive: 0, cyclomatic: 1 },
        { name: 'g', kind: 'function', line: 9, cognitive: 0, cyclomatic: 1 },
      ],
    },
  ];
  const flat = flattenFunctions(files);
  assert.equal(flat.length, 2);
  assert.deepEqual(
    flat.map((f) => f.name),
    ['f', 'g'],
  );
});


const SRC_WITH_REGIONS = [
  'pub fn prod_ok() {}',
  '',
  '#[cfg(test)]',
  'mod tests {',
  '    #[test]',
  '    fn t() {}',
  '}',
].join('\n');

const SRC_NO_REGIONS = ['pub fn only() {}'].join('\n');

function fakeCcccJson(entries) {
  return {
    files: entries.map((e) => ({ cognitive: 0, cyclomatic: 0, functions: e.functions, path: e.path })),
    summary: { file_count: entries.length, function_count: 0 },
  };
}

test('evaluateGate: boundary — cyc=15/cog=20 passes, cyc=16 or cog=21 violates', () => {
  const ccccJson = fakeCcccJson([
    {
      path: 'crates/a/src/lib.rs',
      functions: [
        { name: 'at_limit', kind: 'function', line: 1, cognitive: 20, cyclomatic: 15 },
        { name: 'cyc_over', kind: 'function', line: 2, cognitive: 1, cyclomatic: 16 },
        { name: 'cog_over', kind: 'function', line: 3, cognitive: 21, cyclomatic: 1 },
      ],
    },
  ]);
  const result = evaluateGate({
    ccccJson,
    sources: { 'crates/a/src/lib.rs': SRC_NO_REGIONS },
    thresholds: { maxCyclomatic: 15, maxCognitive: 20 },
  });
  assert.deepEqual(
    result.violations.map((v) => v.name).sort(),
    ['cog_over', 'cyc_over'],
  );
  assert.equal(result.counts.violations, 2);
});

test('evaluateGate: tests-dir files are wholly excluded, bench files are out of scope', () => {
  const ccccJson = fakeCcccJson([
    {
      path: 'crates/a/tests/integ.rs',
      functions: [{ name: 'big_test', kind: 'function', line: 1, cognitive: 99, cyclomatic: 99 }],
    },
    {
      path: 'crates/a/benches/b.rs',
      functions: [{ name: 'bench_fn', kind: 'function', line: 1, cognitive: 99, cyclomatic: 99 }],
    },
    {
      path: 'crates/a/src/lib.rs',
      functions: [{ name: 'ok', kind: 'function', line: 1, cognitive: 1, cyclomatic: 1 }],
    },
  ]);
  const result = evaluateGate({
    ccccJson,
    sources: {
      'crates/a/tests/integ.rs': 'fn big_test() {}',
      'crates/a/benches/b.rs': 'fn bench_fn() {}',
      'crates/a/src/lib.rs': SRC_NO_REGIONS,
    },
    thresholds: { maxCyclomatic: 15, maxCognitive: 20 },
  });
  assert.equal(result.counts.violations, 0);
  assert.equal(result.counts.testExcluded, 1);
  assert.equal(result.counts.benchOutOfScope, 1);
  assert.equal(result.benchViolations.length, 1);
  assert.equal(result.testRegions.files.length, 1);
  assert.equal(result.testRegions.files[0].basis, 'tests-dir');
});

test('evaluateGate: high-complexity fn inside a test region goes to excludedTestViolations', () => {
  const ccccJson = fakeCcccJson([
    {
      path: 'crates/a/src/lib.rs',
      functions: [
        { name: 'prod_violator', kind: 'function', line: 1, cognitive: 40, cyclomatic: 30 },
        { name: 'test_violator', kind: 'function', line: 6, cognitive: 36, cyclomatic: 20 },
      ],
    },
  ]);
  const result = evaluateGate({
    ccccJson,
    sources: { 'crates/a/src/lib.rs': SRC_WITH_REGIONS },
    thresholds: { maxCyclomatic: 15, maxCognitive: 20 },
  });
  assert.equal(result.violations.length, 1);
  assert.equal(result.violations[0].name, 'prod_violator');
  assert.equal(result.excludedTestViolations.length, 1);
  assert.equal(result.excludedTestViolations[0].name, 'test_violator');
  assert.equal(result.counts.production, 1);
  assert.equal(result.counts.testExcluded, 1);
});

test('evaluateGate: closure violations nested via children are detected (the 17-violation requirement)', () => {
  const ccccJson = fakeCcccJson([
    {
      path: 'crates/a/src/lib.rs',
      functions: [
        {
          name: 'outer',
          kind: 'function',
          line: 1,
          cognitive: 1,
          cyclomatic: 1,
          children: [{ name: '<closure>', kind: 'closure', line: 3, cognitive: 35, cyclomatic: 25 }],
        },
      ],
    },
  ]);
  const result = evaluateGate({
    ccccJson,
    sources: { 'crates/a/src/lib.rs': SRC_NO_REGIONS },
    thresholds: { maxCyclomatic: 15, maxCognitive: 20 },
  });
  assert.equal(result.violations.length, 1);
  assert.equal(result.violations[0].kind, 'closure');
  assert.equal(result.violations[0].cyclomatic, 25);
});

test('evaluateGate: thresholds are injectable', () => {
  const ccccJson = fakeCcccJson([
    {
      path: 'crates/a/src/lib.rs',
      functions: [{ name: 'mid', kind: 'function', line: 1, cognitive: 5, cyclomatic: 5 }],
    },
  ]);
  const strict = evaluateGate({
    ccccJson,
    sources: { 'crates/a/src/lib.rs': SRC_NO_REGIONS },
    thresholds: { maxCyclomatic: 4, maxCognitive: 4 },
  });
  const loose = evaluateGate({
    ccccJson,
    sources: { 'crates/a/src/lib.rs': SRC_NO_REGIONS },
    thresholds: { maxCyclomatic: 10, maxCognitive: 10 },
  });
  assert.equal(strict.counts.violations, 1);
  assert.equal(loose.counts.violations, 0);
});


function makeTmpRoot() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'cccc-gate-'));
}

test('main: violation fixture exits 1, writes report, classifies regions', () => {
  const root = makeTmpRoot();
  try {
    fs.mkdirSync(path.join(root, 'src'), { recursive: true });
    fs.mkdirSync(path.join(root, 'tests'), { recursive: true });
    const libSrc = [
      'pub fn violating() {',
      '    if true {} if true {} if true {}',
      '}',
      '#[cfg(test)]',
      'mod tests {',
      '    fn huge() {}',
      '}',
    ].join('\n');
    fs.writeFileSync(path.join(root, 'src', 'lib.rs'), libSrc);
    fs.writeFileSync(path.join(root, 'tests', 'integ.rs'), 'fn t() {}\n');
    const ccccJson = fakeCcccJson([
      {
        path: path.join('src', 'lib.rs'),
        functions: [
          { name: 'violating', kind: 'function', line: 1, cognitive: 30, cyclomatic: 20 },
          { name: 'huge', kind: 'function', line: 6, cognitive: 50, cyclomatic: 40 },
        ],
      },
      {
        path: path.join('tests', 'integ.rs'),
        functions: [{ name: 't', kind: 'function', line: 1, cognitive: 1, cyclomatic: 1 }],
      },
    ]);
    const code = main([], { root, ccccJson });
    assert.equal(code, 1);
    const reportPath = path.join(root, 'target', 'quality', 'cccc-report.json');
    assert.ok(fs.existsSync(reportPath), 'report json must exist');
    const report = JSON.parse(fs.readFileSync(reportPath, 'utf8'));
    assert.equal(report.counts.violations, 1);
    assert.equal(report.violations[0].name, 'violating');
    assert.equal(report.excludedTestViolations.length, 1);
    assert.equal(report.excludedTestViolations[0].name, 'huge');
    assert.equal(report.testRegions.regions.length, 1);
    assert.equal(report.testRegions.regions[0].start, 5);
    assert.equal(report.testRegions.files.length, 1);
    assert.ok(report.generatedAt);
    assert.ok(report.tool);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test('main: clean fixture exits 0', () => {
  const root = makeTmpRoot();
  try {
    fs.mkdirSync(path.join(root, 'src'), { recursive: true });
    fs.writeFileSync(path.join(root, 'src', 'lib.rs'), 'pub fn fine() {}\n');
    const ccccJson = fakeCcccJson([
      {
        path: path.join('src', 'lib.rs'),
        functions: [{ name: 'fine', kind: 'function', line: 1, cognitive: 1, cyclomatic: 1 }],
      },
    ]);
    const code = main([], { root, ccccJson });
    assert.equal(code, 0);
    const report = JSON.parse(
      fs.readFileSync(path.join(root, 'target', 'quality', 'cccc-report.json'), 'utf8'),
    );
    assert.equal(report.counts.violations, 0);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});
