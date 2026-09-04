// Port of the Drekker `addToRuntimeBaseClasses` command. Behaviour is unchanged; only
// the command id moved to `aeth-devkit.addToRuntimeBaseClasses`.
import * as fs from 'node:fs';
import * as path from 'node:path';
import * as vscode from 'vscode';

/**
 * Inserts `fqcn` into the multi-line `runtime-evaluated-base-classes` array. Returns the
 * updated content, or null when the array is missing or inline (`[...]` on one line).
 * A depth counter finds the closing bracket reliably.
 */
export function insertIntoRuntimeBaseClasses(content: string, fqcn: string): string | null {
  const lines = content.split('\n');
  let inArray = false;
  let depth = 0;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (!inArray) {
      if (/^\s*runtime-evaluated-base-classes\s*=\s*\[/.test(line)) {
        inArray = true;
        depth = 0;
        for (const ch of line) {
          if (ch === '[') depth++;
          else if (ch === ']') depth--;
        }
        if (depth <= 0) return null;
      }
    } else {
      for (const ch of line) {
        if (ch === '[') depth++;
        else if (ch === ']') depth--;
      }
      if (depth <= 0) {
        let entryIndent = '      ';
        for (let j = i - 1; j >= 0; j--) {
          const m = /^(\s+)"/.exec(lines[j]);
          if (m) {
            entryIndent = m[1];
            break;
          }
        }
        lines.splice(i, 0, `${entryIndent}"${fqcn}",`);
        return lines.join('\n');
      }
    }
  }
  return null;
}

/**
 * Ensures a multi-line `runtime-evaluated-base-classes` array exists, adding an empty one
 * under `[tool.ruff.lint.flake8-type-checking]` (creating the table at the end if needed).
 */
