import { it, type TestContext } from "node:test";
import * as assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { findConvertedFolder, RustSourceSync } from "../src/rustSync";

async function fixture(t: TestContext) {
  const root = await fs.mkdtemp(join(tmpdir(), "Cargo sync #"));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const project = join(root, "workspace member");
  const copy = join(project, "rsharp-Ab12Cd");
  const name = join("src", "nested", "math");
  const source = join(copy, name + ".rsharp");
  const target = join(project, name + ".rs");
  await fs.mkdir(dirname(source), { recursive: true });
  await fs.mkdir(dirname(target), { recursive: true });
  await fs.writeFile(join(project, "Cargo.toml"), "[workspace]");
  await fs.writeFile(join(copy, "notes.txt"), "unrelated");
  await fs.writeFile(join(copy, "new.rsharp"), "");
  await fs.writeFile(source, "int value() { return 42; }");
  await fs.writeFile(target, "// original Rust\nfn value() -> i32 { 1 }");
  const state = new Map<string, unknown>();
  const actions: string[] = [];
  let calls = 0;
  const rust = "fn value() -> i32 { 42 }";
  const host = {
    state: {
      get: <T>(key: string, fallback?: T) => (state.get(key) ?? fallback) as T,
      update: async (key: string, value: unknown) => { state.set(key, value); },
    },
    isDirty: (_file: string) => false,
    resolveBinary: (): string | undefined => "rsharp",
    confirm: async (_message: string, action: string) => { actions.push(action); return true; },
    runProcess: async (bin: string, args: readonly string[]) => {
      calls++;
      assert.equal(bin, "rsharp");
      assert.deepEqual(args.slice(0, -1), ["to-rust", source, "--prelude", "none", "-o"]);
      assert.notEqual(args[5], target);
      await fs.writeFile(args[5], rust);
      return { exitCode: 0, stdout: "", stderr: "" };
    },
  };
  const sync = new RustSourceSync(host);
  const save = (enabled = () => true) => sync.save(root, source, enabled);
  const current = () => fs.readFile(target, "utf8");
  return { root, project, copy, source, target, rust, host, sync, save, current, actions, calls: () => calls };
}

it("maps only existing originals in a converted Cargo tree, including nested modules", async (t) => {
  const f = await fixture(t);
  assert.equal(findConvertedFolder(f.root, f.source), f.copy);
  assert.equal(f.sync.originalForComparison(f.root, f.source), f.target);
  assert.equal(f.sync.originalForComparison(f.root, f.target), undefined);
  assert.equal(findConvertedFolder(f.copy, f.source), undefined);
  assert.equal(findConvertedFolder(f.project, join(f.root, "outside.rsharp")), undefined);
  assert.equal(findConvertedFolder(f.project, f.target), undefined);
  assert.equal(findConvertedFolder(f.project, join(f.project, "rsharp-other", "app.rsharp")), undefined);
  await fs.writeFile(join(f.copy, "Cargo.toml"), "");
  assert.equal(findConvertedFolder(f.root, f.source), undefined);
  await fs.unlink(join(f.copy, "Cargo.toml"));
  await fs.rm(join(f.project, "Cargo.toml"));
  assert.equal(findConvertedFolder(f.root, f.source), undefined);
  await fs.writeFile(join(f.project, "Cargo.toml"), "[workspace]");

  await f.sync.rememberProject(f.copy);
  await f.save(() => false);
  await f.sync.save(f.root, join(f.project, "standalone.rsharp"), () => true);
  await f.sync.save(f.root, join(f.copy, "new.rsharp"), () => true);
  await fs.writeFile(join(f.project, "new.rs"), "not part of the conversion");
  assert.equal(f.sync.originalForComparison(f.root, join(f.copy, "new.rsharp")), undefined);
  await f.sync.save(f.root, join(f.copy, "new.rsharp"), () => true);
  assert.equal(f.calls(), 0);

  const sibling = join(f.project, "src", "math.rs");
  await fs.writeFile(sibling, "independent");
  await fs.chmod(f.target, 0o744);
  const mode = (await fs.stat(f.target)).mode;
  assert.equal(await f.save(), f.target);
  assert.equal(await f.current(), f.rust);
  assert.equal((await fs.stat(f.target)).mode, mode);
  assert.equal(await fs.readFile(sibling, "utf8"), "independent");
  const timestamp = (await fs.stat(f.target)).mtimeMs;
  await new RustSourceSync(f.host).save(f.project, f.source, () => true);
  assert.equal((await fs.stat(f.target)).mtimeMs, timestamp);
  assert.deepEqual(f.actions, []);
  await fs.unlink(f.target);
  await assert.rejects(f.save(), /original Rust file.*missing or redirected/);
  await assert.rejects(fs.stat(f.target), { code: "ENOENT" });
});

it("links legacy copies explicitly and asks before replacing independent Rust edits", async (t) => {
  const f = await fixture(t);
  const original = await f.current();
  let allow = false;
  f.host.confirm = async (message, action) => {
    assert.ok(message.includes(f.project));
    f.actions.push(action);
    return allow;
  };
  await f.save();
  assert.equal(f.calls(), 0);
  assert.equal(await f.current(), original);
  allow = true;
  await f.save();
  assert.equal(await f.current(), f.rust);
  await fs.writeFile(f.target, "edited in Rust");
  allow = false;
  await f.save();
  assert.equal(await f.current(), "edited in Rust");
  allow = true;
  await f.save();
  assert.equal(await f.current(), f.rust);
  assert.deepEqual(f.actions, ["Link and Sync", "Link and Sync", "Replace Rust File", "Replace Rust File"]);
});

