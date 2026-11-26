import { describe, it } from "node:test";
import * as assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";

const EXT_ROOT = path.resolve(__dirname, "..", "..");
const REPO_ROOT = path.resolve(EXT_ROOT, "..", "..");

function collectRegexes(node: unknown, out: string[]): void {
  if (!node || typeof node !== "object") {
    return;
  }
  if (Array.isArray(node)) {
    for (const item of node) {
      collectRegexes(item, out);
    }
    return;
  }
  const obj = node as Record<string, unknown>;
  for (const key of ["match", "begin", "end"]) {
    if (typeof obj[key] === "string") {
      out.push(obj[key] as string);
    }
  }
  for (const val of Object.values(obj)) {
    collectRegexes(val, out);
  }
}

describe("VS Code manifest, grammar, language config, and snippets", () => {
  it("validates package.json contributions and referenced files", () => {
    const pkg = JSON.parse(
      fs.readFileSync(path.join(EXT_ROOT, "package.json"), "utf8"),
    );
    assert.equal(pkg.contributes.languages[0].id, "rsharp");
    assert.ok(pkg.activationEvents.includes("workspaceContains:Cargo.toml"));
    assert.deepEqual(pkg.contributes.languages[0].extensions, [".rsharp"]);
    assert.equal(pkg.contributes.grammars[0].scopeName, "source.rsharp");
    assert.equal(
      pkg.contributes.grammars[0].embeddedLanguages[
        "meta.embedded.block.rust.rsharp"
      ],
      "rust",
    );

    for (const rel of [
      pkg.contributes.languages[0].configuration,
      pkg.contributes.grammars[0].path,
      pkg.contributes.snippets[0].path,
    ]) {
      assert.ok(
        fs.existsSync(path.join(EXT_ROOT, rel)),
        `Missing contributed file: ${rel}`,
      );
    }
  });

  it("validates TextMate grammar structure, repository includes, and regex syntax", () => {
    const grammar = JSON.parse(
      fs.readFileSync(
        path.join(EXT_ROOT, "syntaxes", "rsharp.tmLanguage.json"),
        "utf8",
      ),
    );
    assert.equal(grammar.scopeName, "source.rsharp");
    assert.ok(grammar.repository["inline-rust"]);

    // Verify all internal `#ref` includes exist in repository
    const jsonText = JSON.stringify(grammar);
    for (const m of jsonText.matchAll(/"include":"#([^"]+)"/g)) {
      assert.ok(
        grammar.repository[m[1]],
        `Missing repository rule '#${m[1]}'`,
      );
    }

    // Verify every regex pattern is valid (convert Oniguruma `\A` anchor to `^` for JS RegExp check)
    const patterns: string[] = [];
    collectRegexes(grammar, patterns);
    assert.ok(patterns.length > 20);
    for (const pat of patterns) {
      const jsPat = pat.replace(/\\A/g, "^");
      assert.doesNotThrow(
        () => new RegExp(jsPat),
        `Invalid regex pattern: ${pat}`,
      );
    }
  });

  it("matches key R# syntax constructs across all repository examples", () => {
    const grammar = JSON.parse(
      fs.readFileSync(
        path.join(EXT_ROOT, "syntaxes", "rsharp.tmLanguage.json"),
        "utf8",
      ),
    );

    const accounts = fs.readFileSync(
      path.join(REPO_ROOT, "examples", "accounts.rsharp"),
      "utf8",
    );
    const matrices = fs.readFileSync(
      path.join(REPO_ROOT, "examples", "matrices_and_control.rsharp"),
      "utf8",
    );
    const interop = fs.readFileSync(
      path.join(REPO_ROOT, "examples", "rust_interop.rsharp"),
      "utf8",
    );

    const inlineRustRe = new RegExp(grammar.repository["inline-rust"].begin);
    assert.match(interop, inlineRustRe);

    const attrRe = new RegExp(grammar.repository.attributes.begin);
    assert.match(accounts, attrRe);

    const interpRe = new RegExp(grammar.repository.strings.patterns[0].begin);
    assert.match(accounts, interpRe);
    assert.match(matrices, interpRe);

    const structDeclRe = new RegExp(
      grammar.repository.declarations.patterns[0].match,
    );
    assert.match(accounts, structDeclRe);

    const enumDeclRe = new RegExp(
      grammar.repository.declarations.patterns[1].match,
    );
    assert.match(matrices, enumDeclRe);

    const traitDeclRe = new RegExp(
      grammar.repository.declarations.patterns[3].match,
    );
    assert.match(interop, traitDeclRe);

    const lifeDeclRe = new RegExp(
      grammar.repository.declarations.patterns[7].match,
    );
    assert.match(interop, lifeDeclRe);
  });

  it("validates language-configuration.json and snippets JSON", () => {
    const langCfg = JSON.parse(
      fs.readFileSync(
        path.join(EXT_ROOT, "language-configuration.json"),
        "utf8",
      ),
    );
    assert.equal(langCfg.comments.lineComment, "//");
    assert.deepEqual(langCfg.comments.blockComment, ["/*", "*/"]);
    assert.doesNotThrow(() => new RegExp(langCfg.wordPattern));
    assert.doesNotThrow(
      () => new RegExp(langCfg.indentationRules.increaseIndentPattern),
    );
    assert.doesNotThrow(
      () => new RegExp(langCfg.indentationRules.decreaseIndentPattern),
    );

    const snippets = JSON.parse(
      fs.readFileSync(
        path.join(EXT_ROOT, "snippets", "rsharp.code-snippets"),
        "utf8",
      ),
    );
    assert.ok(Object.keys(snippets).length >= 10);
  });
});