export function ensureRuntimeBaseClassesArray(content: string): string {
  if (/^\s*runtime-evaluated-base-classes\s*=\s*\[/m.test(content)) return content;
  const emptyArray = ['  runtime-evaluated-base-classes = [', '  ]'];
  const lines = content.split('\n');
  const headerIdx = lines.findIndex((l) => /^\s*\[tool\.ruff\.lint\.flake8-type-checking\]\s*$/.test(l));
  if (headerIdx !== -1) {
    lines.splice(headerIdx + 1, 0, ...emptyArray);
    return lines.join('\n');
  }
  const trimmed = content.replace(/\s*$/, '');
  return `${trimmed}\n\n[tool.ruff.lint.flake8-type-checking]\n${emptyArray.join('\n')}\n`;
}

/**
 * Walks up from a Python file through packages (directories with `__init__.py(i)`) to
 * build its fully-qualified module path. Works for `src/` layouts and site-packages.
 */
export function computeModulePath(filePath: string, exists: (p: string) => boolean = fs.existsSync): string {
  if (!filePath) return '';
  const hasInit = (dir: string) => exists(path.join(dir, '__init__.py')) || exists(path.join(dir, '__init__.pyi'));
  const fileBase = path.basename(filePath).replace(/\.pyi?$/i, '');
  const chain: string[] = [];
  if (fileBase && fileBase !== '__init__') chain.push(fileBase);
  let dir = path.dirname(filePath);
  while (dir && hasInit(dir)) {
    chain.push(path.basename(dir));
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return chain.reverse().join('.');
}

interface Resolved {
  fsPath: string;
  className?: string;
}

/** Follows the definition provider through re-exports to the original `class`/`def`. */
async function resolveDefinition(startUri: vscode.Uri, startPos: vscode.Position): Promise<Resolved | null> {
  type Loc = { uri: vscode.Uri; range: vscode.Range };
  const normalize = (item: vscode.Location | vscode.LocationLink | undefined): Loc | null => {
    if (!item) return null;
    if ('targetUri' in item) return { uri: item.targetUri, range: item.targetSelectionRange ?? item.targetRange };
    if (item.uri && item.range) return { uri: item.uri, range: item.range };
    return null;
  };
  let curUri = startUri;
  let curPos = startPos;
  let result: Resolved | null = null;
  const visited = new Set<string>();
  for (let i = 0; i < 16; i++) {
    let defs: (vscode.Location | vscode.LocationLink)[] | undefined;
    try {
      defs = await vscode.commands.executeCommand('vscode.executeDefinitionProvider', curUri, curPos);
    } catch {
      break;
    }
    if (!defs || defs.length === 0) break;
    const loc = normalize(defs[0]);
    if (!loc) break;
    let doc: vscode.TextDocument;
    try {
      doc = await vscode.workspace.openTextDocument(loc.uri);
    } catch {
      break;
    }
    const lineText = doc.lineAt(loc.range.start.line).text;
    result = { fsPath: loc.uri.fsPath };
    const defMatch = /^\s*(?:class|def)\s+(\w+)/.exec(lineText);
    if (defMatch) {
      result.className = defMatch[1];
      break;
    }
    const key = `${loc.uri.toString()}:${loc.range.start.line}:${loc.range.start.character}`;
    if (visited.has(key)) break;
    visited.add(key);
    curUri = loc.uri;
    curPos = loc.range.start;
  }
  return result;
}

export async function addToRuntimeBaseClasses(): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor) {
    void vscode.window.showErrorMessage('No active editor.');
    return;
  }
  const sel = editor.selection;
  let position = sel.active;
  let fallbackName = '';
  if (!sel.isEmpty) {
    position = sel.start;
    fallbackName = editor.document.getText(sel).trim();
    const wordRange = editor.document.getWordRangeAtPosition(sel.start);
    if (wordRange) {
      position = wordRange.start;
      fallbackName = editor.document.getText(wordRange);
    }
  } else {
    const lineText = editor.document.lineAt(sel.active.line).text;
    const classMatch = /^\s*class\s+(\w+)/.exec(lineText);
    if (classMatch) {
      fallbackName = classMatch[1];
    } else {
      const wordRange = editor.document.getWordRangeAtPosition(sel.active);
      if (wordRange) fallbackName = editor.document.getText(wordRange);
    }
  }
  const wsFolders = vscode.workspace.workspaceFolders;
  if (!wsFolders || wsFolders.length === 0) {
    void vscode.window.showErrorMessage('No workspace folder open.');
    return;
  }
  const wsRoot = wsFolders[0].uri.fsPath;
  const resolved = await resolveDefinition(editor.document.uri, position);
  let className = fallbackName;
  let modulePath = '';
  if (resolved) {
    if (resolved.className) className = resolved.className;
    modulePath = computeModulePath(resolved.fsPath);
  }
  if (!modulePath) modulePath = computeModulePath(editor.document.uri.fsPath);
  const suggested = modulePath && className ? `${modulePath}.${className}` : className;
  const fqcn = await vscode.window.showInputBox({
    title: 'Add to runtime-evaluated-base-classes',
    prompt: 'Fully-qualified class name to add to pyproject.toml',
    value: suggested,
    validateInput: (v) => (v && v.trim() ? null : 'Cannot be empty'),
  });
  if (!fqcn) return;
  const pyprojectPath = path.join(wsRoot, 'pyproject.toml');
  if (!fs.existsSync(pyprojectPath)) {
    void vscode.window.showErrorMessage('pyproject.toml not found in workspace root.');
    return;
  }
  const original = fs.readFileSync(pyprojectPath, 'utf8');
  if (original.includes(`"${fqcn.trim()}"`)) {
    void vscode.window.showInformationMessage(`"${fqcn}" is already listed in runtime-evaluated-base-classes.`);
    return;
  }
  const updated = insertIntoRuntimeBaseClasses(ensureRuntimeBaseClassesArray(original), fqcn.trim());
  if (updated === null) {
    void vscode.window.showErrorMessage('Could not locate runtime-evaluated-base-classes array in pyproject.toml.');
    return;
  }
  fs.writeFileSync(pyprojectPath, updated, 'utf8');
  void vscode.window.showInformationMessage(`Added "${fqcn}" to runtime-evaluated-base-classes in pyproject.toml.`);
}
