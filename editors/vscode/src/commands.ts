import { execFile } from "child_process";
import { mkdtemp, rename, rm } from "node:fs/promises";
import { basename, join } from "node:path";

export interface ProcessResult {
  exitCode: number;
  stdout: string;
  stderr: string;
}

export type ExecFileLike = (
  command: string,
  args: readonly string[],
  callback: (
    error: (Error & { code?: number | string | null }) | null,
    stdout: string,
    stderr: string,
  ) => void,
) => void;

export function runCommandProcess(
  command: string,
  args: readonly string[],
  runner: ExecFileLike,
): Promise<ProcessResult> {
  return new Promise((resolve) => {
    runner(command, args, (error, stdout, stderr) => {
      if (!error) {
        resolve({ exitCode: 0, stdout, stderr });
        return;
      }
      const exitCode = typeof error.code === "number" ? error.code : 1;
      const errOut = stderr.length > 0 ? stderr : error.message;
      resolve({ exitCode, stdout, stderr: errOut });
    });
  });
}

export function defaultExecFile(): ExecFileLike {
  return (command, args, cb) => {
    execFile(command, args, { encoding: "utf8" }, cb);
  };
}

export const RUST_PREVIEW_SCHEME = "rsharp-rust";

export function makePreviewUriString(sourcePath: string): string {
  return `${RUST_PREVIEW_SCHEME}:${sourcePath}.rs`;
}

export function sourcePathFromPreviewUri(uriPath: string): string {
  return uriPath.endsWith(".rs") ? uriPath.slice(0, -3) : uriPath;
}

export interface PreviewProviderDeps {
  resolveBinary: () => string | undefined;
  runProcess: (
    command: string,
    args: readonly string[],
  ) => Promise<ProcessResult>;
}

export class RustPreviewModel {
  constructor(private readonly deps: PreviewProviderDeps) {}

  async loadPreview(uriPath: string): Promise<string> {
    const bin = this.deps.resolveBinary();
    if (!bin) {
      return "// R# binary not found. Build `rsharp-cli` or configure `rsharp.executablePath`.\n";
    }
    const sourcePath = sourcePathFromPreviewUri(uriPath);
    const res = await this.deps.runProcess(bin, ["to-rust", sourcePath]);
    if (res.exitCode !== 0) {
      const msg = res.stderr.trim().replace(/\r?\n/g, "\n// ");
      return `// Failed to transpile ${sourcePath}:\n// ${msg}\n`;
    }
    return res.stdout;
  }
}

export interface ActiveFileInfo {
  filePath: string;
  languageId: string;
  isDirty: boolean;
  save: () => Promise<boolean>;
}

export interface CommandHost {
  getActiveFile: () => ActiveFileInfo | undefined;
  resolveBinary: () => string | undefined;
  runProcess: (
    command: string,
    args: readonly string[],
  ) => Promise<ProcessResult>;
  runInTerminal: (commandLine: string) => void;
  showOutput: (title: string, text: string) => void;
  showInfo: (message: string) => void;
  showError: (message: string) => void;
  openRustPreview: (sourcePath: string) => Promise<void>;
  openUntitledRSharp: (content: string) => Promise<void>;
}

function quoteArg(value: string): string {
  return `"${value.replace(/"/g, '\\"')}"`;
}

export interface CommandHandlers {
  runFile: () => Promise<void>;
  checkFile: () => Promise<void>;
  previewRust: () => Promise<void>;
  convertFromRust: () => Promise<void>;
  convertProject: (projectDir: string) => Promise<string | undefined>;
}

export function createCommandHandlers(host: CommandHost): CommandHandlers {
  const requireBinary = (): string | undefined => {
    const bin = host.resolveBinary();
    if (!bin) {
      host.showError(
        "R# binary not found. Run `cargo build --release -p rsharp-cli` or set `rsharp.executablePath`.",
      );
    }
    return bin;
  };

  const requireActiveLanguage = async (
    expectedLang: string,
    label: string,
  ): Promise<ActiveFileInfo | undefined> => {
    const active = host.getActiveFile();
    if (!active || active.languageId !== expectedLang) {
      host.showError(`Open a ${label} file first.`);
      return undefined;
    }
    if (active.isDirty) {
      await active.save();
    }
    return active;
  };

  return {
    runFile: async () => {
      const bin = requireBinary();
      if (!bin) {
        return;
      }
      const active = await requireActiveLanguage("rsharp", "R# (.rsharp)");
      if (!active) {
        return;
      }
      host.runInTerminal(`${quoteArg(bin)} run ${quoteArg(active.filePath)}`);
    },

    checkFile: async () => {
      const bin = requireBinary();
      if (!bin) {
        return;
      }
      const active = await requireActiveLanguage("rsharp", "R# (.rsharp)");
      if (!active) {
        return;
      }
      const res = await host.runProcess(bin, ["check", active.filePath]);
      if (res.exitCode === 0) {
        host.showOutput("R# Check", res.stdout.trim());
        host.showInfo(`R# check passed: ${active.filePath}`);
      } else {
        host.showOutput("R# Check", res.stderr.trim());
        host.showError("R# check reported errors. See the R# Output panel.");
      }
    },

    previewRust: async () => {
      const bin = requireBinary();
      if (!bin) {
        return;
      }
      const active = await requireActiveLanguage("rsharp", "R# (.rsharp)");
      if (!active) {
        return;
      }
      await host.openRustPreview(active.filePath);
    },

    convertFromRust: async () => {
      const bin = requireBinary();
      if (!bin) {
        return;
      }
      const active = await requireActiveLanguage("rust", "Rust (.rs)");
      if (!active) {
        return;
      }
      const res = await host.runProcess(bin, ["from-rust", active.filePath]);
      if (res.exitCode !== 0) {
        host.showOutput("R# Convert", res.stderr.trim());
        host.showError(
          "Failed to convert Rust file to R#. See the R# Output panel.",
        );
        return;
      }
      await host.openUntitledRSharp(res.stdout);
    },

    convertProject: async (projectDir) => {
      const bin = requireBinary();
      if (!bin) {
        return;
      }

      // The CLI can write some files before reporting a conversion error.
      const staging = await mkdtemp(join(projectDir, ".rsharp-"));
      const output = join(projectDir, basename(staging).slice(1));
      try {
        const res = await host.runProcess(bin, [
          "from-rust", "--project", projectDir, "-o", staging, "--verify",
        ]);
        if (res.exitCode !== 0) {
          throw new Error(res.stderr.trim());
        }
        await rename(staging, output);
      } catch (error) {
        await rm(staging, { recursive: true, force: true });
        throw error;
      }
      host.showInfo(`R# sources saved to ${output}`);
      return output;
    },
  };
}
