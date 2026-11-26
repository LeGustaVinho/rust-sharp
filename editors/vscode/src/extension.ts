import type {
  ExtensionContext,
  FileSystemWatcher,
  OutputChannel,
  Terminal,
  TextDocumentContentProvider,
  Uri as VsCodeUri,
} from "vscode";
import { basename, relative } from "node:path";
import {
  EventEmitter,
  Uri,
  ViewColumn,
  commands,
  languages,
  window,
  workspace,
} from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";
import { defaultBinaryOptions, resolveRSharpBinary } from "./binary";
import { registerProjectSuggestions } from "./projectSuggestion";
import { RustSourceSync } from "./rustSync";
import { registerSyncStatus } from "./syncStatus";
import {
  RUST_PREVIEW_SCHEME,
  RustPreviewModel,
  createCommandHandlers,
  defaultExecFile,
  makePreviewUriString,
  runCommandProcess,
} from "./commands";

let client: LanguageClient | undefined;
let terminal: Terminal | undefined;

function getWorkspaceRoots(): string[] {
  const folders = workspace.workspaceFolders;
  if (!folders) {
    return [];
  }
  return folders.map((f) => f.uri.fsPath);
}

function getConfiguredBinaryPath(): string {
  return workspace
    .getConfiguration("rsharp")
    .get<string>("executablePath", "");
}

function getCheckOnSave(): boolean {
  return workspace
    .getConfiguration("rsharp")
    .get<boolean>("checkOnSave", true);
}

function resolveCurrentBinary(): string | undefined {
  return resolveRSharpBinary(
    defaultBinaryOptions(getConfiguredBinaryPath(), getWorkspaceRoots()),
  );
}

async function startLanguageClient(
  outputChannel: OutputChannel,
  files: FileSystemWatcher,
): Promise<void> {
  const bin = resolveCurrentBinary();
  if (!bin) {
    void window.showWarningMessage(
      "R# language server binary (`rsharp`) was not found. Run `cargo build --release -p rsharp-cli` or configure `rsharp.executablePath`.",
    );
    return;
  }

  const serverOptions: ServerOptions = {
    command: bin,
    args: ["lsp"],
  };
  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "rsharp" }, { scheme: "untitled", language: "rsharp" }],
    synchronize: { fileEvents: files },
    outputChannel,
    initializationOptions: {
      checkOnSave: getCheckOnSave(),
    },
  };

  client = new LanguageClient(
    "rsharp",
    "R# Language Server",
    serverOptions,
    clientOptions,
  );
  await client.start();
}

async function stopLanguageClient(): Promise<void> {
  if (!client) {
    return;
  }
  const active = client;
  client = undefined;
  await active.stop();
}

