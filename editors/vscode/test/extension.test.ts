import { describe, it } from "node:test";
import * as assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import Module = require("node:module");

type TestUri = { fsPath: string; scheme?: string };
type SavedDocument = { languageId: string; uri: TestUri; fileName?: string };

interface FakeState {
  workspaceFolders: Array<{ uri: { fsPath: string } }> | undefined;
  trusted: boolean;
  textDocuments: Array<{ languageId: string; isDirty?: boolean; uri?: TestUri }>;
  openListeners: Array<() => Promise<void>>;
  config: Record<string, unknown>;
  activeTextEditor:
    | {
        document: {
          uri: TestUri;
          languageId: string;
          isDirty: boolean;
          save: () => Promise<boolean>;
        };
      }
    | undefined;
  commands: Map<string, (...args: unknown[]) => unknown>;
  providers: Map<
    string,
    { provideTextDocumentContent: (uri: { path: string }) => Promise<string> }
  >;
  saveListeners: Array<(doc: SavedDocument) => Promise<void>>;
  configListeners: Array<
    (e: { affectsConfiguration: (section: string) => boolean }) => Promise<void>
  >;
  closeTerminalListeners: Array<(term: unknown) => void>;
  messages: string[];
  outputLines: string[];
  terminalTexts: string[];
  clientStarts: number;
  clientStops: number;
  notifications: Array<{ method: string; params: unknown }>;
  shownDocs: unknown[];
}

function createFakeEnv() {
  const state: FakeState = {
    workspaceFolders: undefined,
    trusted: true,
    textDocuments: [],
    openListeners: [],
    config: { executablePath: "", checkOnSave: true },
    activeTextEditor: undefined,
    commands: new Map(),
    providers: new Map(),
    saveListeners: [],
    configListeners: [],
    closeTerminalListeners: [],
    messages: [],
    outputLines: [],
    terminalTexts: [],
    clientStarts: 0,
    clientStops: 0,
    notifications: [],
    shownDocs: [],
  };

  class FakeEventEmitter<T> {
    private listeners: Array<(e: T) => void> = [];
    event = (cb: (e: T) => void) => {
      this.listeners.push(cb);
      return { dispose: () => {} };
    };
    fire(val: T): void {
      for (const l of this.listeners) {
        l(val);
      }
    }
    dispose(): void {}
  }

  class FakeLanguageClient {
    async start(): Promise<void> {
      state.clientStarts += 1;
    }
    async stop(): Promise<void> {
      state.clientStops += 1;
    }
    async sendNotification(method: string, params: unknown): Promise<void> {
      state.notifications.push({ method, params });
    }
  }

  const status = {
    text: "", tooltip: "", command: "", visible: false,
    show() { this.visible = true; },
    hide() { this.visible = false; },
    dispose() {},
  };
  const editorListeners: Array<() => void> = [];
  const vscodeMock = {
    EventEmitter: FakeEventEmitter,
    ViewColumn: { Beside: 2 },
    StatusBarAlignment: { Right: 2 },
    ConfigurationTarget: { WorkspaceFolder: 3 },
    Uri: {
      parse: (raw: string) => {
        const colon = raw.indexOf(":");
        return {
          scheme: raw.slice(0, colon),
          path: raw.slice(colon + 1),
          fsPath: raw.slice(colon + 1),
          toString: () => raw,
        };
      },
    },
    workspace: {
      get isTrusted() { return state.trusted; },
      getWorkspaceFolder: (uri: TestUri) => state.workspaceFolders?.find((f) =>
        !path.relative(f.uri.fsPath, uri.fsPath).startsWith("..")),
      get workspaceFolders() {
        return state.workspaceFolders;
      },
      get textDocuments() {
        return state.textDocuments;
      },
      onDidOpenTextDocument: (listener: () => Promise<void>) => {
        state.openListeners.push(listener);
        return { dispose: () => {} };
      },
      onDidChangeWorkspaceFolders: () => ({ dispose: () => {} }),
      getConfiguration: () => ({
        get: <T>(key: string, def: T): T =>
          key in state.config ? (state.config[key] as T) : def,
        update: async (key: string, value: boolean, target: number) => {
          assert.equal(target, 3);
          state.config[key] = value;
          for (const notify of state.configListeners) {
            await notify({ affectsConfiguration: (section) => section === `rsharp.${key}` });
          }
        },
      }),
      registerTextDocumentContentProvider: (
        scheme: string,
        provider: { provideTextDocumentContent: (uri: { path: string }) => Promise<string> },
      ) => {
        state.providers.set(scheme, provider);
        return { dispose: () => {} };
      },
      onDidSaveTextDocument: (
        cb: (doc: SavedDocument) => Promise<void>,
      ) => {
        state.saveListeners.push(cb);
        return { dispose: () => {} };
      },
      onDidChangeConfiguration: (
        cb: (e: { affectsConfiguration: (section: string) => boolean }) => Promise<void>,
      ) => {
        state.configListeners.push(cb);
        return { dispose: () => {} };
      },
      openTextDocument: async (arg: unknown) => ({ arg }),
    },
    languages: {
      setTextDocumentLanguage: async (doc: unknown, lang: string) => ({
        doc,
        lang,
      }),
    },
    window: {
      createStatusBarItem: () => status,
      onDidChangeActiveTextEditor: (cb: () => void) => {
        editorListeners.push(cb);
        return { dispose() {} };
      },
      get activeTextEditor() {
        return state.activeTextEditor;
      },
      createOutputChannel: () => ({
        appendLine: (line: string) => state.outputLines.push(line),
        show: () => {},
        dispose: () => {},
      }),
      createTerminal: (name: string) => ({
        name,
        show: () => {},
        sendText: (text: string) => {
          state.terminalTexts.push(text);
        },
      }),
      onDidCloseTerminal: (cb: (term: unknown) => void) => {
        state.closeTerminalListeners.push(cb);
        return { dispose: () => {} };
      },
      showWarningMessage: (m: string, _options?: unknown, action?: string) => {
        state.messages.push(`warn:${m}`);
        return action;
      },
      showInformationMessage: (m: string) => state.messages.push(`info:${m}`),
      showErrorMessage: (m: string) => state.messages.push(`err:${m}`),
      showTextDocument: async (doc: unknown) => {
        state.shownDocs.push(doc);
      },
    },
    commands: {
      registerCommand: (id: string, cb: (...args: unknown[]) => unknown) => {
        state.commands.set(id, cb);
        return { dispose: () => {} };
      },
    },
  };

  return {
    state,
    status,
    refreshEditor: () => editorListeners.forEach((cb) => cb()),
    vscodeMock,
    lcMock: { LanguageClient: FakeLanguageClient },
  };
}

