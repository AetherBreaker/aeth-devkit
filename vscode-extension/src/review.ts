import * as fs from 'node:fs';
import * as vscode from 'vscode';
import { ReviewRequest, isInside } from './consent';
import { ProposedDocs } from './proposedDocs';

/** `--dry-run`: every change in one multi-diff editor, read-only, nothing awaited. */
export async function openReview(file: string, cache: string, docs: ProposedDocs): Promise<void> {
  let req: ReviewRequest;
  try {
    req = JSON.parse(fs.readFileSync(file, 'utf8')) as ReviewRequest;
    if (!Array.isArray(req.files) || typeof req.id !== 'string') throw new Error('malformed review request');
    for (const f of req.files) {
      for (const p of [f.current_path, f.proposed_path]) {
        if (p && !isInside(cache, p)) throw new Error(`review path outside the devkit cache: ${p}`);
      }
    }
  } catch (e) {
    void vscode.window.showErrorMessage(`aeth-devkit: ${(e as Error).message}`);
    return;
  }
  // `vscode.changes` takes [label, original, modified] triples; a created file diffs
  // against an empty document.
  const resources = req.files.map((f, i): [vscode.Uri, vscode.Uri, vscode.Uri] => [
    vscode.Uri.file(f.path),
    docs.register(req.id, 'current', `${i}/${f.label}`, f.current_path ? fs.readFileSync(f.current_path, 'utf8') : ''),
    docs.register(req.id, 'proposed', `${i}/${f.label}`, fs.readFileSync(f.proposed_path, 'utf8')),
  ]);
  await vscode.commands.executeCommand('vscode.changes', 'devkit setup-project (dry run)', resources);
}
