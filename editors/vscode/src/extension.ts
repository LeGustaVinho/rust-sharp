import type {
  ExtensionContext,
  OutputChannel,
  Terminal,
  TextDocumentContentProvider,
  Uri as VsCodeUri,
} from "vscode";
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
    documentSelector: [{ scheme: "file", language: "rsharp" }],
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
  context.subscriptions.push(outputChannel);

  const execRunner = defaultExecFile();
  const runProc = (cmd: string, args: readonly string[]) =>
    runCommandProcess(cmd, args, execRunner);

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
    workspace.onDidSaveTextDocument((doc) => {
      if (doc.languageId === "rsharp") {
        previewEmitter.fire(Uri.parse(makePreviewUriString(doc.uri.fsPath)));
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
    await startLanguageClient(outputChannel);
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

  registerProjectSuggestions(context, outputChannel, handlers.convertProject);
  await startForOpenDocuments();
}

export async function deactivate(): Promise<void> {
  await stopLanguageClient();
}
