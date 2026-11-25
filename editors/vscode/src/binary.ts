import { existsSync as fsExistsSync } from "fs";
import { posix as posixPath, win32 as win32Path } from "path";

export interface BinaryResolutionOptions {
  configuredPath: string;
  workspaceRoots: readonly string[];
  platform: NodeJS.Platform;
  envPath: string;
  existsSync: (candidate: string) => boolean;
}

export function executableName(platform: NodeJS.Platform): string {
  return platform === "win32" ? "rsharp.exe" : "rsharp";
}

function hasPathSeparator(value: string): boolean {
  return value.includes("/") || value.includes("\\");
}

function findOnPath(
  name: string,
  envPath: string,
  platform: NodeJS.Platform,
  existsSync: (candidate: string) => boolean,
): string | undefined {
  const delimiter = platform === "win32" ? ";" : ":";
  const pathLib = platform === "win32" ? win32Path : posixPath;
  for (const dir of envPath.split(delimiter)) {
    const trimmed = dir.trim();
    if (!trimmed) {
      continue;
    }
    const candidate = pathLib.join(trimmed, name);
    if (existsSync(candidate)) {
      return candidate;
    }
  }
  return undefined;
}

/**
 * Resolves the `rsharp` executable in precedence order:
 * 1. Explicit `rsharp.executablePath` setting
 * 2. Workspace `target/release/rsharp[.exe]`
 * 3. Workspace `target/debug/rsharp[.exe]`
 * 4. `PATH` lookup
 */
export function resolveRSharpBinary(
  options: BinaryResolutionOptions,
): string | undefined {
  const { configuredPath, workspaceRoots, platform, envPath, existsSync } =
    options;
  const trimmed = configuredPath.trim();
  if (trimmed.length > 0) {
    if (hasPathSeparator(trimmed)) {
      return existsSync(trimmed) ? trimmed : undefined;
    }
    return findOnPath(trimmed, envPath, platform, existsSync);
  }

  const bin = executableName(platform);
  const pathLib = platform === "win32" ? win32Path : posixPath;
  for (const root of workspaceRoots) {
    for (const profile of ["release", "debug"]) {
      const candidate = pathLib.join(root, "target", profile, bin);
      if (existsSync(candidate)) {
        return candidate;
      }
    }
  }

  return findOnPath(bin, envPath, platform, existsSync);
}

export function defaultBinaryOptions(
  configuredPath: string,
  workspaceRoots: readonly string[],
): BinaryResolutionOptions {
  return {
    configuredPath,
    workspaceRoots,
    platform: process.platform,
    envPath: process.env.PATH ?? "",
    existsSync: fsExistsSync,
  };
}
