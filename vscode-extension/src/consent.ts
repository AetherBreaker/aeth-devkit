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
export const ID_PATTERN = /^(?:(\d+)-\d+|review-(\d+))$/;

/**
 * `<cache>/consent/<pid>/<id>.request.json`: each run owns the folder named by its pid and
 * removes it when it ends, so this side only ever reads there and writes its answers.
 */
export function requestPath(cache: string, id: string): string {
  const m = ID_PATTERN.exec(id);
  if (!m) throw new Error(`malformed request id: ${JSON.stringify(id)}`);
  return path.join(cache, 'consent', m[1] ?? m[2], `${id}.request.json`);
}

/**
 * `<id>.ack` is written once every text of a request is in memory (the CLI may delete the
 * files after); `<id>.cancel` is the CLI's, written when it stops waiting.
 */
export function markerPath(requestFile: string, kind: 'ack' | 'cancel'): string {
  return requestFile.replace(/\.request\.json$/, `.${kind}`);
}

/** The message for a request this build cannot answer, or undefined when it can. */
export function protocolMismatch(sent: number): string | undefined {
  if (sent === PROTOCOL) return undefined;
  return `the extension speaks protocol ${PROTOCOL}, devkit sent ${sent}; update one of them`;
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

/**
 * Path of a served document, `/<id>/<side>/…`, ending in the real file name so VS Code
 * picks the language from it: a compose title is `docker/compose.yaml: new service
 * worker`, and with the suffix last the file would highlight as plain text.
 */
export function docPath(id: string, side: 'current' | 'proposed', title: string): string {
  const sep = title.indexOf(': ');
  if (sep === -1) return `/${id}/${side}/${title}`;
  return `/${id}/${side}/${title.slice(sep + 2)}/${title.slice(0, sep)}`;
}

export function cancelPath(req: Request): string {
  return req.response_path.replace(/\.response\.json$/, '.cancel');
}

export type HunkDecision = 'accept' | 'reject' | undefined;

/**
 * Per-hunk decisions. Undecided counts as accepted when applying, so the default answer
 * is still "take the whole proposal".
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
 * The two panel texts for the current decisions. A decided hunk carries the same lines
 * on both sides (the proposed lines when accepted, the current ones when rejected), so
 * its diff collapses like an accepted change in the merge editor. The right panel is the
 * display-side twin of the CLI's `assemble`.
 */
export function panels(current: string, proposed: string, hunks: Hunk[], state: HunkState): { left: string; right: string } {
  const cur = splitLines(current);
  const pro = splitLines(proposed);
  let left = '';
  let right = '';
  let c = 0;
  let p = 0;
  hunks.forEach((h, i) => {
    left += cur.slice(c, h.current[0]).join('');
    right += pro.slice(p, h.proposed[0]).join('');
    const curLines = cur.slice(h.current[0], h.current[1]).join('');
    const proLines = pro.slice(h.proposed[0], h.proposed[1]).join('');
    left += state.accepted(i) ? proLines : curLines;
    right += state.rejected(i) ? curLines : proLines;
    c = h.current[1];
    p = h.proposed[1];
  });
  return { left: left + cur.slice(c).join(''), right: right + pro.slice(p).join('') };
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
