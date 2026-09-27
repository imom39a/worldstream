import assert from "node:assert/strict";
import { readFile, stat } from "node:fs/promises";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const html = await readFile(resolve(root, "site/index.html"), "utf8");
const ids = [...html.matchAll(/\bid="([^"]+)"/gu)].map((match) => match[1]);
assert.equal(ids.length, new Set(ids).size, "HTML IDs must be unique");
assert.match(html, /<html lang="en">/u);
assert.match(html, /name="viewport"/u);
assert.equal((html.match(/<h1\b/gu) ?? []).length, 1);
assert.doesNotMatch(html, /<script\b|<iframe\b|<form\b/iu, "The overview must stay static");

let checked = 0;
for (const [, target] of html.matchAll(/(?:href|src)="([^"]+)"/gu)) {
  if (target.startsWith("#")) {
    assert(ids.includes(target.slice(1)), `Missing anchor: ${target}`);
  } else if (target.startsWith("https://github.com/imom39a/worldstream/")) {
    const match = target.match(/\/(?:blob|tree)\/main\/(.+)$/u);
    if (match) await stat(resolve(root, match[1]));
  } else if (!/^https?:\/\//u.test(target)) {
    await stat(resolve(root, "site", target));
  }
  checked += 1;
}
const css = await readFile(resolve(root, "site/styles.css"), "utf8");
assert.doesNotMatch(css, /@import|url\(\s*["']?https?:/iu, "Styles must work offline");
console.log(`Static site: ${checked} links/assets checked; no runtime or remote styles.`);
