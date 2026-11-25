import { describe, it } from "node:test";
import * as assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import Module = require("node:module");

interface FakeState {
  workspaceFolders: Array<{ uri: { fsPath: string } }> | undefined;
  config: Record<string, unknown>;
  activeTextEditor:
    | {
        document: {
          uri: { fsPath: string };
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
  saveListeners: Array<(doc: { languageId: string; uri: { fsPath: string } }) => void>;
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

  const vscodeMock = {
    EventEmitter: FakeEventEmitter,
    ViewColumn: { Beside: 2 },
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
      get workspaceFolders() {
        return state.workspaceFolders;
      },
      getConfiguration: () => ({
        get: <T>(key: string, def: T): T =>
          key in state.config ? (state.config[key] as T) : def,
      }),
      registerTextDocumentContentProvider: (
        scheme: string,
        provider: { provideTextDocumentContent: (uri: { path: string }) => Promise<string> },
      ) => {
        state.providers.set(scheme, provider);
        return { dispose: () => {} };
      },
      onDidSaveTextDocument: (
        cb: (doc: { languageId: string; uri: { fsPath: string } }) => void,
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
      showWarningMessage: (m: string) => state.messages.push(`warn:${m}`),
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
    vscodeMock,
    lcMock: { LanguageClient: FakeLanguageClient },
  };
}

describe("VS Code extension lifecycle and wiring", () => {
  it("activates, warns on missing binary, starts LSP, handles commands, config, and deactivates", async () => {
    const { state, vscodeMock, lcMock } = createFakeEnv();
    const mod = Module as unknown as {
      _load: (request: string, parent: unknown, isMain: boolean) => unknown;
    };
    const origLoad = mod._load;

    const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "rsharp-ext-test-"));
    const fakeBin = path.join(tmpDir, "fake-rsharp");
    fs.writeFileSync(fakeBin, "");

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
          cb(null, "fn main() {}\n", "");
        } else if (args[0] === "from-rust") {
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

      // 1. Activate without binary -> warning emitted, deactivate is a no-op
      const ctx = { subscriptions: [] as unknown[] };
      await ext.activate(ctx);
      assert.ok(state.messages.some((m) => m.startsWith("warn:")));
      assert.equal(state.clientStarts, 0);
      await ext.deactivate();

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
        l({ languageId: "rsharp", uri: { fsPath: path.join(tmpDir, "demo.rsharp") } });
        l({ languageId: "rust", uri: { fsPath: path.join(tmpDir, "demo.rs") } });
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
