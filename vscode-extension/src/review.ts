import * as fs from 'node:fs';
import * as vscode from 'vscode';
import { PROTOCOL, ReviewRequest, ackPath, isInside } from './consent';
import { ProposedDocs } from './proposedDocs';

/**
 * `--dry-run`: every change in one multi-diff editor, read-only. The CLI waits only for
 * the ack, written once every text is held here, then removes its run folder.
 */
export async function openReview(file: string, cache: string, docs: ProposedDocs): Promise<void> {
  try {
    const req = JSON.parse(fs.readFileSync(file, 'utf8')) as ReviewRequest;
    if (!Array.isArray(req.files) || typeof req.id !== 'string') throw new Error('malformed review request');
    if (req.protocol !== PROTOCOL) {
      throw new Error(`the extension speaks protocol ${PROTOCOL}, devkit sent ${req.protocol}; update one of them`);
    }
    for (const f of req.files) {
      for (const p of [f.current_path, f.proposed_path]) {
        if (p && !isInside(cache, p)) throw new Error(`review path outside the devkit cache: ${p}`);
      }
    }
    // Review ids are per pid, and pids come round again: drop what an earlier review with
    // this id left. `vscode.changes` takes [label, original, modified] triples; a created
    // file diffs against an empty document.
    docs.forget(req.id);
    const resources = req.files.map((f): [vscode.Uri, vscode.Uri, vscode.Uri] => [
      vscode.Uri.file(f.path),
      docs.register(req.id, 'current', f.label, f.current_path ? fs.readFileSync(f.current_path, 'utf8') : ''),
      docs.register(req.id, 'proposed', f.label, fs.readFileSync(f.proposed_path, 'utf8')),
    ]);
    fs.writeFileSync(ackPath(file), '');
    await vscode.commands.executeCommand('vscode.changes', 'devkit setup-project (dry run)', resources);
  } catch (e) {
    void vscode.window.showErrorMessage(`aeth-devkit: ${(e as Error).message}`);
  }
}
