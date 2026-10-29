// Subsequence matching used by Quick Open and the Command Palette (port of FuzzyMatcher.swift).

/**
 * Scores `query` against a path. Matches in the file name weigh more than matches in folders; consecutive characters
 * and word starts earn bonuses; shorter paths win ties. Higher = better; null = no match.
 */
export function fuzzyScore(query: string, path: string): number | null {
  const q = query.toLowerCase().replace(/\s+/g, "");
  if (!q) return 0;
  const chars = path.toLowerCase();
  const nameStart = chars.lastIndexOf("/") + 1;
  let score = 0;
  let qi = 0;
  let previous = -2;
  for (let i = 0; i < chars.length && qi < q.length; i++) {
    if (chars[i] !== q[qi]) continue;
    let s = 1;
    if (i === previous + 1) s += 5;
    if (i === 0 || "/._- ".includes(chars[i - 1] ?? "")) s += 4;
    if (i >= nameStart) s += 3;
    score += s;
    previous = i;
    qi++;
  }
  if (qi < q.length) return null;
  const name = chars.slice(nameStart);
  if (name === q) score += 40;
  else if (name.startsWith(q)) score += 20;
  else if (name.includes(q)) score += 10;
  return score * 4 - Math.floor(Math.min(chars.length, 120) / 4);
}

/** Indices of the matched characters (for highlighting), or [] when there is no match. */
export function fuzzyPositions(query: string, text: string): number[] {
  const q = query.toLowerCase().replace(/\s+/g, "");
  const lower = text.toLowerCase();
  const out: number[] = [];
  let qi = 0;
  for (let i = 0; i < lower.length && qi < q.length; i++) {
    if (lower[i] === q[qi]) { out.push(i); qi++; }
  }
  return qi === q.length ? out : [];
}
