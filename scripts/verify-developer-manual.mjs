#!/usr/bin/env node

import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repository = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const manual = join(repository, "web", "manual");
const contentDirectory = join(manual, "src", "content");
const pagesSource = readFileSync(join(manual, "src", "pages.ts"), "utf8");
const capabilitiesSource = readFileSync(join(manual, "src", "capabilities.ts"), "utf8");
const failures = [];

const routeMatches = [...pagesSource.matchAll(/route:\s*"([^"]+)"/gu)].map((match) => match[1]);
const routeSet = new Set(["/", ...routeMatches]);
if (routeSet.size !== routeMatches.length + 1) failures.push("page routes must be unique");

const importedContent = new Set(
  [...pagesSource.matchAll(/from\s+"\.\/content\/([^"?]+)\.md\?raw"/gu)].map((match) => `${match[1]}.md`),
);
const contentFiles = readdirSync(contentDirectory).filter((name) => name.endsWith(".md")).sort();
for (const name of contentFiles) {
  if (!importedContent.has(name)) failures.push(`${name} is not registered in src/pages.ts`);
}
for (const name of importedContent) {
  if (!contentFiles.includes(name)) failures.push(`src/pages.ts imports missing content/${name}`);
}

for (const name of contentFiles) {
  const source = readFileSync(join(contentDirectory, name), "utf8");
  if (!source.startsWith("# ")) failures.push(`${name} must begin with one level-one heading`);
  if (/pnpm counter:studio(?::browser)? --(?:[ \t]|\\\r?\n)/gu.test(source)) {
    failures.push(`${name} passes a standalone -- through a Counter pnpm script`);
  }
  for (const match of source.matchAll(/\]\(#(\/[^)]+)\)/gu)) {
    if (!routeSet.has(match[1])) failures.push(`${name} links to unknown manual route ${match[1]}`);
  }
  for (const match of source.matchAll(/\]\((http:\/\/[^)]+)\)/gu)) {
    failures.push(`${name} has a non-HTTPS external link: ${match[1]}`);
  }
}

for (const match of capabilitiesSource.matchAll(/route:\s*"([^"]+)"/gu)) {
  if (!routeSet.has(match[1])) failures.push(`capability links to unknown manual route ${match[1]}`);
}

for (const required of ["public/og.png", "index.html", "src/App.tsx", "src/styles.css"]) {
  if (!existsSync(join(manual, required))) failures.push(`missing required manual asset ${required}`);
}

if (failures.length > 0) {
  console.error("Developer manual verification failed:");
  for (const failure of failures) console.error(`- ${failure}`);
  process.exitCode = 1;
} else {
  console.log(`Developer manual verified: ${routeMatches.length} pages, ${contentFiles.length} Markdown guides, all internal routes valid.`);
}