it("preserves Rust on dirty buffers, missing tools, failed generation, or concurrent edits", async (t) => {
  const f = await fixture(t);
  await f.sync.rememberProject(f.copy);
  const before = await f.current();
  f.host.isDirty = (path) => path === f.target;
  await assert.rejects(f.save(), /unsaved Rust edits/);
  f.host.isDirty = () => false;
  f.host.resolveBinary = () => undefined;
  await assert.rejects(f.save(), /executablePath/);
  f.host.resolveBinary = () => "rsharp";
  const generate = f.host.runProcess;
  f.host.runProcess = async (bin, args) => {
    await generate(bin, args);
    return { exitCode: 1, stdout: "", stderr: "invalid R#" };
  };
  await assert.rejects(f.save(), /invalid R#/);
  assert.equal(await f.current(), before);
  f.host.runProcess = async () => ({ exitCode: 0, stdout: "", stderr: "" });
  await assert.rejects(f.save(), { code: "ENOENT" });
  assert.equal(await f.current(), before);
  let enabled = true;
  f.host.runProcess = async (bin, args) => { enabled = false; return generate(bin, args); };
  await f.save(() => enabled);
  assert.equal(await f.current(), before);
  f.host.runProcess = async (bin, args) => {
    await fs.writeFile(f.target, "concurrent Rust edit");
    return generate(bin, args);
  };
  await assert.rejects(f.save(), /changed during synchronization/);
  assert.equal(await f.current(), "concurrent Rust edit");
  f.host.runProcess = async (bin, args) => { f.host.isDirty = () => true; return generate(bin, args); };
  await assert.rejects(f.save(), /unsaved Rust edits/);
  assert.deepEqual(await fs.readdir(dirname(f.target)), ["math.rs"]);
});

it("serializes overlapping saves and retries after an earlier failure", async (t) => {
  const f = await fixture(t);
  await f.sync.rememberProject(f.copy);
  let start!: () => void;
  let release!: () => void;
  const started = new Promise<void>((resolve) => { start = resolve; });
  const resume = new Promise<void>((resolve) => { release = resolve; });
  const generate = f.host.runProcess;
  f.host.runProcess = async (bin, args) => {
    start();
    await resume;
    await generate(bin, args);
    throw new Error("first save failed");
  };
  const first = assert.rejects(f.save(), /first save failed/);
  await started;
  let enabled = true;
  const cancelled = f.save(() => enabled);
  const latest = f.save();
  f.host.runProcess = generate;
  enabled = false;
  release();
  await first;
  assert.equal(await cancelled, undefined);
  assert.equal(await latest, f.target);
  assert.equal(f.calls(), 2);
  assert.equal(await f.current(), f.rust);
});

it("waits for project indexing and reports a storage failure after publishing Rust", async (t) => {
  const f = await fixture(t);
  const persist = f.host.state.update;
  let resume!: () => void;
  const indexing = new Promise<void>((resolve) => { resume = resolve; });
  let indexingStarted!: () => void;
  const started = new Promise<void>((resolve) => { indexingStarted = resolve; });
  f.host.state.update = async (key, value) => {
    indexingStarted();
    await indexing;
    await persist(key, value);
  };
  const record = f.sync.rememberProject(f.copy);
  await started;
  const save = f.save();
  f.host.state.update = async () => { throw new Error("storage full"); };
  resume();
  await record;
  await assert.rejects(save, /is synchronized.*storage full/);
  assert.deepEqual(f.actions, []);
  assert.equal(await f.current(), f.rust);
});

it("does not publish when the workspace setting changes during conflict resolution", async (t) => {
  const f = await fixture(t);
  await f.sync.rememberProject(f.copy);
  await fs.writeFile(f.target, "changed Rust");
  let enabled = true;
  f.host.confirm = async () => { enabled = false; return true; };
  await f.save(() => enabled);
  assert.equal(await f.current(), "changed Rust");
});

it("skips redirected files and honors disabling sync during legacy confirmation", async (t) => {
  const f = await fixture(t);
  let enabled = true;
  f.host.confirm = async () => { enabled = false; return true; };
  await f.save(() => enabled);
  assert.equal(f.calls(), 0);
  const outside = join(f.root, "outside");
  await fs.mkdir(outside);
  const redirect = join(f.copy, "redirect");
  await fs.symlink(outside, redirect, "junction");
  await fs.writeFile(join(outside, "other.rsharp"), "int unrelated;");
  await f.sync.save(f.root, join(redirect, "other.rsharp"), () => true);
  await fs.mkdir(join(f.copy, "linked"));
  await fs.writeFile(join(f.copy, "linked", "other.rsharp"), "int unrelated;");
  await fs.symlink(outside, join(f.project, "linked"), "junction");
  await fs.writeFile(join(outside, "other.rs"), "keep");
  await f.sync.save(f.root, join(f.copy, "linked", "other.rsharp"), () => true);
  await f.sync.rememberProject(f.copy);
  await f.sync.save(f.root, join(f.copy, "linked", "other.rsharp"), () => true);
  assert.equal(await fs.readFile(join(outside, "other.rs"), "utf8"), "keep");
  assert.equal(f.calls(), 0);
});
