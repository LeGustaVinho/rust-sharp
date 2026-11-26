import { createHash } from "node:crypto";
import { constants, realpathSync, statSync } from "node:fs";
import { access, chmod, mkdtemp, readFile, readdir, rename, rm, rmdir, stat } from "node:fs/promises";
import { dirname, extname, isAbsolute, join, relative, sep } from "node:path";
import type { Memento } from "vscode";
import type { PreviewProviderDeps } from "./commands";

type Originals = Record<string, string>;

interface SyncHost extends PreviewProviderDeps {
  state: Pick<Memento, "get" | "update">;
  isDirty: (file: string) => boolean;
  confirm: (message: string, action: string) => Promise<boolean>;
}

function isProjectFile(root: string, file: string): boolean {
  if (!statSync(file, { throwIfNoEntry: false })?.isFile()) {
    return false;
  }
  // A link below the trusted root must not redirect a source read or write.
  const expected = join(realpathSync(root), relative(root, file));
  return relative(expected, realpathSync(file)) === "";
}

function rustName(source: string): string {
  return source.slice(0, -".rsharp".length) + ".rs";
}

async function fingerprint(file: string): Promise<string> {
  return createHash("sha256").update(await readFile(file)).digest("hex");
}

function stateKey(copy: string): string {
  return `rsharp.originalRust:${copy}`;
}

export function findConvertedFolder(root: string, source: string): string | undefined {
  const path = relative(root, source);
  const parts = path.split(sep);
  if (isAbsolute(path) || parts[0] === ".." || extname(source) !== ".rsharp") {
    return;
  }
  for (let i = parts.length - 2; i >= 0; i--) {
    if (/^rsharp-[a-zA-Z0-9]{6}$/.test(parts[i])) {
      const project = join(root, ...parts.slice(0, i));
      const copy = join(project, parts[i]);
      if (
        isProjectFile(root, join(project, "Cargo.toml")) &&
        !statSync(join(copy, "Cargo.toml"), { throwIfNoEntry: false })
      ) {
        return copy;
      }
    }
  }
  return undefined;
}

export class RustSourceSync {
  private readonly pending = new Map<string, Promise<unknown>>();

  constructor(private readonly host: SyncHost) {}

  originalForComparison(root: string, source: string): string | undefined {
    const copy = findConvertedFolder(root, source);
    if (!copy) {
      return;
    }
    const name = rustName(relative(copy, source));
    const originals = this.host.state.get<Originals>(stateKey(copy));
    const target = join(dirname(copy), name);
    if ((!originals || Object.hasOwn(originals, name)) && isProjectFile(root, target)) {
      return target;
    }
    return undefined;
  }

  rememberProject(copy: string): Promise<Originals> {
    return this.inProject(dirname(copy), () => this.recordOriginals(copy));
  }

  private async recordOriginals(copy: string): Promise<Originals> {
    const project = dirname(copy);
    const originals: Originals = {};
    async function visit(dir: string): Promise<void> {
      for (const entry of await readdir(dir, { withFileTypes: true })) {
        const path = join(dir, entry.name);
        if (entry.isDirectory()) {
          await visit(path);
        } else if (entry.isFile() && extname(path) === ".rsharp") {
          const name = rustName(relative(copy, path));
          const target = join(project, name);
          if (isProjectFile(project, target)) {
            originals[name] = await fingerprint(target);
          }
        }
      }
    }
    await visit(copy);
    await this.host.state.update(stateKey(copy), originals);
    return originals;
  }

  async save(root: string, source: string, enabled: () => boolean): Promise<string | undefined> {
    if (!enabled()) {
      return;
    }
    const copy = findConvertedFolder(root, source);
    if (!copy) {
      return;
    }
    return this.inProject(dirname(copy), () => this.update(root, copy, source, enabled));
  }

  // Indexing a conversion, Save All, and auto-save can overlap in the same Cargo project.
  private async inProject<T>(project: string, run: () => Promise<T>): Promise<T> {
    const previous = this.pending.get(project) ?? Promise.resolve();
    const task = previous.catch(() => {}).then(run);
    this.pending.set(project, task);
    try {
      return await task;
    } finally {
      if (this.pending.get(project) === task) {
        this.pending.delete(project);
      }
    }
  }

  private async update(root: string, copy: string, source: string, enabled: () => boolean) {
    const project = dirname(copy);
    const name = rustName(relative(copy, source));
    const target = join(project, name);
    if (!enabled() || !isProjectFile(root, source)) {
      return;
    }
    const key = stateKey(copy);
    let originals = this.host.state.get<Originals>(key);
    if (originals && !Object.hasOwn(originals, name)) {
      return;
    }
    if (!isProjectFile(root, target)) {
      if (originals) {
        throw new Error(`The original Rust file "${target}" is missing or redirected. Restore it before syncing.`);
      }
      return;
    }
    const requireClean = () => {
      if (this.host.isDirty(target)) {
        throw new Error(`Save or revert the unsaved Rust edits in "${target}", then save the R# file again.`);
      }
    };
    requireClean();
    const before = await fingerprint(target);
    if (!originals) {
      const link = await this.host.confirm(
        `Link "${copy}" to the Rust project "${project}" and update "${name}" from this save?`,
        "Link and Sync",
      );
      if (!link || !enabled()) {
        return;
      }
      originals = await this.recordOriginals(copy);
    }
    const bin = this.host.resolveBinary();
    if (!bin) {
      throw new Error("Set rsharp.executablePath to the rsharp CLI and save again.");
    }
    const staging = await mkdtemp(join(dirname(target), ".rsharp-sync-"));
    const generated = join(staging, "source.tmp");
    try {
      const result = await this.host.runProcess(bin, ["to-rust", source, "--prelude", "none", "-o", generated]);
      if (result.exitCode !== 0) {
        throw new Error(result.stderr.trim());
      }
      const after = await fingerprint(generated);
      if (!enabled()) {
        return;
      }
      if (before !== after && originals[name] !== before) {
        const replace = await this.host.confirm(
          `"${target}" has changed separately. Replace its contents with the Rust generated from this save?`,
          "Replace Rust File",
        );
        if (!replace) {
          return;
        }
      }
      if (before !== after) {
        await access(target, constants.W_OK);
        await chmod(generated, (await stat(target)).mode & 0o777);
        if (!isProjectFile(root, target) || await fingerprint(target) !== before) {
          throw new Error(`"${target}" changed during synchronization. Review it and save the R# file again.`);
        }
        if (!enabled()) {
          return;
        }
        requireClean();
        await rename(generated, target);
      }
      if (originals[name] !== after) {
        try {
          await this.host.state.update(key, { ...originals, [name]: after });
        } catch (error) {
          throw new Error(`"${target}" is synchronized, but VS Code could not record its new contents. ${error}`);
        }
      }
      return target;
    } finally {
      await rm(generated, { force: true });
      await rmdir(staging);
    }
  }
}
