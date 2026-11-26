import { it, type TestContext } from "node:test";
import * as assert from "node:assert/strict";
import * as fs from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import Module = require("node:module");
import type { ExtensionContext, OutputChannel } from "vscode";

function folderAt(dir: string, name: string, scheme = "file") {
  const fsPath = join(dir, name);
  fs.mkdirSync(fsPath);
  return { name, uri: { scheme, fsPath, toString: () => `${scheme}:${fsPath}` } };
}
type Folder = ReturnType<typeof folderAt>;

function fixture(t: TestContext) {
  const dir = fs.mkdtempSync(join(tmpdir(), "rsharp-suggestion-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const project = folderAt(dir, "Cargo project");
  fs.writeFileSync(join(project.uri.fsPath, "Cargo.toml"), "[workspace]");
  fs.writeFileSync(join(project.uri.fsPath, "main.rs"), "fn main() {}");
  const preferences = new Map<string, boolean>();
  const prompts: string[] = [];
  const conversions: string[] = [];
  const errors: string[] = [];
  const output: string[] = [];
  let choice: string | undefined;
  let onFoldersChanged: (event: { added: Folder[] }) => Promise<unknown>;

  class RelativePattern {
    constructor(readonly base: Folder, readonly pattern: string) {}
  }
  const workspace = {
    isTrusted: true,
    workspaceFolders: [project] as Folder[] | undefined,
    getWorkspaceFolder: (uri: Folder["uri"]) => workspace.workspaceFolders?.find((f) => f.uri === uri),
    onDidChangeWorkspaceFolders: (cb: typeof onFoldersChanged) => {
      onFoldersChanged = cb;
      return { dispose() {} };
    },
    findFiles: async ({ base, pattern }: RelativePattern, exclude: string, limit: number) => {
      assert.equal(limit, 1);
      return fs.globSync(pattern, { cwd: base.uri.fsPath, exclude: [exclude] }).slice(0, limit);
    },
  };
  const window = {
    showInformationMessage: async (message: string, ...actions: string[]) => {
      prompts.push(message);
      assert.deepEqual(actions, ["Convert to R#", "Don't Ask Again Here"]);
      return choice;
    },
    withProgress: async (_options: unknown, action: () => Promise<void>) => action(),
    showErrorMessage: (message: string) => errors.push(message),
  };
  const loader = Module as unknown as { _load: (name: string, ...args: unknown[]) => unknown };
  const original = loader._load;
  const suggestionPath = require.resolve("../src/projectSuggestion");
  delete require.cache[suggestionPath];
  let registerProjectSuggestions: typeof import("../src/projectSuggestion").registerProjectSuggestions;
  try {
    loader._load = (name, ...args) => name === "vscode"
      ? { workspace, window, RelativePattern, ProgressLocation: { Notification: 15 } }
      : original.call(loader, name, ...args);
    ({ registerProjectSuggestions } = require(suggestionPath));
  } finally {
    loader._load = original;
  }
  const convert = async (projectDir: string) => { conversions.push(projectDir); };
  function open(run = convert) {
    registerProjectSuggestions(
      { subscriptions: [], workspaceState: {
        get: (key: string) => preferences.get(key),
        update: async (key: string, value: boolean) => preferences.set(key, value),
      } } as unknown as ExtensionContext,
      { appendLine: (line: string) => output.push(line), show() {} } as unknown as OutputChannel,
      run,
    );
    return new Promise<void>((resolve) => setImmediate(resolve));
  }
  return {
    dir, project, prompts, conversions, errors, output, workspace, window, open,
    choose: (action?: string) => { choice = action; },
    added: (...folders: Folder[]) => onFoldersChanged({ added: folders }),
  };
}

it("offers conversion on folder open and remembers only an explicit opt-out for that folder", async (t) => {
  const f = fixture(t);
  await f.open();
  assert.equal(f.prompts.length, 1);
  assert.match(f.prompts[0], /Cargo project/);
  await f.added(f.project);
  assert.equal(f.prompts.length, 1);
  assert.deepEqual(f.conversions, []);

  f.choose("Don't Ask Again Here");
  await f.open();
  assert.equal(f.prompts.length, 2);
  f.choose("Convert to R#");
  await f.open();
  assert.equal(f.prompts.length, 2);
  assert.deepEqual(f.conversions, []);

  const other = folderAt(f.dir, "Second project");
  fs.writeFileSync(join(other.uri.fsPath, "Cargo.toml"), "[workspace]");
  fs.mkdirSync(join(other.uri.fsPath, "member"));
  fs.writeFileSync(join(other.uri.fsPath, "member", "lib.rs"), "fn f() {}");
  f.workspace.workspaceFolders!.push(other);
  await f.added(other);
  assert.match(f.prompts.at(-1)!, /Second project/);
  assert.deepEqual(f.conversions, [other.uri.fsPath]);
});

it("ignores virtual, untrusted, non-Cargo, generated-only, and mixed folders", async (t) => {
  const f = fixture(t);
  f.workspace.workspaceFolders = undefined;
  await f.open();
  f.workspace.isTrusted = false;
  await f.added(f.project);
  f.workspace.isTrusted = true;
  const virtual = folderAt(f.dir, "virtual", "git");
  await f.added(virtual, folderAt(f.dir, "notes"));
  const invalid = folderAt(f.dir, "invalid manifest");
  fs.mkdirSync(join(invalid.uri.fsPath, "Cargo.toml"));
  fs.writeFileSync(join(invalid.uri.fsPath, "main.rs"), "fn main() {}");
  await f.added(invalid);

  const root = f.project.uri.fsPath;
  fs.mkdirSync(join(root, "target"));
  fs.renameSync(join(root, "main.rs"), join(root, "target", "generated.rs"));
  await f.added(f.project);
  fs.writeFileSync(join(root, "lib.rs"), "fn f() {}");
  fs.writeFileSync(join(root, "existing.rsharp"), "void g() {}");
  await f.added(f.project);
  assert.deepEqual(f.prompts, []);
  fs.renameSync(join(root, "existing.rsharp"), join(root, "target", "generated.rsharp"));
  await f.added(f.project);
  assert.equal(f.prompts.length, 1);
  assert.deepEqual(f.conversions, []);
});

it("handles search and conversion failures and ignores acceptance after folder removal", async (t) => {
  const f = fixture(t);
  f.choose("Convert to R#");
  await f.open(async () => { throw new Error("rust-analyzer unavailable"); });
  assert.equal(f.errors.length, 1);
  assert.match(f.output[0], /rust-analyzer unavailable/);

  const prompt = f.window.showInformationMessage;
  f.window.showInformationMessage = async (...args) => {
    f.workspace.isTrusted = false;
    return prompt(...args);
  };
  await f.open();
  assert.deepEqual(f.conversions, []);
  f.workspace.isTrusted = true;
  f.window.showInformationMessage = async (...args) => {
    f.workspace.workspaceFolders = [];
    return prompt(...args);
  };
  await f.open();
  assert.equal(f.prompts.length, 3);
  assert.deepEqual(f.conversions, []);
  f.workspace.findFiles = async () => { throw new Error("search failed"); };
  await f.added(f.project);
  assert.match(f.output.at(-1)!, /search failed/);
  assert.equal(f.errors.length, 1);
});