export async function activate(context: ExtensionContext): Promise<void> {
  const outputChannel = window.createOutputChannel("R#");
  const files = workspace.createFileSystemWatcher("**/*.rsharp");
  context.subscriptions.push(outputChannel, files);

  const execRunner = defaultExecFile();
  const runProc = (cmd: string, args: readonly string[]) =>
    runCommandProcess(cmd, args, execRunner);
  const rustSync = new RustSourceSync({
    state: context.workspaceState,
    resolveBinary: resolveCurrentBinary,
    runProcess: runProc,
    isDirty: (target) => workspace.textDocuments.some((doc) =>
      doc.isDirty && doc.uri.scheme === "file" && relative(doc.uri.fsPath, target) === "",
    ),
    confirm: async (message, action) =>
      await window.showWarningMessage(message, { modal: true }, action) === action,
  });

  const comparisons = new Map<string, string>();
  const comparisonChanges = new EventEmitter<VsCodeUri>();
  const comparisonScheme = "rsharp-sync-preview";
  context.subscriptions.push(
    comparisonChanges,
    workspace.registerTextDocumentContentProvider(comparisonScheme, {
      onDidChange: comparisonChanges.event,
      provideTextDocumentContent: (uri) => comparisons.get(uri.toString()),
    }),
    workspace.onDidCloseTextDocument((doc) => comparisons.delete(doc.uri.toString())),
    commands.registerCommand("rsharp.compareOriginalRust", async () => {
      const doc = window.activeTextEditor?.document;
      const folder = doc?.languageId === "rsharp" && doc.uri.scheme === "file" && workspace.getWorkspaceFolder(doc.uri);
      const target = folder && rustSync.originalForComparison(folder.uri.fsPath, doc.uri.fsPath);
      if (!target) {
        void window.showInformationMessage("Open an R# file linked to an original Rust file in this workspace.");
        return;
      }
      if (!client) {
        void window.showErrorMessage("Start the R# language server to compare with the original Rust.");
        return;
      }
      if (workspace.textDocuments.some((d) => d.isDirty && d.uri.scheme === "file" && relative(d.uri.fsPath, target) === "")) {
        void window.showInformationMessage("Save or revert the Rust editor changes before comparing with the original file on disk.");
        return;
      }
      try {
        const generated = await client.sendRequest<string>("rsharp/rustForSync", { text: doc.getText() });
        const original = Uri.file(target);
        const preview = original.with({ scheme: comparisonScheme, query: doc.uri.toString() });
        comparisons.set(preview.toString(), generated);
        comparisonChanges.fire(preview);
        await commands.executeCommand("vscode.diff", original, preview, `${basename(target)} — Original ↔ Rust Sync`);
      } catch (error) {
        void window.showErrorMessage(`Could not compare the original Rust: ${error}`);
      }
    }),
  );

  const previewEmitter = new EventEmitter<VsCodeUri>();
  const previewModel = new RustPreviewModel({
    resolveBinary: resolveCurrentBinary,
    runProcess: runProc,
  });
  const previewProvider: TextDocumentContentProvider = {
    onDidChange: previewEmitter.event,
    provideTextDocumentContent: (uri) => previewModel.loadPreview(uri.path),
  };

  context.subscriptions.push(
    previewEmitter,
    workspace.registerTextDocumentContentProvider(
      RUST_PREVIEW_SCHEME,
      previewProvider,
    ),
    workspace.onDidSaveTextDocument(async (doc) => {
      if (doc.languageId !== "rsharp") {
        return;
      }
      previewEmitter.fire(Uri.parse(makePreviewUriString(doc.uri.fsPath)));
      const folder = doc.uri.scheme === "file" && workspace.getWorkspaceFolder(doc.uri);
      if (!folder) {
        return;
      }
      const enabled = () => workspace.isTrusted &&
        workspace.getWorkspaceFolder(doc.uri)?.uri.fsPath === folder.uri.fsPath &&
        workspace.getConfiguration("rsharp", doc.uri).get<boolean>("syncRustOnSave", false);
      try {
        const target = await rustSync.save(folder.uri.fsPath, doc.uri.fsPath, enabled);
        if (target) {
          outputChannel.appendLine(`[R# Sync] Updated ${target}`);
        }
      } catch (error) {
        outputChannel.appendLine(`[R# Sync] ${error}`);
        void window.showErrorMessage(`Rust sync for "${doc.fileName}": ${error}`);
      }
    }),
    window.onDidCloseTerminal((closed) => {
      if (closed === terminal) {
        terminal = undefined;
      }
    }),
  );

  const handlers = createCommandHandlers({
    getActiveFile: () => {
      const editor = window.activeTextEditor;
      if (!editor) {
        return undefined;
      }
      return {
        filePath: editor.document.uri.fsPath,
        languageId: editor.document.languageId,
        isDirty: editor.document.isDirty,
        save: async () => editor.document.save(),
      };
    },
    resolveBinary: resolveCurrentBinary,
    runProcess: runProc,
    runInTerminal: (commandLine) => {
      if (!terminal) {
        terminal = window.createTerminal("R#");
      }
      terminal.show(true);
      terminal.sendText(commandLine);
    },
    showOutput: (title, text) => {
      outputChannel.appendLine(`[${title}] ${text}`);
      outputChannel.show(true);
    },
    showInfo: (message) => {
      void window.showInformationMessage(message);
    },
    showError: (message) => {
      void window.showErrorMessage(message);
    },
    openRustPreview: async (sourcePath) => {
      const uri = Uri.parse(makePreviewUriString(sourcePath));
      previewEmitter.fire(uri);
      const doc = await workspace.openTextDocument(uri);
      await languages.setTextDocumentLanguage(doc, "rust");
      await window.showTextDocument(doc, {
        viewColumn: ViewColumn.Beside,
        preview: true,
        preserveFocus: true,
      });
    },
    openUntitledRSharp: async (content) => {
      const doc = await workspace.openTextDocument({
        language: "rsharp",
        content,
      });
      await window.showTextDocument(doc);
    },
  });

  let serverRequested = false;
  const restartServer = async () => {
    serverRequested = true;
    await stopLanguageClient();
    await startLanguageClient(outputChannel, files);
  };
  const startForOpenDocuments = async () => {
    if (
      !serverRequested &&
      workspace.textDocuments.some((doc) => doc.languageId === "rsharp")
    ) {
      await restartServer();
    }
  };

  context.subscriptions.push(
    commands.registerCommand("rsharp.runFile", handlers.runFile),
    commands.registerCommand("rsharp.checkFile", handlers.checkFile),
    commands.registerCommand("rsharp.previewRust", handlers.previewRust),
    commands.registerCommand(
      "rsharp.convertFromRust",
      handlers.convertFromRust,
    ),
    commands.registerCommand("rsharp.restartServer", restartServer),
    workspace.onDidOpenTextDocument(startForOpenDocuments),
    workspace.onDidChangeConfiguration(async (event) => {
      if (event.affectsConfiguration("rsharp.executablePath") && serverRequested) {
        await restartServer();
      } else if (event.affectsConfiguration("rsharp.checkOnSave") && client) {
        await client.sendNotification("workspace/didChangeConfiguration", {
          settings: { rsharp: { checkOnSave: getCheckOnSave() } },
        });
      }
    }),
  );

  registerSyncStatus(context);
  registerProjectSuggestions(context, outputChannel, async (project) => {
    const copy = await handlers.convertProject(project);
    if (copy) {
      try {
        await rustSync.rememberProject(copy);
      } catch (error) {
        outputChannel.appendLine(`[R# Sync] Could not record the original Rust files for "${copy}": ${error}`);
      }
    }
  });
  await startForOpenDocuments();
}

export async function deactivate(): Promise<void> {
  await stopLanguageClient();
}
