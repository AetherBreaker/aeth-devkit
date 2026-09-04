import * as fs from 'node:fs';
import * as os from 'node:os';
import * as vscode from 'vscode';
import {
  EXTENSION_ID,
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
const log = vscode.window.createOutputChannel('devkit');
/** Rejected hunks are dimmed in the proposed document, independent of lens repaint. */
const rejected = vscode.window.createTextEditorDecorationType({
  isWholeLine: true,
  opacity: '0.45',
  textDecoration: 'line-through',
});
let status: vscode.StatusBarItem;

export function activate(context: vscode.ExtensionContext): void {
  status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
  status.command = 'aeth-devkit.applyAccepted';
  context.subscriptions.push(
    log,
    rejected,
    status,
    vscode.workspace.registerTextDocumentContentProvider(SCHEME, docs),
    vscode.languages.registerCodeLensProvider({ scheme: SCHEME }, lenses),
    vscode.window.registerUriHandler({ handleUri }),
    vscode.window.tabGroups.onDidChangeTabs(onTabsChanged),
    vscode.window.onDidChangeVisibleTextEditors(() => sessions.forEach(render)),
    vscode.commands.registerCommand('aeth-devkit.acceptHunk', (id: string, i: number) => setHunk(id, i, true)),
    vscode.commands.registerCommand('aeth-devkit.rejectHunk', (id: string, i: number) => setHunk(id, i, false)),
    vscode.commands.registerCommand('aeth-devkit.acceptAllHunks', (arg?: unknown) =>
      withSession(arg, async (s) => {
        s.state.acceptAll();
        render(s);
      }),
    ),
    vscode.commands.registerCommand('aeth-devkit.applyAccepted', (arg?: unknown) =>
      withSession(arg, (s) => decide(s, s.state.response())),
    ),
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
  log.appendLine(`error: ${message}`);
  void vscode.window.showErrorMessage(`aeth-devkit: ${message}`);
}

/**
 * Whether the floating `editor/content` buttons can show. VS Code strips proposals it
 * has not enabled from the manifest it loads, so the loaded manifest is the truth about
 * *this* window; the CLI's `content_menu` only says the argv.json entry exists. Both must
 * hold, so a grant that still needs a restart falls back to the title icons.
 */
function contentMenuLive(req: Request): boolean {
  const proposals = vscode.extensions.getExtension(EXTENSION_ID)?.packageJSON?.enabledApiProposals as string[] | undefined;
  const live = req.content_menu && (proposals ?? []).includes('contribEditorContentMenu');
  log.appendLine(`content menu: cli=${req.content_menu} manifest=${JSON.stringify(proposals)} -> ${live}`);
  return live;
}

async function handleUri(uri: vscode.Uri): Promise<void> {
  log.appendLine(`uri: ${uri.toString()}`);
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
  await vscode.commands.executeCommand('setContext', 'aeth-devkit.contentMenu', contentMenuLive(req));
  await vscode.commands.executeCommand('setContext', 'aeth-devkit.offerReplaceAll', req.offer_replace_all);
  const current = docs.register(req.id, 'current', req.title, fs.readFileSync(req.current_path, 'utf8'));
  const proposed = docs.register(req.id, 'proposed', req.title, fs.readFileSync(req.proposed_path, 'utf8'));
  const s: OpenSession = {
    req,
    state: new HunkState(req.hunks.length),
    answered: false,
    proposed,
    cancelPoll: setInterval(() => {
      if (fs.existsSync(cancelPath(req))) {
        log.appendLine(`${req.id}: cancelled by the CLI`);
        s.answered = true;
        void closeTab(s);
      }
    }, 250),
  };
  sessions.set(req.id, s);
  await vscode.commands.executeCommand('vscode.diff', current, proposed, `devkit: ${req.title}`, { preview: false });
  render(s);
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
  log.appendLine(`${id}: hunk ${i} ${on ? 'accepted' : 'rejected'}`);
  const s = sessions.get(id);
  if (!s) return;
  s.state.set(i, on);
  render(s);
}

/** Lenses, the dimming of rejected hunks, and the status bar count, from one state. */
function render(s: OpenSession): void {
  lenses.refresh();
  const ranges = s.req.hunks
    .filter((_, i) => !s.state.accepted[i])
    .filter((h) => h.proposed[1] > h.proposed[0])
    .map((h) => new vscode.Range(h.proposed[0], 0, h.proposed[1] - 1, Number.MAX_SAFE_INTEGER));
  for (const editor of vscode.window.visibleTextEditors) {
    if (editor.document.uri.toString() === s.proposed.toString()) editor.setDecorations(rejected, ranges);
  }
  const m = s.req.hunks.length;
  status.text = `$(diff) devkit: ${s.state.acceptedCount} of ${m} hunks accepted`;
  status.tooltip = `${s.req.title} — click to apply the accepted hunks`;
  status.show();
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
  log.appendLine(`${s.req.id}: ${JSON.stringify(r)}`);
  s.answered = true;
  writeResponse(s.req.response_path, r);
  await closeTab(s);
}

async function closeTab(s: OpenSession): Promise<void> {
  clearInterval(s.cancelPoll);
  sessions.delete(s.req.id);
  docs.forget(s.req.id);
  lenses.refresh();
  if (sessions.size === 0) status.hide();
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
      log.appendLine(`${s.req.id}: dismissed`);
      s.answered = true;
      writeResponse(s.req.response_path, { decision: 'dismissed' });
      void closeTab(s);
    }
  }
}
