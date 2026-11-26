import type { ExtensionContext } from "vscode";
import { commands, ConfigurationTarget, StatusBarAlignment, window, workspace } from "vscode";
import { findConvertedFolder } from "./rustSync";

export function registerSyncStatus(context: ExtensionContext): void {
  const status = window.createStatusBarItem("rsharp.syncRustOnSave", StatusBarAlignment.Right);
  status.name = "R# Rust Sync on Save";
  status.command = "rsharp.toggleRustSync";

  function activeSettings() {
    const doc = window.activeTextEditor?.document;
    if (!workspace.isTrusted || doc?.languageId !== "rsharp" || doc.uri.scheme !== "file") {
      return;
    }
    const folder = workspace.getWorkspaceFolder(doc.uri);
    if (folder && findConvertedFolder(folder.uri.fsPath, doc.uri.fsPath)) {
      return workspace.getConfiguration("rsharp", doc.uri);
    }
    return undefined;
  }

  function refresh() {
    const settings = activeSettings();
    void commands.executeCommand("setContext", "rsharp.hasOriginalProject", Boolean(settings));
    if (!settings) {
      status.hide();
      return;
    }
    const enabled = settings.get<boolean>("syncRustOnSave", false);
    status.text = enabled ? "$(sync) Rust Sync: On" : "$(circle-slash) Rust Sync: Off";
    status.tooltip = enabled
      ? "Turn off updates to the original Rust files when saving R# in this workspace folder"
      : "Turn on updates to the original Rust files when saving R# in this workspace folder";
    status.show();
  }

  context.subscriptions.push(
    status,
    commands.registerCommand(status.command, async () => {
      const settings = activeSettings();
      if (settings) {
        await settings.update(
          "syncRustOnSave",
          !settings.get<boolean>("syncRustOnSave", false),
          ConfigurationTarget.WorkspaceFolder,
        );
      }
    }),
    window.onDidChangeActiveTextEditor(refresh),
    workspace.onDidChangeWorkspaceFolders(refresh),
    workspace.onDidChangeConfiguration((event) => {
      if (event.affectsConfiguration("rsharp.syncRustOnSave")) {
        refresh();
      }
    }),
  );
  refresh();
}
