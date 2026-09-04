import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { describe, expect, it } from 'vitest';
import { HunkState, cacheDir, cancelPath, isInside, parseRequest, requestPath, writeResponse } from '../src/consent';

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
  it('collapses all-accepted to replace and none to keep', () => {
    const s = new HunkState(3);
    expect(s.response()).toEqual({ decision: 'replace' });
    s.toggle(1);
    expect(s.acceptedCount).toBe(2);
    expect(s.response()).toEqual({ decision: 'partial', accepted: [0, 2] });
    s.set(0, false);
    s.set(2, false);
    expect(s.response()).toEqual({ decision: 'keep' });
    s.acceptAll();
    expect(s.response()).toEqual({ decision: 'replace' });
    s.toggle(99);
    expect(s.acceptedCount).toBe(3);
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
