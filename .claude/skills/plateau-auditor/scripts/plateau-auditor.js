const MAX_VIEWPOINT_LENGTH = 200;
const DEFAULT_VARIANCE_PCT = 21;
const DEFAULT_PLATEAU_WINDOW = 4;
const MAX_AUDITOR_FINDINGS = 5;

const REDACTION_PATTERNS = [
  /#\d{2,}/,
  /[A-Za-z0-9_\-./]+\.(?:rs|mjs|js|toml|json|md|py)\b/,
  /(?:feature|fix|refactor|impl|chore)\/[A-Za-z0-9_\-./]+/,
];

export function redactionViolations(text) {
  if (typeof text !== 'string') {
    return ['non-string'];
  }
  const found = [];
  for (const pattern of REDACTION_PATTERNS) {
    const m = pattern.exec(text);
    if (m) {
      found.push(m[0]);
    }
  }
  return found;
}

function normalizeViewpoint(text) {
  return text.trim().toLowerCase().replace(/\s+/g, ' ');
}

export function parseViewpointLedger(text) {
  const result = { viewpoints: [], errors: [] };
  if (typeof text !== 'string') {
    result.errors.push({ line: 0, reason: 'non-string' });
    return result;
  }
  let category = 'general';
  const lines = text.split(/\r?\n/);
  for (let i = 0; i < lines.length; i += 1) {
    const raw = lines[i];
    const lineNo = i + 1;
    if (raw.startsWith('## ')) {
      category = raw.slice(3).trim() || 'general';
      continue;
    }
    if (!raw.startsWith('- ')) {
      continue;
    }
    const text2 = raw.slice(2).trim();
    if (text2.length === 0) {
      result.errors.push({ line: lineNo, reason: 'empty-viewpoint' });
      continue;
    }
    if (text2.length > MAX_VIEWPOINT_LENGTH) {
      result.errors.push({ line: lineNo, reason: 'too-long' });
      continue;
    }
    const violations = redactionViolations(text2);
    if (violations.length > 0) {
      result.errors.push({ line: lineNo, reason: 'redaction-violation', detail: violations.join(',') });
      continue;
    }
    result.viewpoints.push({ category, text: text2 });
  }
  return result;
}

export function filterUntried(viewpoints, triedTexts) {
  if (!Array.isArray(viewpoints)) {
    return [];
  }
  const triedList = Array.isArray(triedTexts) ? triedTexts : [];
  const triedNorm = triedList
    .filter((t) => typeof t === 'string' && t.trim().length > 0)
    .map(normalizeViewpoint);
  const untried = [];
  for (const vp of viewpoints) {
    if (!vp || typeof vp.text !== 'string') {
      continue;
    }
    const vpNorm = normalizeViewpoint(vp.text);
    let tried = false;
    for (const t of triedNorm) {
      if (t === vpNorm || vpNorm.includes(t) || t.includes(vpNorm)) {
        tried = true;
        break;
      }
    }
    if (!tried) untried.push(vp);
  }
  return untried;
}

export function isBeyondVariance(newBest, priorBest, variancePct) {
  if (
    typeof newBest !== 'number' || !Number.isFinite(newBest) ||
    typeof priorBest !== 'number' || !Number.isFinite(priorBest) ||
    typeof variancePct !== 'number' || !Number.isFinite(variancePct) || variancePct < 0
  ) {
    return false;
  }
  return newBest > priorBest * (1 + variancePct / 100);
}

export function detectPlateau(scores, variancePct, window) {
  if (!Array.isArray(scores) || scores.length === 0) {
    return false;
  }
  const win = Number.isInteger(window) && window >= 1 ? window : DEFAULT_PLATEAU_WINDOW;
  const v = typeof variancePct === 'number' && Number.isFinite(variancePct) && variancePct >= 0
    ? variancePct
    : DEFAULT_VARIANCE_PCT;
  let best = null;
  let quietTail = 0;
  for (const s of scores) {
    if (typeof s !== 'number' || !Number.isFinite(s)) {
      return false;
    }
    const improved = best === null ? true : isBeyondVariance(s, best, v);
    if (improved) {
      quietTail = 0;
    } else {
      quietTail += 1;
    }
    if (best === null || s > best) {
      best = s;
    }
  }
  return quietTail >= win;
}

export function buildAuditorPrompt(input) {
  const viewpoints = input && Array.isArray(input.viewpoints) ? input.viewpoints : [];
  if (viewpoints.length === 0) {
    return { ok: false, reason: 'empty-viewpoint-ledger' };
  }
  const untried = filterUntried(viewpoints, input.tried);
  if (untried.length === 0) {
    return { ok: false, reason: 'no-untried-viewpoints' };
  }
  const lines = [];
  lines.push('あなたは監査役サブエージェントです。直近の run が停滞 (plateau) しているため、記録を観点台帳と突き合わせてください。');
  lines.push('');
  if (input.plateauSummary) {
    lines.push(`停滞の証拠: ${input.plateauSummary}`);
    lines.push('');
  }
  if (input.tried && input.tried.length > 0) {
    lines.push('すでに試した手:');
    for (const t of input.tried) {
      lines.push(`- ${t}`);
    }
    lines.push('');
  }
  lines.push('未着手の観点候補 (この中から、記録と照らして実際に未着手かつ適用可能なものだけを選んでください):');
  const cap = Math.min(untried.length, MAX_AUDITOR_FINDINGS);
  for (const vp of untried.slice(0, cap)) {
    lines.push(`- [${vp.category}] ${vp.text}`);
  }
  if (untried.length > cap) {
    lines.push(`(ほか ${untried.length - cap} 件は省略)`);
  }
  lines.push('');
  lines.push(`出力形式: 適用すべき観点がある場合は先頭に "UNTRIED: " を付けた1行ごとに挙げる (最大 ${MAX_AUDITOR_FINDINGS} 件)。観点のまま1行で書く。対象名 (ファイルパス・PR番号・ブランチ名) は含めない。該当なしは "NONE" の1行のみ。`);
  lines.push('記録と照らして該当がなければ NONE で構いません。推測で埋めないこと。');
  return { ok: true, prompt: lines.join('\n'), untriedCount: untried.length };
}

export function parseAuditorReport(text) {
  if (typeof text !== 'string' || text.trim().length === 0) {
    return { ok: false, reason: 'unparseable' };
  }
  const items = [];
  let sawNone = false;
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (line === 'NONE') {
      sawNone = true;
      continue;
    }
    if (line.startsWith('UNTRIED: ')) {
      const item = line.slice(9).trim();
      if (item.length > 0) {
        items.push(item);
      }
    }
  }
  if (items.length > MAX_AUDITOR_FINDINGS) {
    return { ok: false, reason: 'too-many-findings' };
  }
  if (items.length === 0 && !sawNone) {
    return { ok: false, reason: 'unparseable' };
  }
  return { ok: true, items };
}
