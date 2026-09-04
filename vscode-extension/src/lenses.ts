import * as vscode from 'vscode';
import type { Session } from './consent';
import { parseUri } from './proposedDocs';

/**
 * Line 0: the whole-file decisions. Above each hunk: its accept/reject toggle showing the
 * current state. Only the proposed (right-hand) document gets lenses.
 */
export class ConsentLenses implements vscode.CodeLensProvider {
  private readonly emitter = new vscode.EventEmitter<void>();
  readonly onDidChangeCodeLenses = this.emitter.event;

  constructor(private readonly session: (id: string) => Session | undefined) {}

  refresh(): void {
    this.emitter.fire();
  }

  provideCodeLenses(doc: vscode.TextDocument): vscode.CodeLens[] {
    const at = parseUri(doc.uri);
    if (!at || at.side !== 'proposed') return [];
    const s = this.session(at.id);
    if (!s) return [];
    const top = new vscode.Range(0, 0, 0, 0);
    const m = s.req.hunks.length;
    const lenses = [
      new vscode.CodeLens(top, {
        title: `$(check-all) Apply accepted (${s.state.acceptedCount} of ${m})`,
        command: 'aeth-devkit.applyAccepted',
        arguments: [at.id],
      }),
      new vscode.CodeLens(top, { title: 'Accept all hunks', command: 'aeth-devkit.acceptAllHunks', arguments: [at.id] }),
    ];
    if (s.req.offer_replace_all) {
      lenses.push(
        new vscode.CodeLens(top, { title: 'Replace all (rest of this run)', command: 'aeth-devkit.replaceAll', arguments: [at.id] }),
      );
    }
    lenses.push(new vscode.CodeLens(top, { title: 'Keep file', command: 'aeth-devkit.keepFile', arguments: [at.id] }));
    s.req.hunks.forEach((h, i) => {
      // A pure deletion has an empty proposed range at the end; clamp into the document.
      const line = Math.min(h.proposed[0], Math.max(doc.lineCount - 1, 0));
      const on = s.state.accepted[i];
      lenses.push(
        new vscode.CodeLens(new vscode.Range(line, 0, line, 0), {
          title: on ? `$(check) Hunk ${i + 1} accepted — reject` : `$(x) Hunk ${i + 1} rejected — accept`,
          command: on ? 'aeth-devkit.rejectHunk' : 'aeth-devkit.acceptHunk',
          arguments: [at.id, i],
        }),
      );
    });
    return lenses;
  }
}
