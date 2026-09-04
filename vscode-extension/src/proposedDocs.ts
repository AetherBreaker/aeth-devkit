import * as vscode from 'vscode';
import { SCHEME } from './consent';

/**
 * Read-only texts the CLI wrote, served as `aeth-devkit-proposed:/<id>/<side>/<title>`.
 * Both sides of the diff come from here (never the real file), so an unsaved editor
 * buffer can never shift the hunk numbering the CLI computed.
 */
export class ProposedDocs implements vscode.TextDocumentContentProvider {
  private readonly texts = new Map<string, string>();

  register(id: string, side: 'current' | 'proposed', title: string, text: string): vscode.Uri {
    const uri = vscode.Uri.from({ scheme: SCHEME, path: `/${id}/${side}/${title}` });
    this.texts.set(uri.path, text);
    return uri;
  }

  forget(id: string): void {
    for (const key of [...this.texts.keys()]) if (key.startsWith(`/${id}/`)) this.texts.delete(key);
  }

  provideTextDocumentContent(uri: vscode.Uri): string {
    return this.texts.get(uri.path) ?? '';
  }
}

/** `{ id, side }` for a document of this scheme, else undefined. Titles may contain `/`. */
export function parseUri(uri: vscode.Uri): { id: string; side: string } | undefined {
  if (uri.scheme !== SCHEME) return undefined;
  const [, id, side] = uri.path.split('/');
  return id && side ? { id, side } : undefined;
}
