import { statSync } from "node:fs";
import { join } from "node:path";
import type { ExtensionContext, OutputChannel, WorkspaceFolder } from "vscode";
import { ProgressLocation, RelativePattern, window, workspace } from "vscode";

const excludedDirectories =
  "**/{.*,target,node_modules,vendor,dist,build,pkg,out,obj,worktrees,third_party}/**";
const convertAction = "Convert to R#";
const muteAction = "Don't Ask Again Here";

export function registerProjectSuggestions(
  context: ExtensionContext,
  output: OutputChannel,
  convertProject: (projectDir: string) => Promise<void>,
): void {
  const offered = new Set<string>();

  async function suggest(folder: WorkspaceFolder): Promise<void> {
    const folderId = folder.uri.toString();
    const key = `rsharp.muteConversion:${folderId}`;
    if (
      !workspace.isTrusted ||
      folder.uri.scheme !== "file" ||
      context.workspaceState.get<boolean>(key)
    ) {
      return;
    }
    const manifest = statSync(join(folder.uri.fsPath, "Cargo.toml"), { throwIfNoEntry: false });
    if (!manifest?.isFile()) {
      return;
    }

    const [rust, rsharp] = await Promise.all(
      ["**/*.rs", "**/*.rsharp"].map((pattern) =>
        workspace.findFiles(new RelativePattern(folder, pattern), excludedDirectories, 1),
      ),
    );
    if (!rust.length || rsharp.length || offered.has(folderId)) {
      return;
    }
    offered.add(folderId);
    const choice = await window.showInformationMessage(
      `Convert the saved Rust sources in "${folder.name}" to R#? The converted files will go in a new subfolder.`,
      convertAction,
      muteAction,
    );
    if (choice === muteAction) {
      await context.workspaceState.update(key, true);
    }
    if (
      choice !== convertAction ||
      !workspace.isTrusted ||
      workspace.getWorkspaceFolder(folder.uri)?.uri.toString() !== folderId
    ) {
      return;
    }
    try {
      await window.withProgress(
        { location: ProgressLocation.Notification, title: `Converting "${folder.name}" to R#` },
        () => convertProject(folder.uri.fsPath),
      );
    } catch (error) {
      output.appendLine(`[R# Convert] ${error}`);
      output.show(true);
      void window.showErrorMessage(`Failed to convert "${folder.name}". See the R# Output panel.`);
    }
  }

  function checkFolders(folders: readonly WorkspaceFolder[]) {
    return Promise.all(folders.map((folder) =>
      suggest(folder).catch((error) => {
        output.appendLine(`[R#] Could not check "${folder.name}" for conversion: ${error}`);
      }),
    ));
  }

  context.subscriptions.push(
    workspace.onDidChangeWorkspaceFolders(({ added }) => checkFolders(added)),
  );
  void checkFolders(workspace.workspaceFolders ?? []);
}
