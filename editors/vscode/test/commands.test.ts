import { describe, it } from "node:test";
import * as assert from "node:assert/strict";
import {
  ActiveFileInfo,
  CommandHost,
  ProcessResult,
  RustPreviewModel,
  createCommandHandlers,
  defaultExecFile,
  makePreviewUriString,
  runCommandProcess,
  sourcePathFromPreviewUri,
} from "../src/commands";

describe("commands and process runner", () => {
  it("runs processes and normalizes success, numeric exit codes, and spawn errors", async () => {
    const ok = await runCommandProcess("cmd", ["a"], (_c, _a, cb) =>
      cb(null, "out", ""),
    );
    assert.deepEqual(ok, { exitCode: 0, stdout: "out", stderr: "" });

    const nonZero = await runCommandProcess("cmd", ["a"], (_c, _a, cb) => {
      const err = Object.assign(new Error("failed"), { code: 2 });
      cb(err, "", "bad syntax");
    });
    assert.deepEqual(nonZero, {
      exitCode: 2,
      stdout: "",
      stderr: "bad syntax",
    });

    const spawnErr = await runCommandProcess("cmd", ["a"], (_c, _a, cb) => {
      const err = Object.assign(new Error("ENOENT"), { code: "ENOENT" });
      cb(err, "", "");
    });
    assert.deepEqual(spawnErr, {
      exitCode: 1,
      stdout: "",
      stderr: "ENOENT",
    });

    // Exercise defaultExecFile wrapper directly with `node --version`.
    const real = await runCommandProcess(
      process.execPath,
      ["--version"],
      defaultExecFile(),
    );
    assert.equal(real.exitCode, 0);
    assert.match(real.stdout, /^v\d+/);
  });

  it("maps preview URIs and loads transpiled Rust content", async () => {
    const uri = makePreviewUriString("/ws/app.rsharp");
    assert.equal(uri, "rsharp-rust:/ws/app.rsharp.rs");
    assert.equal(sourcePathFromPreviewUri("/ws/app.rsharp.rs"), "/ws/app.rsharp");
    assert.equal(sourcePathFromPreviewUri("/ws/app.rsharp"), "/ws/app.rsharp");

    const noBin = new RustPreviewModel({
      resolveBinary: () => undefined,
      runProcess: async () => ({ exitCode: 0, stdout: "", stderr: "" }),
    });
    assert.match(await noBin.loadPreview("/ws/a.rsharp.rs"), /not found/);

    const okModel = new RustPreviewModel({
      resolveBinary: () => "/bin/rsharp",
      runProcess: async () => ({
        exitCode: 0,
        stdout: "fn main() {}\n",
        stderr: "",
      }),
    });
    assert.equal(await okModel.loadPreview("/ws/a.rsharp.rs"), "fn main() {}\n");

    const errModel = new RustPreviewModel({
      resolveBinary: () => "/bin/rsharp",
      runProcess: async () => ({
        exitCode: 1,
        stdout: "",
        stderr: "line 1\nline 2",
      }),
    });
    const errText = await errModel.loadPreview("/ws/a.rsharp.rs");
    assert.match(errText, /Failed to transpile \/ws\/a\.rsharp/);
    assert.match(errText, /\/\/ line 1\n\/\/ line 2/);
  });

  it("handles run, check, preview, and convert commands across all paths", async () => {
    const logs: string[] = [];
    let binary: string | undefined = "/bin/rsharp";
    let activeFile: ActiveFileInfo | undefined;
    let nextProcessResult: ProcessResult = {
      exitCode: 0,
      stdout: "OK",
      stderr: "",
    };

    const host: CommandHost = {
      getActiveFile: () => activeFile,
      resolveBinary: () => binary,
      runProcess: async (cmd, args) => {
        logs.push(`exec:${cmd}:${args.join(",")}`);
        return nextProcessResult;
      },
      runInTerminal: (line) => logs.push(`term:${line}`),
      showOutput: (title, text) => logs.push(`out:${title}:${text}`),
      showInfo: (msg) => logs.push(`info:${msg}`),
      showError: (msg) => logs.push(`err:${msg}`),
      openRustPreview: async (p) => {
        logs.push(`preview:${p}`);
      },
      openUntitledRSharp: async (c) => {
        logs.push(`untitled:${c}`);
      },
    };

    const h = createCommandHandlers(host);

    // Missing binary on all 4 commands
    binary = undefined;
    await h.runFile();
    await h.checkFile();
    await h.previewRust();
    await h.convertFromRust();
    assert.equal(logs.filter((l) => l.startsWith("err:")).length, 4);
    logs.length = 0;

    // Missing or wrong active file
    binary = "/bin/rsharp";
    activeFile = undefined;
    await h.runFile();
    await h.checkFile();
    await h.previewRust();
    await h.convertFromRust();
    assert.equal(logs.filter((l) => l.startsWith("err:")).length, 4);
    logs.length = 0;

    // Active dirty .rsharp file -> saves before running, checks ok & err, previews Rust
    let savedCount = 0;
    activeFile = {
      filePath: '/ws/my "app".rsharp',
      languageId: "rsharp",
      isDirty: true,
      save: async () => {
        savedCount += 1;
        return true;
      },
    };
    await h.runFile();
    assert.equal(savedCount, 1);
    assert.ok(logs.some((l) => l.includes('term:"/bin/rsharp" run "/ws/my \\"app\\".rsharp"')));

    activeFile.isDirty = false;
    nextProcessResult = { exitCode: 0, stdout: "OK file\n", stderr: "" };
    await h.checkFile();
    assert.ok(logs.includes("out:R# Check:OK file"));
    assert.ok(logs.some((l) => l.startsWith("info:R# check passed:")));

    nextProcessResult = { exitCode: 1, stdout: "", stderr: "error: bad\n" };
    await h.checkFile();
    assert.ok(logs.includes("out:R# Check:error: bad"));
    assert.ok(logs.some((l) => l.includes("R# check reported errors")));

    await h.previewRust();
    assert.ok(logs.includes('preview:/ws/my "app".rsharp'));

    // Active .rs file -> convert ok & convert err
    activeFile = {
      filePath: "/ws/lib.rs",
      languageId: "rust",
      isDirty: false,
      save: async () => true,
    };
    nextProcessResult = { exitCode: 0, stdout: "void main() {}\n", stderr: "" };
    await h.convertFromRust();
    assert.ok(logs.includes("untitled:void main() {}\n"));

    nextProcessResult = { exitCode: 1, stdout: "", stderr: "unresolved type" };
    await h.convertFromRust();
    assert.ok(logs.includes("out:R# Convert:unresolved type"));
    assert.ok(logs.some((l) => l.includes("Failed to convert Rust file")));
  });
});
