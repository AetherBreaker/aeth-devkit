import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  Hunk,
  HunkState,
  cacheDir,
  cancelPath,
  docPath,
  isInside,
  merge,
  panels,
  parseRequest,
  requestPath,
  rightPanelLines,
  splitLines,
  writeResponse,
} from '../src/consent';

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'aeth-consent-'));

const request = (cache: string) =>
  JSON.stringify({
    protocol: 1,
    id: '12-0',
    title: 'docker/Dockerfile',
    current_path: path.join(cache, 'consent', '12-0.current'),
    proposed_path: path.join(cache, 'consent', '12-0.proposed'),
    hunks: [{ current: [1, 2], proposed: [1, 2] }],
    offer_replace_all: true,
    content_menu: false,
    response_path: path.join(cache, 'consent', '12-0.response.json'),
  });

describe('cacheDir', () => {
  it('matches the CLI on every platform', () => {
    expect(cacheDir({ LOCALAPPDATA: 'C:\\Users\\j\\AppData\\Local' }, 'win32', 'C:\\Users\\j')).toBe(
      path.join('C:\\Users\\j\\AppData\\Local', 'aeth-devkit'),
    );
    expect(cacheDir({}, 'win32', 'C:\\Users\\j')).toBeUndefined();
    expect(cacheDir({}, 'linux', '/home/j')).toBe(path.join('/home/j/.cache', 'aeth-devkit'));
    expect(cacheDir({ XDG_CACHE_HOME: '/x' }, 'linux', '/home/j')).toBe(path.join('/x', 'aeth-devkit'));
  });
});

describe('requestPath and isInside', () => {
  it('accepts only well-formed ids', () => {
    expect(requestPath('/c', '12-0')).toBe(path.join('/c', 'consent', '12-0.request.json'));
    expect(requestPath('/c', 'review-12')).toBe(path.join('/c', 'consent', 'review-12.request.json'));
    for (const bad of ['', '../x', '12-0/../../etc', 'review', 'a-b']) expect(() => requestPath('/c', bad)).toThrow();
  });
  it('rejects paths outside the cache', () => {
    const c = tmp();
    expect(isInside(c, path.join(c, 'consent', 'x'))).toBe(true);
    expect(isInside(c, c)).toBe(false);
    expect(isInside(c, path.join(c, '..', 'x'))).toBe(false);
    expect(isInside(c, path.join(os.tmpdir(), 'elsewhere'))).toBe(false);
  });
});

describe('docPath', () => {
  it('ends in the real file name so the language is detected', () => {
    expect(docPath('12-0', 'proposed', 'docker/Dockerfile')).toBe('/12-0/proposed/docker/Dockerfile');
    expect(docPath('12-0', 'current', 'docker/compose.yaml: new service worker')).toBe(
      '/12-0/current/new service worker/docker/compose.yaml',
    );
    expect(docPath('review-1', 'proposed', '0/docker/compose.yaml')).toBe('/review-1/proposed/0/docker/compose.yaml');
  });
});

describe('parseRequest', () => {
  it('parses a spec request and rejects escapes and garbage', () => {
    const c = tmp();
    const r = parseRequest(request(c), c);
    expect(r.hunks[0].proposed).toEqual([1, 2]);
    expect(cancelPath(r)).toBe(path.join(c, 'consent', '12-0.cancel'));
    const escaped = request(c).replace(
      /"response_path":"[^"]*"/,
      `"response_path":${JSON.stringify(path.join(c, '..', 'evil.json'))}`,
    );
    expect(() => parseRequest(escaped, c)).toThrow(/outside the devkit cache/);
    expect(() => parseRequest('{"id":"12-0"}', c)).toThrow(/malformed/);
  });
});

describe('HunkState', () => {
  it('counts undecided as accepted and collapses all-or-nothing answers', () => {
    const s = new HunkState(3);
    expect(s.response()).toEqual({ decision: 'replace' });
    expect(s.undecidedCount).toBe(3);
    s.decide(1, 'reject');
    expect(s.acceptedCount).toBe(2);
    expect(s.response()).toEqual({ decision: 'partial', accepted: [0, 2] });
    s.decide(0, 'reject');
    s.decide(2, 'reject');
    expect(s.response()).toEqual({ decision: 'keep' });
    s.decide(1, undefined);
    expect(s.response()).toEqual({ decision: 'partial', accepted: [1] });
    s.acceptAll();
    expect(s.response()).toEqual({ decision: 'replace' });
    expect(s.undecidedCount).toBe(0);
    s.decide(99, 'reject');
    expect(s.acceptedCount).toBe(3);
  });
});

describe('panels', () => {
  // The CLI's own test texts: two hunks, the second one line longer in the proposal.
  const cur = 'a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n';
  const prop = 'a\nB\nc\nd\ne\nf\ng\nh\ni\nJ\nK\n';
  const hunks: Hunk[] = [
    { current: [1, 2], proposed: [1, 2] },
    { current: [9, 10], proposed: [9, 11] },
  ];

  it('splits lines losslessly', () => {
    expect(splitLines('a\nb').join('')).toBe('a\nb');
    expect(splitLines('a\nb\n')).toEqual(['a\n', 'b\n']);
    expect(splitLines('')).toEqual([]);
  });

  it('mirrors the CLI assemble for rejected hunks', () => {
    const ranges = hunks.map((h) => ({ base: h.proposed, other: h.current }));
    expect(merge(prop, cur, ranges, () => false)).toBe(prop);
    expect(merge(prop, cur, ranges, () => true)).toBe(cur);
    expect(merge(prop, cur, ranges, (i) => i === 0)).toBe('a\nb\nc\nd\ne\nf\ng\nh\ni\nJ\nK\n');
  });

  it('collapses decided hunks on both sides and shifts later lens lines', () => {
    const s = new HunkState(2);
    expect(panels(cur, prop, hunks, s)).toEqual({ left: cur, right: prop });
    expect(rightPanelLines(hunks, s)).toEqual([1, 9]);
    s.decide(0, 'accept');
    expect(panels(cur, prop, hunks, s).left).toBe('a\nB\nc\nd\ne\nf\ng\nh\ni\nj\n');
    s.decide(1, 'reject');
    const p = panels(cur, prop, hunks, s);
    expect(p.right).toBe('a\nB\nc\nd\ne\nf\ng\nh\ni\nj\n');
    expect(p.left).toBe(p.right);
    expect(rightPanelLines(hunks, s)).toEqual([1, 9]);
    // A rejected hunk earlier in the file shifts everything after it.
    const t = new HunkState(2);
    const grow: Hunk[] = [
      { current: [0, 3], proposed: [0, 1] },
      { current: [5, 6], proposed: [3, 4] },
    ];
    t.decide(0, 'reject');
    expect(rightPanelLines(grow, t)).toEqual([0, 5]);
  });
});

describe('writeResponse', () => {
  it('leaves only the final file behind', () => {
    const dir = tmp();
    const p = path.join(dir, 'r.response.json');
    writeResponse(p, { decision: 'partial', accepted: [0] });
    expect(JSON.parse(fs.readFileSync(p, 'utf8'))).toEqual({ decision: 'partial', accepted: [0] });
    expect(fs.readdirSync(dir)).toEqual(['r.response.json']);
  });
});
