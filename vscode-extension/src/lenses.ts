import * as vscode from 'vscode';
import { rightPanelLines, type Session } from './consent';
import { parseUri } from './proposedDocs';

/**
 * Above each hunk of the right-hand panel: `Accept` / `Reject` while undecided, then the
 * outcome as plain text plus `Undo`. Whole-file actions are editor buttons, not lenses.
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
    const lines = rightPanelLines(s.req.hunks, s.state);
    return s.req.hunks.flatMap((_, i) => {
      // A pure deletion has an empty range at the end; clamp into the document.
      const line = Math.min(lines[i], Math.max(doc.lineCount - 1, 0));
      const range = new vscode.Range(line, 0, line, 0);
      const n = i + 1;
      const d = s.state.decisions[i];
      if (d === undefined) {
        return [
          new vscode.CodeLens(range, { title: `$(check) Accept hunk ${n}`, command: 'aeth-devkit.acceptHunk', arguments: [at.id, i] }),
          new vscode.CodeLens(range, { title: `$(x) Reject hunk ${n}`, command: 'aeth-devkit.rejectHunk', arguments: [at.id, i] }),
        ];
      }
      return [
        new vscode.CodeLens(range, {
          title: d === 'accept' ? `$(check) Hunk ${n} accepted` : `$(x) Hunk ${n} rejected`,
          command: '',
        }),
        new vscode.CodeLens(range, { title: 'Undo', command: 'aeth-devkit.undoHunk', arguments: [at.id, i] }),
      ];
    });
  }
}
