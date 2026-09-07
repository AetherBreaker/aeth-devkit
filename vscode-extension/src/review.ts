import * as fs from 'node:fs';
import * as vscode from 'vscode';
import { ReviewRequest, isInside, markerPath, protocolMismatch } from './consent';
import { ProposedDocs } from './proposedDocs';

/**
 * `--dry-run`: every change in one multi-diff editor, read-only. The CLI waits only for
 * the ack, written once every text is held here, then removes its run folder.
 */
export async function openReview(file: string, cache: string, docs: ProposedDocs): Promise<void> {
  try {
    const req = JSON.parse(fs.readFileSync(file, 'utf8')) as ReviewRequest;
    if (!Array.isArray(req.files) || typeof req.id !== 'string') throw new Error('malformed review request');
    const mismatch = protocolMismatch(req.protocol);
    if (mismatch) throw new Error(mismatch);
    // The CLI stopped waiting (its ack deadline passed): nothing to show any more.
    if (fs.existsSync(markerPath(file, 'cancel'))) return;
    for (const f of req.files) {
      for (const p of [f.current_path, f.proposed_path]) {
        if (p && !isInside(cache, p)) throw new Error(`review path outside the devkit cache: ${p}`);
      }
    }
    // `vscode.changes` takes [label, original, modified] triples; a created file diffs
    // against an empty document.
    docs.forgetReviews();
    const resources = req.files.map((f): [vscode.Uri, vscode.Uri, vscode.Uri] => [
      vscode.Uri.file(f.path),
      docs.register(req.id, 'current', f.label, f.current_path ? fs.readFileSync(f.current_path, 'utf8') : ''),
      docs.register(req.id, 'proposed', f.label, fs.readFileSync(f.proposed_path, 'utf8')),
    ]);
    fs.writeFileSync(markerPath(file, 'ack'), '');
    await vscode.commands.executeCommand('vscode.changes', 'devkit setup-project (dry run)', resources);
  } catch (e) {
    // The run folder is gone: the CLI gave up on the review or has finished.
    const gone = (e as NodeJS.ErrnoException).code === 'ENOENT';
    void vscode.window.showErrorMessage(
      `aeth-devkit: ${gone ? 'devkit is no longer waiting for this review (the run ended or gave up on it); rerun setup-project --dry-run' : (e as Error).message}`,
    );
  }
}
