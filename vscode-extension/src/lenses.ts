import * as vscode from 'vscode';
import type { Session } from './consent';
import { parseUri } from './proposedDocs';

/**
 * Above each hunk of the proposed (right-hand) document: an `Accept` and a `Reject`
 * lens, the chosen one marked. Whole-file actions are editor buttons, not lenses.
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
    return s.req.hunks.flatMap((h, i) => {
      // A pure deletion has an empty proposed range at the end; clamp into the document.
      const line = Math.min(h.proposed[0], Math.max(doc.lineCount - 1, 0));
      const range = new vscode.Range(line, 0, line, 0);
      const on = s.state.accepted[i];
      return [
        new vscode.CodeLens(range, {
          title: on ? `$(check) Hunk ${i + 1}: accepted` : `Accept hunk ${i + 1}`,
          command: 'aeth-devkit.acceptHunk',
          arguments: [at.id, i],
        }),
        new vscode.CodeLens(range, {
          title: on ? `Reject hunk ${i + 1}` : `$(x) Hunk ${i + 1}: rejected`,
          command: 'aeth-devkit.rejectHunk',
          arguments: [at.id, i],
        }),
      ];
    });
  }
}
