import { describe, it } from "node:test";
import * as assert from "node:assert/strict";
import {
  defaultBinaryOptions,
  executableName,
  resolveRSharpBinary,
} from "../src/binary";

describe("binary resolution", () => {
  it("picks platform-specific executable name", () => {
    assert.equal(executableName("win32"), "rsharp.exe");
    assert.equal(executableName("linux"), "rsharp");
  });

  it("prefers configured path with directory separator", () => {
    const found = resolveRSharpBinary({
      configuredPath: "  C:\\tools\\rsharp.exe  ",
      workspaceRoots: ["C:\\repo"],
      platform: "win32",
      envPath: "",
      existsSync: (p) => p === "C:\\tools\\rsharp.exe",
    });
    assert.equal(found, "C:\\tools\\rsharp.exe");

    const missing = resolveRSharpBinary({
      configuredPath: "/opt/missing/rsharp",
      workspaceRoots: ["/repo"],
      platform: "linux",
      envPath: "/usr/bin",
      existsSync: () => false,
    });
    assert.equal(missing, undefined);
  });

  it("looks up bare configured binary name on PATH", () => {
    const found = resolveRSharpBinary({
      configuredPath: "my-rsharp",
      workspaceRoots: [],
      platform: "linux",
      envPath: ":/usr/local/bin:/usr/bin",
      existsSync: (p) => p === "/usr/local/bin/my-rsharp",
    });
    assert.equal(found, "/usr/local/bin/my-rsharp");
  });

  it("prefers workspace release over debug, then falls back to PATH", () => {
    const release = resolveRSharpBinary({
      configuredPath: "",
      workspaceRoots: ["/ws"],
      platform: "linux",
      envPath: "/usr/bin",
      existsSync: (p) =>
        p === "/ws/target/release/rsharp" || p === "/ws/target/debug/rsharp",
    });
    assert.equal(release, "/ws/target/release/rsharp");

    const debug = resolveRSharpBinary({
      configuredPath: "",
      workspaceRoots: ["D:\\ws"],
      platform: "win32",
      envPath: "C:\\bin",
      existsSync: (p) => p === "D:\\ws\\target\\debug\\rsharp.exe",
    });
    assert.equal(debug, "D:\\ws\\target\\debug\\rsharp.exe");

    const fromPath = resolveRSharpBinary({
      configuredPath: "",
      workspaceRoots: ["D:\\ws"],
      platform: "win32",
      envPath: " ; C:\\bin ; C:\\tools ",
      existsSync: (p) => p === "C:\\tools\\rsharp.exe",
    });
    assert.equal(fromPath, "C:\\tools\\rsharp.exe");

    const none = resolveRSharpBinary({
      configuredPath: "",
      workspaceRoots: [],
      platform: "linux",
      envPath: "/usr/bin",
      existsSync: () => false,
    });
    assert.equal(none, undefined);
  });

  it("builds default options with and without PATH env", () => {
    const savedPath = process.env.PATH;
    try {
      process.env.PATH = "/bin";
      const opts = defaultBinaryOptions("", ["/ws"]);
      assert.equal(opts.envPath, "/bin");
      assert.equal(opts.platform, process.platform);

      delete process.env.PATH;
      const emptyOpts = defaultBinaryOptions("", []);
      assert.equal(emptyOpts.envPath, "");
    } finally {
      if (savedPath !== undefined) {
        process.env.PATH = savedPath;
      }
    }
  });
});
