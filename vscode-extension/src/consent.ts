// The consent protocol as the extension sees it. No `vscode` import: everything here is
// unit-tested under vitest. The CLI owns every decision; this side reports choices.
import * as fs from 'node:fs';
import * as path from 'node:path';

export const PROTOCOL = 1;
export const EXTENSION_ID = 'aeth.aeth-devkit';
export const SCHEME = 'aeth-devkit-proposed';

/** `[start, end)` 0-based line ranges in each text; context excluded. */
export interface Hunk {
  current: [number, number];
  proposed: [number, number];
}

export interface Request {
  protocol: number;
  id: string;
  title: string;
  current_path: string;
  proposed_path: string;
  hunks: Hunk[];
  offer_replace_all: boolean;
  content_menu: boolean;
  response_path: string;
}

export interface ReviewRequest {
  protocol: number;
  id: string;
  files: { path: string; label: string; current_path: string | null; proposed_path: string }[];
}

export type Response =
  | { decision: 'replace' }
  | { decision: 'replace_all' }
  | { decision: 'keep' }
  | { decision: 'partial'; accepted: number[] }
  | { decision: 'dismissed' }
  | { decision: 'error'; message: string };

/** devkit's cache dir, computed exactly as `aeth_devkit_core::update::cache_dir` does. */
export function cacheDir(env: NodeJS.ProcessEnv, platform: NodeJS.Platform, home: string): string | undefined {
  if (platform === 'win32') return env.LOCALAPPDATA ? path.join(env.LOCALAPPDATA, 'aeth-devkit') : undefined;
  return path.join(env.XDG_CACHE_HOME || path.join(home, '.cache'), 'aeth-devkit');
}

/** Consent ids are `<pid>-<n>`, review ids `review-<pid>`; nothing else reaches the disk. */
export const ID_PATTERN = /^(\d+-\d+|review-\d+)$/;

export function requestPath(cache: string, id: string): string {
  if (!ID_PATTERN.test(id)) throw new Error(`malformed request id: ${JSON.stringify(id)}`);
  return path.join(cache, 'consent', `${id}.request.json`);
}

/** Whether `file` is strictly inside `dir` (any `vscode://` link can name a request). */
export function isInside(dir: string, file: string): boolean {
  const rel = path.relative(path.resolve(dir), path.resolve(file));
  return rel !== '' && !rel.startsWith('..') && !path.isAbsolute(rel);
}

export function parseRequest(text: string, cache: string): Request {
  const r = JSON.parse(text) as Partial<Request>;
  const ok =
    typeof r.protocol === 'number' &&
    typeof r.id === 'string' &&
    typeof r.title === 'string' &&
    typeof r.current_path === 'string' &&
    typeof r.proposed_path === 'string' &&
    typeof r.response_path === 'string' &&
    Array.isArray(r.hunks);
  if (!ok) throw new Error('malformed consent request');
  const req = r as Request;
  for (const p of [req.current_path, req.proposed_path, req.response_path]) {
    if (!isInside(cache, p)) throw new Error(`request path outside the devkit cache: ${p}`);
  }
  return req;
}

export function cancelPath(req: Request): string {
  return req.response_path.replace(/\.response\.json$/, '.cancel');
}

export type HunkDecision = 'accept' | 'reject' | undefined;

/**
 * Per-hunk decisions. Undecided hunks stay highlighted in the diff and count as accepted
 * when applying, so the default answer is still "take the whole proposal".
 */
export class HunkState {
  readonly decisions: HunkDecision[];

  constructor(count: number) {
    this.decisions = Array<HunkDecision>(count).fill(undefined);
  }

  decide(i: number, d: HunkDecision): void {
    if (i >= 0 && i < this.decisions.length) this.decisions[i] = d;
  }

  acceptAll(): void {
    this.decisions.fill('accept');
  }

  rejected(i: number): boolean {
    return this.decisions[i] === 'reject';
  }

  accepted(i: number): boolean {
    return this.decisions[i] === 'accept';
  }

  get acceptedCount(): number {
    return this.decisions.filter((d) => d !== 'reject').length;
  }

  get undecidedCount(): number {
    return this.decisions.filter((d) => d === undefined).length;
  }

  /** `Apply accepted`: every hunk is a plain replace, none a keep, otherwise partial. */
  response(): Response {
    const idx = this.decisions.flatMap((d, i) => (d === 'reject' ? [] : [i]));
    if (idx.length === this.decisions.length) return { decision: 'replace' };
    if (idx.length === 0) return { decision: 'keep' };
    return { decision: 'partial', accepted: idx };
  }
}

/** Lines with their `\n` kept, like Rust's `split_inclusive`, so joins are lossless. */
export function splitLines(text: string): string[] {
  return text.match(/[^\n]*\n|[^\n]+$/g) ?? [];
}

/**
 * `base` with every hunk where `takeOther(i)` replaced by the other text's lines. The
 * display-side twin of the CLI's `assemble`: with base = proposed and other = current it
 * reverts rejected hunks; with the sides swapped it applies accepted ones to the current
 * text, so a decided hunk shows the same lines in both panels and its diff collapses.
 */
export function merge(
  base: string,
  other: string,
  ranges: { base: [number, number]; other: [number, number] }[],
  takeOther: (i: number) => boolean,
): string {
  const b = splitLines(base);
  const o = splitLines(other);
  let out = '';
  let cursor = 0;
  ranges.forEach((r, i) => {
    out += b.slice(cursor, r.base[0]).join('');
    out += (takeOther(i) ? o.slice(r.other[0], r.other[1]) : b.slice(r.base[0], r.base[1])).join('');
    cursor = r.base[1];
  });
  return out + b.slice(cursor).join('');
}

/** The two panel texts for the current decisions: only undecided hunks still differ. */
export function panels(current: string, proposed: string, hunks: Hunk[], state: HunkState): { left: string; right: string } {
  return {
    left: merge(
      current,
      proposed,
      hunks.map((h) => ({ base: h.current, other: h.proposed })),
      (i) => state.accepted(i),
    ),
    right: merge(
      proposed,
      current,
      hunks.map((h) => ({ base: h.proposed, other: h.current })),
      (i) => state.rejected(i),
    ),
  };
}

/** Where each hunk starts in the right panel once rejected hunks carry current lines. */
export function rightPanelLines(hunks: Hunk[], state: HunkState): number[] {
  let offset = 0;
  return hunks.map((h, i) => {
    const line = h.proposed[0] + offset;
    if (state.rejected(i)) offset += h.current[1] - h.current[0] - (h.proposed[1] - h.proposed[0]);
    return line;
  });
}

/** Temp file + rename: the CLI polls this path and must never read a half-written file. */
export function writeResponse(responsePath: string, response: Response): void {
  const tmp = `${responsePath}.tmp`;
  fs.writeFileSync(tmp, JSON.stringify(response), 'utf8');
  fs.renameSync(tmp, responsePath);
}

/** One open consent diff. `answered` stops the tab-close handler writing `dismissed`. */
export interface Session {
  req: Request;
  state: HunkState;
  answered: boolean;
}
