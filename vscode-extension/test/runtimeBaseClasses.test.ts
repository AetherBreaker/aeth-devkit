import { describe, expect, it } from 'vitest';
import { computeModulePath, ensureRuntimeBaseClassesArray, insertIntoRuntimeBaseClasses } from '../src/runtimeBaseClasses';

describe('insertIntoRuntimeBaseClasses', () => {
  it('appends before the closing bracket with the existing indent', () => {
    const toml = '[tool.ruff.lint.flake8-type-checking]\n  runtime-evaluated-base-classes = [\n    "a.B",\n  ]\n';
    expect(insertIntoRuntimeBaseClasses(toml, 'c.D')).toBe(
      '[tool.ruff.lint.flake8-type-checking]\n  runtime-evaluated-base-classes = [\n    "a.B",\n    "c.D",\n  ]\n',
    );
  });
  it('uses the default indent for an empty array and rejects inline arrays', () => {
    expect(insertIntoRuntimeBaseClasses('runtime-evaluated-base-classes = [\n]\n', 'x.Y')).toBe(
      'runtime-evaluated-base-classes = [\n      "x.Y",\n]\n',
    );
    expect(insertIntoRuntimeBaseClasses('runtime-evaluated-base-classes = ["a"]\n', 'x.Y')).toBeNull();
    expect(insertIntoRuntimeBaseClasses('[tool]\n', 'x.Y')).toBeNull();
  });
});

describe('ensureRuntimeBaseClassesArray', () => {
  it('leaves an existing array alone', () => {
    const toml = 'runtime-evaluated-base-classes = [\n]\n';
    expect(ensureRuntimeBaseClassesArray(toml)).toBe(toml);
  });
  it('adds the array under the existing table or appends the table', () => {
    expect(ensureRuntimeBaseClassesArray('[tool.ruff.lint.flake8-type-checking]\n  strict = true\n')).toBe(
      '[tool.ruff.lint.flake8-type-checking]\n  runtime-evaluated-base-classes = [\n  ]\n  strict = true\n',
    );
    expect(ensureRuntimeBaseClassesArray('[project]\nname = "x"\n\n')).toBe(
      '[project]\nname = "x"\n\n[tool.ruff.lint.flake8-type-checking]\n  runtime-evaluated-base-classes = [\n  ]\n',
    );
  });
});

describe('computeModulePath', () => {
  it('walks up through packages and drops __init__', () => {
    const pkg = new Set(['/w/src/pkg/__init__.py', '/w/src/pkg/sub/__init__.py']);
    const exists = (p: string) => pkg.has(p.replace(/\\/g, '/'));
    expect(computeModulePath('/w/src/pkg/sub/mod.py', exists)).toBe('pkg.sub.mod');
    expect(computeModulePath('/w/src/pkg/sub/__init__.py', exists)).toBe('pkg.sub');
    expect(computeModulePath('/w/src/other.py', exists)).toBe('other');
    expect(computeModulePath('', exists)).toBe('');
  });
});
