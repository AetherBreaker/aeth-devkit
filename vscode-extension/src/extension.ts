import * as vscode from 'vscode';
import { addToRuntimeBaseClasses } from './runtimeBaseClasses';

export function activate(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.commands.registerCommand('aeth-devkit.addToRuntimeBaseClasses', addToRuntimeBaseClasses),
  );
}

export function deactivate(): void {}