describe("VS Code extension lifecycle and wiring", () => {
  it("activates, warns on missing binary, starts LSP, handles commands, config, and deactivates", async (t) => {
    const { state, vscodeMock, lcMock, status, refreshEditor } = createFakeEnv();
    const mod = Module as unknown as {
      _load: (request: string, parent: unknown, isMain: boolean) => unknown;
    };
    const origLoad = mod._load;

    const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "rsharp-ext-test-"));
    const fakeBin = path.join(tmpDir, "fake-rsharp");
    fs.writeFileSync(fakeBin, "");
    let convertProject!: (project: string) => Promise<void>;

    const childProcMock = {
      execFile: (
        _cmd: string,
        args: readonly string[],
        _opts: unknown,
        cb: (err: Error | null, stdout: string, stderr: string) => void,
      ) => {
        if (args[0] === "check") {
          cb(null, "OK\n", "");
        } else if (args[0] === "to-rust") {
          if (args.includes("-o")) {
            fs.writeFileSync(args[args.indexOf("-o") + 1], "fn main() {}\n");
          }
          cb(null, "fn main() {}\n", "");
        } else if (args[0] === "from-rust") {
          if (args[1] === "--project") {
            fs.writeFileSync(path.join(args[4], "main.rsharp"), "void main() {}\n");
          }
          cb(null, "void main() {}\n", "");
        } else {
          cb(new Error("unknown"), "", "unknown");
        }
      },
    };

    const savedPath = process.env.PATH;
    try {
      process.env.PATH = "";
      mod._load = function (request: string, parent: unknown, isMain: boolean) {
        if (request === "vscode") {
          return vscodeMock;
        }
        if (request === "vscode-languageclient/node") {
          return lcMock;
        }
        if (request === "child_process") {
          return childProcMock;
        }
        if (request === "./projectSuggestion") {
          return { registerProjectSuggestions: (_ctx: unknown, _out: unknown, run: typeof convertProject) => {
            convertProject = run;
          } };
        }
        return origLoad.call(this, request, parent, isMain);
      };

      const cmdPath = require.resolve("../src/commands");
      const extPath = require.resolve("../src/extension");
      delete require.cache[cmdPath];
      delete require.cache[extPath];
      const ext = require(extPath) as {
        activate: (ctx: { subscriptions: unknown[] }) => Promise<void>;
        deactivate: () => Promise<void>;
      };

      // A Rust-only window can offer conversion without starting the R# server.
      const stored = new Map<string, unknown>();
      const ctx = {
        subscriptions: [] as unknown[],
        workspaceState: {
          get: (key: string) => stored.get(key),
          update: async (key: string, value: unknown) => { stored.set(key, value); },
        },
      };
      state.textDocuments.push({ languageId: "rust" });
      await ext.activate(ctx);
      await state.configListeners[0]({ affectsConfiguration: () => true });
      assert.equal(state.messages.length, 0);
      assert.equal(state.clientStarts, 0);
      state.textDocuments.push({ languageId: "rsharp" });
      await state.openListeners[0]();
      await state.openListeners[0]();
      assert.equal(state.messages.filter((m) => m.startsWith("warn:")).length, 1);
      await ext.deactivate();
      await convertProject(tmpDir);
      assert.ok(state.messages.some((m) => m.includes("binary not found")));

      // 2. Point config at fake binary + workspace folder, trigger executablePath config change
      state.workspaceFolders = [{ uri: { fsPath: tmpDir } }];
      state.config.executablePath = fakeBin;
      for (const l of state.configListeners) {
        await l({
          affectsConfiguration: (s) => s === "rsharp.executablePath",
        });
      }
      assert.equal(state.clientStarts, 1);

      // Toggle checkOnSave -> sends LSP didChangeConfiguration notification
      state.config.checkOnSave = false;
      for (const l of state.configListeners) {
        await l({
          affectsConfiguration: (s) => s === "rsharp.checkOnSave",
        });
        await l({
          affectsConfiguration: () => false,
        });
      }
      assert.equal(state.notifications.length, 1);

      // Restart server command
      await (state.commands.get("rsharp.restartServer") as () => Promise<void>)();
      assert.equal(state.clientStops, 1);
      assert.equal(state.clientStarts, 2);

      // Active editor missing -> error path
      await (state.commands.get("rsharp.runFile") as () => Promise<void>)();
      assert.ok(state.messages.some((m) => m.startsWith("err:")));

      // Active dirty .rsharp file -> runFile creates terminal, second run reuses it, closing clears it
      let saved = false;
      state.activeTextEditor = {
        document: {
          uri: { fsPath: path.join(tmpDir, "demo.rsharp") },
          languageId: "rsharp",
          isDirty: true,
          save: async () => {
            saved = true;
            return true;
          },
        },
      };
      await (state.commands.get("rsharp.runFile") as () => Promise<void>)();
      assert.equal(saved, true);
      state.activeTextEditor.document.isDirty = false;
      await (state.commands.get("rsharp.runFile") as () => Promise<void>)();
      assert.equal(state.terminalTexts.length, 2);

      // Close unrelated terminal then close R# terminal
      for (const l of state.closeTerminalListeners) {
        l({ name: "Other" });
      }
      // Run checkFile, previewRust, preview provider, and save listener (both rsharp and non-rsharp)
      await (state.commands.get("rsharp.checkFile") as () => Promise<void>)();
      assert.ok(state.outputLines.some((l) => l.includes("[R# Check]")));
      assert.ok(state.messages.some((m) => m.startsWith("info:R# check passed:")));

      await (state.commands.get("rsharp.previewRust") as () => Promise<void>)();
      const provider = state.providers.get("rsharp-rust")!;
      const previewContent = await provider.provideTextDocumentContent({
        path: `${path.join(tmpDir, "demo.rsharp")}.rs`,
      });
      assert.match(previewContent, /fn main/);

      for (const l of state.saveListeners) {
        await l({ languageId: "rsharp", uri: { fsPath: path.join(tmpDir, "demo.rsharp") } });
        await l({ languageId: "rust", uri: { fsPath: path.join(tmpDir, "demo.rs") } });
      }

      // Close actual R# terminal by firing close with the same terminal object created via a spy
      const origCreateTerminal = vscodeMock.window.createTerminal;
      let capturedTerm: unknown;
      vscodeMock.window.createTerminal = (name: string) => {
        capturedTerm = origCreateTerminal(name);
        return capturedTerm as { name: string; show: () => void; sendText: (t: string) => void };
      };
      // Trigger another close after capturing; first clear via new activation in fresh module load
      state.activeTextEditor = {
        document: {
          uri: { fsPath: path.join(tmpDir, "lib.rs") },
          languageId: "rust",
          isDirty: false,
          save: async () => true,
        },
      };
      await (state.commands.get("rsharp.convertFromRust") as () => Promise<void>)();
      assert.ok(state.shownDocs.length >= 2);

      await t.test("the visible toggle uses folder settings and drives the R# save listener", async () => {
        const target = path.join(tmpDir, "main.rs");
        fs.writeFileSync(path.join(tmpDir, "Cargo.toml"), "[workspace]");
        fs.writeFileSync(target, "// original");
        await convertProject(tmpDir);
        const copy = fs.readdirSync(tmpDir).find((name) => name.startsWith("rsharp-"))!;
        const source = path.join(tmpDir, copy, "main.rsharp");
        const doc = { uri: { fsPath: source, scheme: "file" }, fileName: source,
          languageId: "rsharp", isDirty: false, save: async () => true };
        state.activeTextEditor = { document: doc };
        const click = () => state.commands.get("rsharp.toggleRustSync")!();
        const save = () => state.saveListeners[0](doc);
        state.trusted = false;
        refreshEditor();
        await click();
        assert.equal(status.visible, false);
        state.trusted = true;
        state.workspaceFolders = undefined;
        refreshEditor();
        await save();
        assert.equal(status.visible, false);
        state.workspaceFolders = [{ uri: { fsPath: tmpDir } }];
        refreshEditor();
        assert.equal(status.command, "rsharp.toggleRustSync");
        assert.equal(status.visible, true);
        assert.match(status.text, /Rust Sync: Off/);
        await save();
        assert.equal(fs.readFileSync(target, "utf8"), "// original");
        await click();
        assert.equal(state.config.syncRustOnSave, true);
        assert.match(status.text, /Rust Sync: On/);
        assert.match(status.tooltip, /Turn off/);
        const dirtyRust = { languageId: "rust", uri: { fsPath: target, scheme: "file" }, isDirty: true };
        state.textDocuments.push(dirtyRust);
        await save();
        assert.ok(state.messages.some((m) => m.includes("unsaved Rust edits")));
        dirtyRust.isDirty = false;
        await save();
        assert.equal(fs.readFileSync(target, "utf8"), "fn main() {}\n");
        assert.ok(state.outputLines.includes(`[R# Sync] Updated ${target}`));
        fs.writeFileSync(target, "// independent edit");
        await save();
        assert.ok(state.messages.some((m) => m.includes("changed separately")));
        assert.equal(fs.readFileSync(target, "utf8"), "fn main() {}\n");
        await click();
        assert.match(status.text, /Rust Sync: Off/);
        doc.uri.scheme = "untitled";
        refreshEditor();
        assert.equal(status.visible, false);

        ctx.workspaceState.update = async () => { throw new Error("storage unavailable"); };
        await convertProject(tmpDir);
        assert.ok(state.outputLines.some((line) => line.includes("storage unavailable")));
      });

      // Deactivate stops active client
      await ext.deactivate();
      assert.equal(state.clientStops, 2);

      // Re-activate fresh to verify exact terminal close branch
      delete require.cache[extPath];
      const ext2 = require(extPath) as {
        activate: (ctx: { subscriptions: unknown[] }) => Promise<void>;
        deactivate: () => Promise<void>;
      };
      state.closeTerminalListeners.length = 0;
      await ext2.activate({ subscriptions: [] });
      state.activeTextEditor = {
        document: {
          uri: { fsPath: path.join(tmpDir, "demo.rsharp") },
          languageId: "rsharp",
          isDirty: false,
          save: async () => true,
        },
      };
      await (state.commands.get("rsharp.runFile") as () => Promise<void>)();
      assert.ok(capturedTerm);
      for (const l of state.closeTerminalListeners) {
        l(capturedTerm);
      }
      await ext2.deactivate();
    } finally {
      mod._load = origLoad;
      if (savedPath !== undefined) {
        process.env.PATH = savedPath;
      }
      fs.rmSync(tmpDir, { recursive: true, force: true });
    }
  });
});
