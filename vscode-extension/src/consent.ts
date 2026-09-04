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

/** Per-hunk accept/reject, accepted by default. */
export class HunkState {
  readonly accepted: boolean[];

  constructor(count: number) {
    this.accepted = Array<boolean>(count).fill(true);
  }

  toggle(i: number): void {
    if (i >= 0 && i < this.accepted.length) this.accepted[i] = !this.accepted[i];
  }

  set(i: number, on: boolean): void {
    if (i >= 0 && i < this.accepted.length) this.accepted[i] = on;
  }

  acceptAll(): void {
    this.accepted.fill(true);
  }

  get acceptedCount(): number {
    return this.accepted.filter(Boolean).length;
  }

  /** `Apply accepted`: every hunk is a plain replace, none a keep, otherwise partial. */
  response(): Response {
    const idx = this.accepted.flatMap((a, i) => (a ? [i] : []));
    if (idx.length === this.accepted.length) return { decision: 'replace' };
    if (idx.length === 0) return { decision: 'keep' };
    return { decision: 'partial', accepted: idx };
  }
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
