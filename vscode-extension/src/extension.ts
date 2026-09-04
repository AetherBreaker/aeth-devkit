import * as fs from 'node:fs';
import * as os from 'node:os';
import * as vscode from 'vscode';
import {
  HunkState,
  PROTOCOL,
  Request,
  Response,
  SCHEME,
  Session,
  cacheDir,
  cancelPath,
  parseRequest,
  requestPath,
  writeResponse,
} from './consent';
import { ConsentLenses } from './lenses';
import { ProposedDocs, parseUri } from './proposedDocs';
import { openReview } from './review';
import { addToRuntimeBaseClasses } from './runtimeBaseClasses';

interface OpenSession extends Session {
  proposed: vscode.Uri;
  /** Polls for the CLI's `<id>.cancel` marker (Ctrl-C in the terminal). */
  cancelPoll: NodeJS.Timeout;
}

const sessions = new Map<string, OpenSession>();
const docs = new ProposedDocs();
const lenses = new ConsentLenses((id) => sessions.get(id));

export function activate(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.workspace.registerTextDocumentContentProvider(SCHEME, docs),
    vscode.languages.registerCodeLensProvider({ scheme: SCHEME }, lenses),
    vscode.window.registerUriHandler({ handleUri }),
    vscode.window.tabGroups.onDidChangeTabs(onTabsChanged),
    vscode.commands.registerCommand('aeth-devkit.acceptHunk', (id: string, i: number) => setHunk(id, i, true)),
    vscode.commands.registerCommand('aeth-devkit.rejectHunk', (id: string, i: number) => setHunk(id, i, false)),
    vscode.commands.registerCommand('aeth-devkit.acceptAllHunks', (id: string) => {
      sessions.get(id)?.state.acceptAll();
      lenses.refresh();
    }),
    vscode.commands.registerCommand('aeth-devkit.applyAccepted', (id: string) => {
      const s = sessions.get(id);
      if (s) void decide(s, s.state.response());
    }),
    vscode.commands.registerCommand('aeth-devkit.replaceFile', (arg?: unknown) =>
      withSession(arg, (s) => decide(s, { decision: 'replace' })),
    ),
    vscode.commands.registerCommand('aeth-devkit.replaceAll', (arg?: unknown) =>
      withSession(arg, (s) => decide(s, { decision: 'replace_all' })),
    ),
    vscode.commands.registerCommand('aeth-devkit.keepFile', (arg?: unknown) =>
      withSession(arg, (s) => decide(s, { decision: 'keep' })),
    ),
    vscode.commands.registerCommand('aeth-devkit.addToRuntimeBaseClasses', addToRuntimeBaseClasses),
  );
}

export function deactivate(): void {
  for (const s of sessions.values()) clearInterval(s.cancelPoll);
}

function fail(message: string): void {
  void vscode.window.showErrorMessage(`aeth-devkit: ${message}`);
}

async function handleUri(uri: vscode.Uri): Promise<void> {
  // The URL carries only an id; the file lives where this extension expects devkit's
  // cache, so a link from anywhere else can name nothing outside that folder.
  const id = new URLSearchParams(uri.query).get('id') ?? '';
  const cache = cacheDir(process.env, process.platform, os.homedir());
  if (!cache) return fail('cannot locate the devkit cache directory');
  let file: string;
  try {
    file = requestPath(cache, id);
  } catch (e) {
    return fail((e as Error).message);
  }
  if (uri.path === '/review') return openReview(file, cache, docs);
  if (uri.path !== '/consent') return;
  let req: Request;
  try {
    req = parseRequest(fs.readFileSync(file, 'utf8'), cache);
  } catch (e) {
    return fail((e as Error).message);
  }
  if (req.protocol !== PROTOCOL) {
    writeResponse(req.response_path, {
      decision: 'error',
      message: `the extension speaks protocol ${PROTOCOL}, devkit sent ${req.protocol}; update one of them`,
    });
    return;
  }
  await ensureDiffCodeLens();
  await vscode.commands.executeCommand('setContext', 'aeth-devkit.contentMenu', req.content_menu);
  const current = docs.register(req.id, 'current', req.title, fs.readFileSync(req.current_path, 'utf8'));
  const proposed = docs.register(req.id, 'proposed', req.title, fs.readFileSync(req.proposed_path, 'utf8'));
  const s: OpenSession = {
    req,
    state: new HunkState(req.hunks.length),
    answered: false,
    proposed,
    cancelPoll: setInterval(() => {
      if (fs.existsSync(cancelPath(req))) {
        s.answered = true;
        void closeTab(s);
      }
    }, 250),
  };
  sessions.set(req.id, s);
  await vscode.commands.executeCommand('vscode.diff', current, proposed, `devkit: ${req.title}`, { preview: false });
}

/** `diffEditor.codeLens` is off by default; without it the per-hunk lenses never show. */
async function ensureDiffCodeLens(): Promise<void> {
  const cfg = vscode.workspace.getConfiguration('diffEditor');
  const info = cfg.inspect<boolean>('codeLens');
  if (info?.globalValue === undefined && info?.workspaceValue === undefined) {
    await cfg.update('codeLens', true, vscode.ConfigurationTarget.Global);
    return;
  }
  if (cfg.get<boolean>('codeLens') === false) {
    void vscode.window.showWarningMessage(
      'aeth-devkit: diffEditor.codeLens is off, so per-hunk Accept/Reject is hidden; the whole-file buttons still work.',
    );
  }
}

function setHunk(id: string, i: number, on: boolean): void {
  sessions.get(id)?.state.set(i, on);
  lenses.refresh();
}

/** Title/content buttons receive the resource URI; the palette gives nothing. */
function withSession(arg: unknown, f: (s: OpenSession) => Promise<void>): void {
  const uri = arg instanceof vscode.Uri ? arg : vscode.window.activeTextEditor?.document.uri;
  const id = uri ? parseUri(uri)?.id : undefined;
  const s = id ? sessions.get(id) : sessions.size === 1 ? [...sessions.values()][0] : undefined;
  if (s) void f(s);
  else void vscode.window.showWarningMessage('aeth-devkit: no open consent diff.');
}

async function decide(s: OpenSession, r: Response): Promise<void> {
  s.answered = true;
  writeResponse(s.req.response_path, r);
  await closeTab(s);
}

async function closeTab(s: OpenSession): Promise<void> {
  clearInterval(s.cancelPoll);
  sessions.delete(s.req.id);
  docs.forget(s.req.id);
  lenses.refresh();
  for (const group of vscode.window.tabGroups.all) {
    for (const tab of group.tabs) {
      if (tab.input instanceof vscode.TabInputTextDiff && tab.input.modified.toString() === s.proposed.toString()) {
        await vscode.window.tabGroups.close(tab);
      }
    }
  }
}

/** A diff closed by the user (not by `decide`/`closeTab`) is a dismissal. */
function onTabsChanged(e: vscode.TabChangeEvent): void {
  for (const tab of e.closed) {
    if (!(tab.input instanceof vscode.TabInputTextDiff)) continue;
    const at = parseUri(tab.input.modified);
    const s = at ? sessions.get(at.id) : undefined;
    if (s && !s.answered) {
      s.answered = true;
      writeResponse(s.req.response_path, { decision: 'dismissed' });
      void closeTab(s);
    }
  }
}
